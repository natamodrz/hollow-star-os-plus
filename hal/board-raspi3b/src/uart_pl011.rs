//! ARM PL011 (UART0) driver for Raspberry Pi 3B (BCM2837).

use hal_core::SerialConsole;

use crate::PL011_UART0_BASE;
use crate::gpio::{GpioAlt, configure_uart_pins};
use crate::mmio::MmioBlock;

const DR: usize = 0x00;
const FR: usize = 0x18;
const IBRD: usize = 0x24;
const FBRD: usize = 0x28;
const LCRH: usize = 0x2C;
const CR: usize = 0x30;
const ICR: usize = 0x44;

const FR_BUSY: u32 = 1 << 3;
const FR_RXFE: u32 = 1 << 4;
const FR_TXFF: u32 = 1 << 5;

const LCRH_FEN: u32 = 1 << 4;
const LCRH_WLEN_8: u32 = 0b11 << 5;

const CR_UARTEN: u32 = 1 << 0;
const CR_TXE: u32 = 1 << 8;
const CR_RXE: u32 = 1 << 9;

/// PL011 UART0 serial console driver.
pub struct Pl011Uart {
    mmio: MmioBlock,
}

impl Pl011Uart {
    /// Creates an uninitialized PL011 UART0 handle bound to BCM2837 UART0 MMIO.
    pub const fn new() -> Self {
        // SAFETY: `PL011_UART0_BASE` is the physical MMIO base of UART0 on BCM2837,
        // identity-mapped during EL1 early boot.
        let mmio = unsafe { MmioBlock::new(PL011_UART0_BASE) };
        Self { mmio }
    }
}

impl Default for Pl011Uart {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialConsole for Pl011Uart {
    fn init(&mut self) {
        // Disable UART0 before reconfiguring GPIO mux and baud divisors.
        self.mmio.write(CR, 0);
        self.flush();

        configure_uart_pins(GpioAlt::Alt0);

        // Clear all pending interrupts.
        self.mmio.write(ICR, 0x7FF);

        // Configure 115200 baud assuming standard 48 MHz PL011 UARTCLK:
        // 48_000_000 / (16 * 115200) = 26.041666... -> IBRD = 26, FBRD = 3.
        self.mmio.write(IBRD, 26);
        self.mmio.write(FBRD, 3);

        // Enable FIFO and 8N1 word format.
        self.mmio.write(LCRH, LCRH_FEN | LCRH_WLEN_8);

        // Enable UART0, TX, and RX.
        self.mmio.write(CR, CR_UARTEN | CR_TXE | CR_RXE);
    }

    fn write_byte(&mut self, byte: u8) {
        while (self.mmio.read(FR) & FR_TXFF) != 0 {
            core::hint::spin_loop();
        }
        self.mmio.write(DR, u32::from(byte));
    }

    fn read_byte(&mut self) -> Option<u8> {
        if (self.mmio.read(FR) & FR_RXFE) != 0 {
            None
        } else {
            Some((self.mmio.read(DR) & 0xFF) as u8)
        }
    }

    fn flush(&mut self) {
        while (self.mmio.read(FR) & FR_BUSY) != 0 {
            core::hint::spin_loop();
        }
    }
}
