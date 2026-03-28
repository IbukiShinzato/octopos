use core::fmt::Write;
use core::str;

use kernel::abi::Ioctl;

use crate::ioctl;
use crate::syscall::{Fd, read, write};

pub struct Stdout;

impl core::fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        match write(Fd::STDOUT, s.as_bytes()) {
            Ok(len) if len == s.len() => Ok(()),
            _ => Err(core::fmt::Error),
        }
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        <$crate::Stdout as core::fmt::Write>::write_fmt(
            &mut $crate::Stdout,
            format_args!($($arg)*),
        ).unwrap();
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };

    ($($arg:tt)*) => {
        $crate::print!("{}\n", format_args!($($arg)*))
    };
}

pub struct Stderr;

impl core::fmt::Write for Stderr {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        match write(Fd::STDERR, s.as_bytes()) {
            Ok(len) if len == s.len() => Ok(()),
            _ => Err(core::fmt::Error),
        }
    }
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {
        <$crate::Stderr as core::fmt::Write>::write_fmt(
            &mut $crate::Stderr,
            format_args!($($arg)*),
        ).unwrap();
    };
}

#[macro_export]
macro_rules! eprintln {
    () => {
        $crate::eprint!("\n")
    };

    ($($arg:tt)*) => {
        $crate::eprint!("{}\n", format_args!($($arg)*))
    };
}

const LINE_MAX: usize = 256;

#[derive(Debug, Clone, Copy)]
pub struct LineEditor<'a> {
    /// buffer for the current line being edited
    buf: [u8; LINE_MAX],
    /// how many bytes are in the `buf`
    len: usize,
    /// index of the character being edited in the `buf`
    cursor: usize,
    /// the prompt to display before the line editor
    prompt: &'a str,
}

impl<'a> LineEditor<'a> {
    pub fn new() -> Self {
        Self {
            buf: [0; LINE_MAX],
            len: 0,
            cursor: 0,
            prompt: "",
        }
    }

    pub fn read_line(&mut self, prompt: &'a str) -> Option<&str> {
        ioctl(Fd::STDIN, Ioctl::CONSOLE_SET_RAW, 1).expect("failed to set console to raw mode");

        self.len = 0;
        self.cursor = 0;

        self.prompt = prompt;
        Stderr.write_str(self.prompt).unwrap();

        let mut c = [0u8; 1];
        loop {
            read(Fd::STDIN, &mut c).unwrap();

            match c[0] {
                b'\n' | b'\r' => {
                    Stdout.write_str("\r\n").unwrap();
                    break;
                }

                b'\x08' | b'\x7f' => {
                    self.backspace();
                }

                b'\x1b' => {
                    self.handle_escape();
                }

                // Ctrl-A
                b'\x01' => {
                    self.move_to_start();
                }

                // Ctrl-E
                b'\x05' => {
                    self.move_to_end();
                }

                // Ctrl-U
                b'\x15' => {
                    self.kill_line();
                }

                // Ctrl-W
                b'\x17' => {
                    self.kill_word();
                }

                c if c.is_ascii_graphic() || c == b' ' => {
                    self.insert(c);
                }

                // EOF
                b'\x04' if self.len == 0 => {
                    Stdout.write_str("\r\n").unwrap();
                    return None;
                }

                _ => {}
            }
        }

        ioctl(Fd::STDIN, Ioctl::CONSOLE_SET_RAW, 0).expect("failed to set console to cooked mode");

        Some(unsafe { str::from_utf8_unchecked(&self.buf[..self.len]) })
    }

    /// Called when `0x1b` is read.
    /// Reads the next two bytes and dispatches the correct handler.
    fn handle_escape(&mut self) {
        let mut seq = [0u8; 2];
        read(Fd::STDIN, &mut seq[..1]).unwrap();
        read(Fd::STDIN, &mut seq[1..]).unwrap();

        match seq {
            [b'[', b'D'] => self.move_left(),
            [b'[', b'C'] => self.move_right(),
            _ => {}
        }
    }

    fn insert(&mut self, c: u8) {
        // shift buf[cursor..len] right by 1
        for i in (self.cursor..self.len).rev() {
            self.buf[i + 1] = self.buf[i];
        }

        // place c at cursor
        self.buf[self.cursor] = c;

        self.cursor += 1;
        self.len += 1;

        self.redraw();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }

        // shift buf[cursor..len] left by 1
        for i in (self.cursor - 1)..(self.len - 1) {
            self.buf[i] = self.buf[i + 1];
        }

        self.cursor -= 1;
        self.len -= 1;

        self.redraw();
    }

    fn kill_line(&mut self) {
        // shift buf[cursor..len] to buf[0..]
        for i in self.cursor..self.len {
            self.buf[i - self.cursor] = self.buf[i];
        }

        self.len -= self.cursor;
        self.cursor = 0;

        self.redraw();
    }

    fn kill_word(&mut self) {
        let mut i = self.cursor;

        // skip over any spaces before a word
        while i > 0 && self.buf[i - 1] == b' ' {
            i -= 1;
        }

        // skip over the first word
        while i > 0 && self.buf[i - 1] != b' ' {
            i -= 1;
        }

        // shift buf[cursor..len] left by cursor - i
        for j in self.cursor..self.len {
            self.buf[j - (self.cursor - i)] = self.buf[j];
        }

        self.len -= self.cursor - i;
        self.cursor = i;

        self.redraw();
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;

            // emit \x1b[D to move cursor left
            Stdout.write_str("\x1b[D").unwrap();
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.len {
            self.cursor += 1;

            // emit \x1b[C to move cursor right
            Stdout.write_str("\x1b[C").unwrap();
        }
    }

    fn move_to_start(&mut self) {
        self.cursor = 0;
        self.redraw();
    }

    fn move_to_end(&mut self) {
        self.cursor = self.len;
        self.redraw();
    }

    fn redraw(&self) {
        // move cursor to start of line
        // erase everything to the right of the cursor
        Stdout.write_str("\r\x1b[K").unwrap();

        Stdout.write_str(self.prompt).unwrap();

        // write input buffer
        Stdout
            .write_str(unsafe { str::from_utf8_unchecked(&self.buf[..self.len]) })
            .unwrap();

        // move cursor to correct position if it isn't already
        let back = self.len - self.cursor;
        if back > 0 {
            Stdout.write_fmt(format_args!("\x1b[{}D", back)).unwrap();
        }
    }

    // fn redraw_full(&self, prompt: &str) {
    //     todo!()
    // }
}

impl<'a> Default for LineEditor<'a> {
    fn default() -> Self {
        Self::new()
    }
}
