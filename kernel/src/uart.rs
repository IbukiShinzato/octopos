use core::ptr;

use crate::console::Console;
use crate::memlayout::UART0;
use crate::printf::PRINTF;
use crate::proc::{self, Channel};
use crate::spinlock::SpinLock;

// UART control registers are memory-mapped at address UART0.
// http://byterunner.com/16550.html

/// Receive Holding Register (for input bytes)
const RHR: usize = 0;

/// Transmit Holding Register (for output bytes)
const THR: usize = 0;

/// Interrupt Enable Register
const IER: usize = 1;
const IER_RX_ENABLE: u8 = 1 << 0;
const IER_TX_ENABLE: u8 = 1 << 1;

/// FIFO Control Register
const FCR: usize = 2;
const FCR_FIFO_ENABLE: u8 = 1 << 0;
/// Clear the content of the two FIFOs
const FCR_FIFO_CLEAR: u8 = 3 << 1;

/// Interrupt Status Register
const ISR: usize = 2;

/// Line Control Register
const LCR: usize = 3;
const LCR_EIGHT_BITS: u8 = 3;
/// Special mode to set baud rate
const LCR_BAUD_LATCH: u8 = 1 << 7;

/// Line Status Register
const LSR: usize = 5;
/// Input is waiting to be read from RHR
const LSR_RX_READY: u8 = 1 << 0;
/// THR can accept another character to send
const LSR_TX_IDLE: u8 = 1 << 5;

pub static UART: SpinLock<Uart> = SpinLock::new(Uart::new(UART0), "uart");

#[derive(Debug)]
pub struct Uart {
    base_address: usize,
    tx_busy: bool,
    tx_channel: (),
}

impl Uart {
    pub const fn new(base_address: usize) -> Self {
        Self {
            base_address,
            tx_busy: false,
            tx_channel: (),
        }
    }

    /// Read a byte from the given UART register.
    fn read_reg(&self, reg: usize) -> u8 {
        // Safety: reading from memory-mapped UART register
        unsafe { ptr::read_volatile((self.base_address as *mut u8).add(reg)) }
    }

    /// Write a byte to the given UART register.
    fn write_reg(&mut self, reg: usize, value: u8) {
        // Safety: writing to memory-mapped UART register
        unsafe { ptr::write_volatile((self.base_address as *mut u8).add(reg), value) }
    }

    /// Initialize the UART to 38.4K baud, 8 data bits, no parity, one stop bit.
    pub fn init(&mut self) {
        // disable interrupts
        self.write_reg(IER, 0x00);

        // special mode to set baud rate
        self.write_reg(LCR, LCR_BAUD_LATCH);

        // LSB for baud rate of 38.4K
        self.write_reg(0, 0x03);

        // MSB for baud rate of 38.4K
        self.write_reg(1, 0x00);

        // leave set-baud mode
        self.write_reg(LCR, LCR_EIGHT_BITS);

        // reset and enable FIFOs
        self.write_reg(FCR, FCR_FIFO_ENABLE | FCR_FIFO_CLEAR);

        // enable transmit and receive interrupts
        self.write_reg(IER, IER_TX_ENABLE | IER_RX_ENABLE);
    }
}

/// Reads one input character from the UART.
/// Returns `None` if no input is waiting.
///
/// Does not acquire `UART.lock()` because it only reads receive-side registers (`LSR`, `RHR`),
/// which do not conflict with the transmit-side writes in `write()` and `write_sync()`.
pub fn getc() -> Option<u8> {
    // Safety: receive-side register reads do not conflict with concurrent transmit-side writes.
    let uart = unsafe { UART.get_mut_unchecked() };

    if uart.read_reg(LSR) & LSR_RX_READY != 0 {
        Some(uart.read_reg(RHR))
    } else {
        None
    }
}

/// Writes the given buffer to UART.
/// Sleeps if the UART is not ready to transmit, and will be woken up by a UART interrupt when it
/// is ready. Used by the user-level `write()` system call.
pub fn write(buf: &[u8]) {
    let mut uart = UART.lock();

    for c in buf {
        if PRINTF.is_panicking() {
            // Another core panicked and trying to print the panic message.
            // Stop printing and release the lock asap.
            return;
        }

        while uart.tx_busy {
            // Wait for a UART transmit-complete interrupt to set `tx_busy` to false.
            uart = proc::sleep(Channel::Buffer(&uart.tx_channel as *const _ as usize), uart);

            if PRINTF.is_panicking() {
                // While we are sleeping, another core panicked and trying to print the panic
                // message. Stop printing and release the lock asap.
                return;
            }
        }

        uart.write_reg(THR, *c);
        uart.tx_busy = true;
    }
}

/// Polling alternative to `write()` that does not rely on interrupts.
///
/// Used by the kernel `print!` macro and to echo input characters in `Console::handle_interrupt()`.
///
/// Acquires `UART.lock()` to serialize with `write()`, ensuring that interrupt-driven user-space
/// output and synchronous kernel output cannot interleave. If this hart already holds the lock
/// (e.g. a panic fired from within `write()`), the lock acquisition is skipped to avoid deadlock.
pub fn write_sync(buf: &[u8]) {
    // If one core already panicked, stop all other cores.
    if PRINTF.is_panicked() {
        loop {
            core::hint::spin_loop();
        }
    }

    // Skip acquisition if this hart already holds the lock (same-CPU panic scenario).
    let _guard = (!UART.is_holding()).then(|| UART.lock());

    // Safety: the lock is held either via `_guard` above or by the caller.
    let uart = unsafe { UART.get_mut_unchecked() };

    for c in buf {
        // wait for Transmit Holding Empty to be set in LSR
        while (uart.read_reg(LSR) & LSR_TX_IDLE) == 0 {}

        uart.write_reg(THR, *c);
    }
}

/// Handles a UART interrupt.
/// Acknowledges the interrupt, wakes up the sending thread if a transmit-complete interrupt, and
/// reads and processes incoming characters.
/// Called if the UART has finished transmitting a character or if there is an input character
/// waiting to be read.
pub fn handle_interrupt() {
    {
        let mut uart = UART.lock();

        // acknowledge the interrupt
        uart.read_reg(ISR);

        if (uart.read_reg(LSR) & LSR_TX_IDLE) != 0 {
            // UART finished transmitting, wake up the sending thread.
            uart.tx_busy = false;
            proc::wakeup(Channel::Buffer(&uart.tx_channel as *const _ as usize));
        }

        // drop uart lock
    }

    // read and process incoming characters, if any
    while let Some(c) = getc() {
        Console::handle_interrupt(c);
    }
}

/// Initializes the UART.
///
/// # Safety
/// This function must be called only once during system initialization.
pub unsafe fn init() {
    unsafe { UART.get_mut_unchecked().init() }
}
