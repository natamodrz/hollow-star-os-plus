//! BCM2837 GPIO pin muxing helpers for UART pins (GPIO 14 & 15).

use crate::GPIO_BASE;
use crate::mmio::MmioBlock;

const GPFSEL1_OFFSET: usize = 0x04;
const GPFSEL3_OFFSET: usize = 0x0C;
const GPFSEL4_OFFSET: usize = 0x10;
const GPFSEL5_OFFSET: usize = 0x14;
const GPPUD_OFFSET: usize = 0x94;
const GPPUDCLK0_OFFSET: usize = 0x98;
const GPPUDCLK1_OFFSET: usize = 0x9C;

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

/// Routes GPIO 48..53 (microSD slot) to ALT3 (`Arasan EMMC SD1`) and sets GPIO 34..39
/// (Wi-Fi SDIO) to passive inputs so `drv-sd` owns the physical microSD card cleanly on RPi 3B.
pub fn configure_emmc_pins() {
    // SAFETY: `GPIO_BASE` is the fixed physical BCM2837 GPIO base identity-mapped during early boot.
    let gpio = unsafe { MmioBlock::new(GPIO_BASE) };

    // Disconnect GPIO 34..39 from SD1 (`GPFSEL3` bits [29:12] = 0b000 Input).
    let mut gpfsel3 = gpio.read(GPFSEL3_OFFSET);
    gpfsel3 &= !(0x3FFFF << 12);
    gpio.write(GPFSEL3_OFFSET, gpfsel3);

    // Route GPIO 48..49 (`GPFSEL4` bits [29:24]) to ALT3 (`0b111`).
    let mut gpfsel4 = gpio.read(GPFSEL4_OFFSET);
    gpfsel4 |= (0b111 << 24) | (0b111 << 27);
    gpio.write(GPFSEL4_OFFSET, gpfsel4);

    // Route GPIO 50..53 (`GPFSEL5` bits [11:0]) to ALT3 (`0b111`).
    let mut gpfsel5 = gpio.read(GPFSEL5_OFFSET);
    gpfsel5 |= 0b111 | (0b111 << 3) | (0b111 << 6) | (0b111 << 9);
    gpio.write(GPFSEL5_OFFSET, gpfsel5);

    // Enable weak pull-ups on CMD/DAT0..3 (GPIO 49..53 = bits 17..21 in GPPUDCLK1) per SDHCI spec.
    gpio.write(GPPUD_OFFSET, 2);
    spin_cycles(150);
    gpio.write(GPPUDCLK1_OFFSET, 0b11111 << 17);
    spin_cycles(150);
    gpio.write(GPPUD_OFFSET, 0);
    gpio.write(GPPUDCLK1_OFFSET, 0);
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
