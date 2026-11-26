#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

//! Raspberry Pi 3B (BCM2837) board implementation for [`hal_core`].
//!
//! All BCM2837 physical MMIO addresses live strictly inside this crate so that
//! `kernel/` stays portable across boards (e.g. RPi 3B, CM0, or other ARM64 SBCs).

mod gpio;
pub mod irq;
mod mmio;
pub mod timer;
pub mod uart_mini;
pub mod uart_pl011;

use hal_core::{Board, BoardInfo, InterruptController, MemoryKind, MemoryRegion, SerialConsole};

/// BCM2837 peripheral MMIO physical base address on Raspberry Pi 3B.
pub const PERIPHERAL_BASE: usize = 0x3F00_0000;
/// BCM2837 peripheral MMIO window length (16 MiB).
pub const PERIPHERAL_SIZE: usize = 0x0100_0000;
/// BCM2837 ARM Local Peripherals MMIO base address (`0x4000_0000`).
pub const ARM_LOCAL_BASE: usize = 0x4000_0000;
/// BCM2837 ARM Local Peripherals MMIO window length (128 KiB).
pub const ARM_LOCAL_SIZE: usize = 0x0002_0000;

/// BCM2837 GPIO controller base address.
pub const GPIO_BASE: usize = PERIPHERAL_BASE + 0x0020_0000;
/// BCM2837 ARM PL011 UART0 base address.
pub const PL011_UART0_BASE: usize = PERIPHERAL_BASE + 0x0020_1000;
/// BCM2837 AUX (Mini-UART / SPI) base address.
pub const AUX_BASE: usize = PERIPHERAL_BASE + 0x0021_5000;

/// Conservative ARM-accessible physical RAM limit on 1 GiB RPi 3B (948 MiB before VideoCore split).
pub const DEFAULT_ARM_RAM_SIZE: usize = 0x3B40_0000;
const FIRMWARE_RESERVED_END: usize = 0x0008_0000;

static RASPI3B_MEMORY_REGIONS: [MemoryRegion; 4] = [
    MemoryRegion::new(0x0000_0000, FIRMWARE_RESERVED_END, MemoryKind::Reserved),
    MemoryRegion::new(
        FIRMWARE_RESERVED_END,
        DEFAULT_ARM_RAM_SIZE - FIRMWARE_RESERVED_END,
        MemoryKind::UsableRam,
    ),
    MemoryRegion::new(PERIPHERAL_BASE, PERIPHERAL_SIZE, MemoryKind::DeviceMmio),
    MemoryRegion::new(ARM_LOCAL_BASE, ARM_LOCAL_SIZE, MemoryKind::DeviceMmio),
];

/// Active early serial console type for Raspberry Pi 3B.
#[cfg(not(feature = "mini-uart"))]
pub type ActiveConsole = uart_pl011::Pl011Uart;

/// Active early serial console type when `mini-uart` feature is enabled.
#[cfg(feature = "mini-uart")]
pub type ActiveConsole = uart_mini::MiniUart;

#[cfg(not(feature = "mini-uart"))]
const UART_MODE_NAME: &str = "PL011 (UART0)";

#[cfg(feature = "mini-uart")]
const UART_MODE_NAME: &str = "Mini-UART (UART1)";

/// Board handle for Raspberry Pi 3B implementing [`hal_core::Board`].
pub struct Raspi3bBoard {
    console: ActiveConsole,
    irq: irq::Bcm2837IrqController,
    timer: timer::ArmGenericTimer,
}

impl Raspi3bBoard {
    /// Initializes early board peripherals (UART and interrupt controller) and returns the board handle.
    pub fn init() -> Self {
        let mut console = ActiveConsole::new();
        console.init();

        let mut irq = irq::Bcm2837IrqController::new();
        irq.init();

        Self {
            console,
            irq,
            timer: timer::ArmGenericTimer::new(),
        }
    }

    /// Returns a fresh console handle for emergency panic logging.
    pub fn emergency_console() -> ActiveConsole {
        ActiveConsole::new()
    }

    /// Returns a lightweight interrupt controller handle for IRQ dispatch.
    pub const fn irq_handle() -> irq::Bcm2837IrqController {
        irq::Bcm2837IrqController::new()
    }

    /// Returns a lightweight system timer handle for IRQ re-arming.
    pub const fn timer_handle() -> timer::ArmGenericTimer {
        timer::ArmGenericTimer::new()
    }
}

impl Board for Raspi3bBoard {
    type Console = ActiveConsole;
    type Irq = irq::Bcm2837IrqController;
    type Timer = timer::ArmGenericTimer;

    fn info(&self) -> BoardInfo {
        BoardInfo {
            name: "Raspberry Pi 3B",
            soc: "BCM2837 (4x Cortex-A53)",
            uart_mode: UART_MODE_NAME,
            cpu_count: 4,
            ram_base: 0x0000_0000,
            ram_size_bytes: DEFAULT_ARM_RAM_SIZE,
        }
    }

    fn console(&mut self) -> &mut Self::Console {
        &mut self.console
    }

    fn irq_controller(&mut self) -> &mut Self::Irq {
        &mut self.irq
    }

    fn timer(&mut self) -> &mut Self::Timer {
        &mut self.timer
    }

    fn memory_regions(&self) -> &[MemoryRegion] {
        &RASPI3B_MEMORY_REGIONS
    }

    fn halt(&self) -> ! {
        halt_forever()
    }
}

/// Halts the calling CPU core in a low-power wait loop.
pub fn halt_forever() -> ! {
    loop {
        #[cfg(target_arch = "aarch64")]
        // SAFETY: `wfe` puts the current core into a low-power standby until an event arrives.
        unsafe {
            core::arch::asm!("wfe", options(nomem, nostack, preserves_flags));
        }

        #[cfg(not(target_arch = "aarch64"))]
        core::hint::spin_loop();
    }
}
