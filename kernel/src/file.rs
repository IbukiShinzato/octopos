use core::slice;
use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::sync::Arc;

use crate::console::Console;
use crate::fs::{BSIZE, FsError, Inode, Stat};
use crate::log::Operation;
use crate::param::{MAXOPBLOCKS, NDEV, NFILE};
use crate::pipe::Pipe;
use crate::proc;
use crate::sleeplock::SleepLock;
use crate::spinlock::SpinLock;
use crate::syscall::SysError;
use crate::vm::VA;

#[derive(Debug, Clone)]
pub enum FileType {
    None,
    Pipe { pipe: Arc<Pipe> },
    Inode { inode: Inode },
    Device { inode: Inode, major: u16 },
}

/// File metadata protected by table-wide spinlock
#[derive(Debug, Clone)]
pub struct FileMeta {
    pub ref_count: usize,
}

/// Per-file mutable state protected by per-file sleeplock
#[derive(Debug, Clone)]
pub struct FileInner {
    /// Index into the file table.
    pub readable: bool,
    pub writeable: bool,
    pub r#type: FileType,
    pub offset: u32,
}

impl Default for FileInner {
    fn default() -> Self {
        Self {
            readable: false,
            writeable: false,
            r#type: FileType::None,
            offset: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RecoveryType {
    None,
    Inode { dev: u32, inum: u32 },
    Device { dev: u32, inum: u32, major: u16 },
}

#[derive(Debug, Clone, Copy)]
pub struct RLog {
    pub readable: bool,
    pub writeable: bool,
    pub r#type: RecoveryType,
    pub offset: u32,
}

impl RLog {
    pub const fn new() -> Self {
        Self {
            readable: false,
            writeable: false,
            r#type: RecoveryType::None,
            offset: 0,
        }
    }
}
pub static FILE_TABLE: FileTable = FileTable::new();
pub static R_LOG_TABLE: RLogTable = RLogTable::new();

static TARGET_READ_COUNT: AtomicUsize = AtomicUsize::new(0);
const READ_UE_TARGET_ID: usize = 1;

#[derive(Debug)]
pub struct RLogTable {
    pub entries: SpinLock<[RLog; NFILE]>,
}

impl RLogTable {
    const fn new() -> Self {
        let entries = SpinLock::new([const { RLog::new() }; NFILE], "R-Log-table");

        Self { entries }
    }
}

/// Global file table
#[derive(Debug)]
pub struct FileTable {
    /// Protects allocation and reference counts
    pub meta: SpinLock<[FileMeta; NFILE]>,
    /// Protects references to heap-allocated per-file states.
    pub inner: SpinLock<[Option<Arc<SleepLock<FileInner>>>; NFILE]>,
}

impl FileTable {
    const fn new() -> Self {
        let meta = SpinLock::new([const { FileMeta { ref_count: 0 } }; NFILE], "filetable");
        let inner = SpinLock::new([const { None }; NFILE], "file-inner-table");

        Self { meta, inner }
    }

    pub fn get_inner(&self, id: usize) -> Arc<SleepLock<FileInner>> {
        let inner_table = self.inner.lock();

        Arc::clone(
            inner_table[id]
                .as_ref()
                .expect("FileInner is not allocated"),
        )
    }

    pub fn take_inner(&self, id: usize) -> Arc<SleepLock<FileInner>> {
        let mut inner_table = self.inner.lock();

        inner_table[id].take().expect("FileInner is not allocated")
    }

    pub fn replace_file_inner(&self, id: usize) -> Result<Arc<SleepLock<FileInner>>, SysError> {
        let r_log = R_LOG_TABLE.entries.lock()[id];
        let (readable, writeable, r#type, offset) =
            (r_log.readable, r_log.writeable, r_log.r#type, r_log.offset);

        let restored_type = match r#type {
            RecoveryType::None => {
                return Err(SysError::IoError);
            }
            RecoveryType::Inode { dev, inum } => {
                let inode = Inode::recovery_inode_get(dev, inum).ok_or(SysError::IoError)?;
                FileType::Inode { inode }
            }
            RecoveryType::Device { dev, inum, major } => {
                let inode = Inode::recovery_inode_get(dev, inum).ok_or(SysError::IoError)?;
                FileType::Device { inode, major }
            }
        };

        let new_inner = FileInner {
            readable,
            writeable,
            r#type: restored_type,
            offset,
        };
        let new_inner = Arc::new(SleepLock::new(new_inner, "file"));

        let mut inner_table = self.inner.lock();

        let slot = inner_table
            .get_mut(id)
            .and_then(|entry| entry.as_mut())
            .ok_or(SysError::IoError)?;

        let old_inner = core::mem::replace(slot, new_inner);

        Ok(old_inner)
    }
}

/// File handle, just an index into the `FileTable`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub id: usize,
}

impl File {
    /// Allocates a file structure.
    pub fn alloc() -> Result<Self, FsError> {
        let mut meta = FILE_TABLE.meta.lock();

        for (i, meta) in meta.iter_mut().enumerate() {
            if meta.ref_count == 0 {
                let new_inner = Arc::new(SleepLock::new(FileInner::default(), "file"));

                {
                    let mut inner_table = FILE_TABLE.inner.lock();
                    inner_table[i] = Some(new_inner);
                };

                meta.ref_count = 1;

                return Ok(Self { id: i });
            }
        }

        err!(FsError::OutOfFile);
    }

    /// Incremets the reference count for the file.
    pub fn dup(&mut self) -> Self {
        let meta = &mut FILE_TABLE.meta.lock()[self.id];

        assert!(meta.ref_count >= 1, "filedup");

        meta.ref_count += 1;

        self.clone()
    }

    /// Decrements the reference count and closes the file if it reaches 0.
    pub fn close(&mut self) {
        let mut meta_guard = FILE_TABLE.meta.lock();
        let meta = &mut meta_guard[self.id];

        assert!(meta.ref_count >= 1, "fileclose");

        meta.ref_count -= 1;
        if meta.ref_count > 0 {
            return;
        }

        let inner_copy = {
            let file_inner_ref = FILE_TABLE.take_inner(self.id);
            let mut inner = file_inner_ref.lock();
            // copy inner before resetting fields
            let copy = inner.clone();

            meta.ref_count = 0;
            inner.r#type = FileType::None;

            drop(inner);

            {
                let mut r_logs = R_LOG_TABLE.entries.lock();
                r_logs[self.id] = RLog::new();
            }

            drop(meta_guard);
            copy
        }; // Release locks before closing Pipe or putting Inode.

        match inner_copy.r#type {
            FileType::None => {}
            FileType::Pipe { pipe } => {
                pipe.close(inner_copy.writeable);
            }
            FileType::Inode { inode } | FileType::Device { inode, .. } => {
                let _op = Operation::begin();
                inode.put();
            }
        }
    }

    /// Gets metadata about file.
    pub fn stat(&self, addr: VA) -> Result<(), SysError> {
        let file_inner_ref = FILE_TABLE.get_inner(self.id);
        let file_inner = file_inner_ref.lock();

        match &file_inner.r#type {
            FileType::Inode { inode } | FileType::Device { inode, .. } => {
                let inode_inner = inode.lock();
                let stat = inode.stat(&inode_inner);
                inode.unlock(inode_inner);

                let src = unsafe {
                    slice::from_raw_parts(&stat as *const _ as *const u8, size_of::<Stat>())
                };
                if log!(proc::copy_to_user(src, addr)).is_err() {
                    err!(SysError::BadAddress);
                }

                Ok(())
            }
            _ => Err(SysError::BadDescriptor),
        }
    }

    /// Reads from file.
    pub fn read(&self, addr: VA, n: usize) -> Result<usize, SysError> {
        let file_inner_ref = FILE_TABLE.get_inner(self.id);
        let mut file_inner = file_inner_ref.lock();

        // prototype: emulate UE detection immediately before readable access
        if should_inject_file_inner_ue(self.id) {
            let _old_inner = FILE_TABLE.replace_file_inner(self.id)?;

            drop(file_inner);

            err!(SysError::IoError);
        }

        if !file_inner.readable {
            err!(SysError::BadDescriptor);
        }

        match &mut file_inner.r#type {
            FileType::None => panic!("fileread"),

            FileType::Pipe { pipe } => pipe.read(addr, n),

            FileType::Inode { inode } => {
                let inode = inode.clone();
                let mut inode_inner = inode.lock();

                let dst = unsafe { slice::from_raw_parts_mut(addr.as_mut_ptr(), n) };
                let read = log!(inode.read(&mut inode_inner, file_inner.offset, dst, true));

                inode.unlock(inode_inner);
                if let Ok(read) = read {
                    file_inner.offset += read;

                    {
                        let mut r_logs = R_LOG_TABLE.entries.lock();
                        r_logs[self.id].offset = file_inner.offset;
                    }
                }

                if let Ok(read) = read {
                    Ok(read as usize)
                } else {
                    err!(SysError::IoError);
                }
            }

            FileType::Device { inode: _, major } => match &DEVICES[*major as usize] {
                Some(dev) => (dev.read)(addr, n),
                None => err!(SysError::NoEntry),
            },
        }
    }

    /// Writes to a file.
    pub fn write(&mut self, addr: VA, n: usize) -> Result<usize, SysError> {
        let file_inner_ref = FILE_TABLE.get_inner(self.id);
        let mut file_inner = file_inner_ref.lock();

        if !file_inner.writeable {
            err!(SysError::BadDescriptor);
        }

        match &mut file_inner.r#type {
            FileType::None => panic!("filewrite"),

            FileType::Pipe { pipe } => pipe.write(addr, n),

            FileType::Inode { inode } => {
                let inode = inode.clone();

                // write a few block at a time to avoid exceeding the maximum log transaction size,
                // including inode, indirect block, allocation blocks, and 2 block of slop for
                // non-aligned writes.
                let max = ((MAXOPBLOCKS - 1 - 1 - 2) / 2) * BSIZE;
                let mut i = 0;

                while i < n {
                    let n1 = (n - i).min(max);

                    // start log transaction
                    let _op = Operation::begin();
                    let mut inode_inner = inode.lock();

                    let src =
                        unsafe { slice::from_raw_parts((addr.as_usize() + i) as *const u8, n1) };
                    let write = log!(inode.write(&mut inode_inner, file_inner.offset, src, true));

                    inode.unlock(inode_inner);

                    // end log transaction
                    drop(_op);

                    match write {
                        Ok(w) if w > 0 => {
                            file_inner.offset += w;

                            let offset = {
                                let mut r_logs = R_LOG_TABLE.entries.lock();
                                r_logs[self.id].offset = file_inner.offset;
                                r_logs[self.id].offset
                            };

                            println!(
                                "DEBUG: File.id={}, FileInner.offset={}, RLog.offset={}",
                                self.id, file_inner.offset, offset
                            );

                            i += w as usize;
                        }
                        _ => break,
                    };
                }

                if i == n {
                    Ok(n)
                } else {
                    err!(SysError::IoError);
                }
            }

            FileType::Device { inode: _, major } => match &DEVICES[*major as usize] {
                Some(dev) => (dev.write)(addr, n),
                None => err!(SysError::NoEntry),
            },
        }
    }

    pub fn ioctl(&self, cmd: usize, arg: usize) -> Result<usize, SysError> {
        let file_inner_ref = FILE_TABLE.get_inner(self.id);
        let file_inner = file_inner_ref.lock();

        match &file_inner.r#type {
            FileType::Device { major, .. } if *major as usize == CONSOLE => {
                Console::ioctl(cmd, arg)
            }
            FileType::Device { .. } => err!(SysError::NotImplemented),

            _ => err!(SysError::BadDescriptor),
        }
    }
}

fn should_inject_file_inner_ue(id: usize) -> bool {
    if id != READ_UE_TARGET_ID {
        return false;
    }

    TARGET_READ_COUNT.fetch_add(1, Ordering::Relaxed) == 1
}

/// File open flags
pub struct OpenFlag;

impl OpenFlag {
    pub const READ_ONLY: usize = 0x000;
    pub const WRITE_ONLY: usize = 0x001;
    pub const READ_WRITE: usize = 0x002;
    pub const CREATE: usize = 0x200;
    pub const TRUNCATE: usize = 0x400;
}

/// Device interface
#[derive(Debug, Clone, Copy)]
pub struct Device {
    pub read: fn(addr: VA, n: usize) -> Result<usize, SysError>,
    pub write: fn(addr: VA, n: usize) -> Result<usize, SysError>,
}

/// Device-specific ioctl commands
pub struct Ioctl;

impl Ioctl {
    pub const CONSOLE_SET_RAW: usize = 1;
    pub const CONSOLE_SET_FG_PID: usize = 2;
}

/// Console device major number
pub const CONSOLE: usize = 1;

/// Device table
pub static DEVICES: [Option<Device>; NDEV] = {
    let mut devices = [None; NDEV];
    devices[CONSOLE] = Some(Device {
        read: Console::read,
        write: Console::write,
    });
    devices
};
