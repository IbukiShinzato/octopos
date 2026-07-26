use core::fmt::Display;

use crate::spinlock::SpinLock;

pub static MESSAGE_BUFFER: SpinLock<MessageBuffer> = SpinLock::new(MessageBuffer::new(), "message");

const BUFSIZE: usize = 4096;

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

pub struct MessageBuffer {
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

    fn set_msg(&mut self, buf: &[u8], len: usize) -> Result<(), MessageError> {
        if len > BUFSIZE || len > buf.len() {
            err!(MessageError::OutOfRange);
        }

        for (i, &c) in buf.iter().take(len).enumerate() {
            self.data[i] = c;
        }

        self.valid_len = len;

        Ok(())
    }
}
