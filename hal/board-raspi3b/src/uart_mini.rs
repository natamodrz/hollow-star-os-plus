//! BCM2837 AUX Mini-UART (UART1) driver for Raspberry Pi 3B.

use hal_core::SerialConsole;

use crate::AUX_BASE;
use crate::gpio::{GpioAlt, configure_uart_pins};
use crate::mmio::MmioBlock;

const AUX_ENABLES: usize = 0x04;
const AUX_MU_IO: usize = 0x40;
const AUX_MU_IER: usize = 0x44;
const AUX_MU_IIR: usize = 0x48;
const AUX_MU_LCR: usize = 0x4C;
const AUX_MU_MCR: usize = 0x50;
const AUX_MU_LSR: usize = 0x54;
const AUX_MU_CNTL: usize = 0x60;
const AUX_MU_BAUD: usize = 0x68;

const LSR_DATA_READY: u32 = 1 << 0;
const LSR_TX_EMPTY: u32 = 1 << 5;
const LSR_TX_IDLE: u32 = 1 << 6;

/// AUX Mini-UART (UART1) serial console driver.
pub struct MiniUart {
    mmio: MmioBlock,
}

impl MiniUart {
    /// Creates an uninitialized Mini-UART handle bound to BCM2837 AUX MMIO.
    pub const fn new() -> Self {
        // SAFETY: `AUX_BASE` is the physical MMIO base of the BCM2837 AUX block,
        // identity-mapped during EL1 early boot.
        let mmio = unsafe { MmioBlock::new(AUX_BASE) };
        Self { mmio }
    }
}

impl Default for MiniUart {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialConsole for MiniUart {
    fn init(&mut self) {
        // Enable Mini-UART peripheral inside AUX_ENABLES (bit 0).
        let enables = self.mmio.read(AUX_ENABLES);
        self.mmio.write(AUX_ENABLES, enables | 1);

        // Disable TX/RX and interrupts during configuration.
        self.mmio.write(AUX_MU_CNTL, 0);
        self.mmio.write(AUX_MU_IER, 0);
        // Set 8-bit mode (bits [1:0] = 0b11 per BCM2835/2837 errata).
        self.mmio.write(AUX_MU_LCR, 3);
        self.mmio.write(AUX_MU_MCR, 0);
        // Clear TX and RX FIFOs.
        self.mmio.write(AUX_MU_IIR, 0xC6);
        // 115200 baud at 250 MHz system clock: 250_000_000 / (8 * 115200) - 1 = 270.
        self.mmio.write(AUX_MU_BAUD, 270);

        configure_uart_pins(GpioAlt::Alt5);

        // Re-enable TX and RX.
        self.mmio.write(AUX_MU_CNTL, 3);
    }

    fn write_byte(&mut self, byte: u8) {
        while (self.mmio.read(AUX_MU_LSR) & LSR_TX_EMPTY) == 0 {
            core::hint::spin_loop();
        }
        self.mmio.write(AUX_MU_IO, u32::from(byte));
    }

    fn read_byte(&mut self) -> Option<u8> {
        if (self.mmio.read(AUX_MU_LSR) & LSR_DATA_READY) == 0 {
            None
        } else {
            Some((self.mmio.read(AUX_MU_IO) & 0xFF) as u8)
        }
    }

    fn flush(&mut self) {
        while (self.mmio.read(AUX_MU_LSR) & LSR_TX_IDLE) == 0 {
            core::hint::spin_loop();
        }
    }
}
