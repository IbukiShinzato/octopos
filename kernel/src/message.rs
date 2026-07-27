use core::fmt::Display;

use crate::spinlock::SpinLock;

static MESSAGE_BUFFER: SpinLock<MessageBuffer> = SpinLock::new(MessageBuffer::new(), "message");

pub(crate) const BUFSIZE: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageError {
    OutOfRange,
}

impl Display for MessageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MessageError::OutOfRange => write!(f, "out of range"),
        }
    }
}

struct MessageBuffer {
    data: [u8; BUFSIZE],
    valid_len: usize,
}

impl MessageBuffer {
    const fn new() -> Self {
        Self {
            data: [0; BUFSIZE],
            valid_len: 0,
        }
    }

    fn set_msg(&mut self, buf: &[u8], len: usize) -> Result<usize, MessageError> {
        if len > BUFSIZE || len > buf.len() {
            err!(MessageError::OutOfRange);
        }

        self.data[..len].copy_from_slice(&buf[..len]);
        self.valid_len = len;

        Ok(len)
    }

    fn get_msg(&self, buf: &mut [u8], len: usize) -> Result<usize, MessageError> {
        if len > self.valid_len || len > buf.len() {
            err!(MessageError::OutOfRange);
        }

        buf[..len].copy_from_slice(&self.data[..len]);

        Ok(len)
    }
}

pub fn set_msg(buf: &[u8], len: usize) -> Result<usize, MessageError> {
    MESSAGE_BUFFER.lock().set_msg(buf, len)
}

pub fn get_msg(buf: &mut [u8], len: usize) -> Result<usize, MessageError> {
    MESSAGE_BUFFER.lock().get_msg(buf, len)
}
