//! BCM2837 GPIO pin muxing helpers for UART pins (GPIO 14 & 15).

use crate::GPIO_BASE;
use crate::mmio::MmioBlock;

const GPFSEL1_OFFSET: usize = 0x04;
const GPPUD_OFFSET: usize = 0x94;
const GPPUDCLK0_OFFSET: usize = 0x98;

/// Alternate function mode for BCM2837 GPIO pins.
#[derive(Clone, Copy)]
pub enum GpioAlt {
    /// Alternate function 0 (PL011 UART0 on GPIO 14/15).
    Alt0 = 0b100,
    /// Alternate function 5 (Mini-UART / UART1 on GPIO 14/15).
    Alt5 = 0b010,
}

/// Configures GPIO 14 (TXD) and GPIO 15 (RXD) to the requested alternate function
/// and disables internal pull-up/pull-down resistors per BCM2837 specification.
pub fn configure_uart_pins(alt: GpioAlt) {
    // SAFETY: `GPIO_BASE` is the fixed physical MMIO address of the BCM2837 GPIO controller
    // and EL1 early boot runs with identity mapping (MMU disabled).
    let gpio = unsafe { MmioBlock::new(GPIO_BASE) };

    let mut gpfsel1 = gpio.read(GPFSEL1_OFFSET);
    // GPIO 14 occupies bits [14:12], GPIO 15 occupies bits [17:15].
    gpfsel1 &= !((0b111 << 12) | (0b111 << 15));
    let func = alt as u32;
    gpfsel1 |= (func << 12) | (func << 15);
    gpio.write(GPFSEL1_OFFSET, gpfsel1);

    // Disable pull-up/down on pins 14 and 15 (BCM2835/2837 GPPUD sequence).
    gpio.write(GPPUD_OFFSET, 0);
    spin_cycles(150);
    gpio.write(GPPUDCLK0_OFFSET, (1 << 14) | (1 << 15));
    spin_cycles(150);
    gpio.write(GPPUD_OFFSET, 0);
    gpio.write(GPPUDCLK0_OFFSET, 0);
}

fn spin_cycles(count: usize) {
    for _ in 0..count {
        #[cfg(target_arch = "aarch64")]
        // SAFETY: `nop` has no memory or register side effects beyond advancing cycles.
        unsafe {
            core::arch::asm!("nop", options(nomem, nostack, preserves_flags));
        }

        #[cfg(not(target_arch = "aarch64"))]
        core::hint::spin_loop();
    }
}
