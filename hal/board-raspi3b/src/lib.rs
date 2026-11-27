#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

//! Raspberry Pi 3B (BCM2837) board implementation for [`hal_core`].
//!
//! All BCM2837 physical MMIO addresses live strictly inside this crate so that
//! `kernel/` stays portable across boards (e.g. RPi 3B, RPi Zero 2 W, or other ARM64 SBCs).

mod gpio;
pub mod irq;
mod mmio;
pub mod timer;
pub mod uart_mini;
pub mod uart_pl011;

use hal_core::{
    Board, BoardInfo, InterruptController, MemoryKind, MemoryRegion, PAGE_SIZE, SerialConsole,
    SmpError,
};

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
/// BCM2837 Arasan EMMC (SDHCI) controller base address.
pub const EMMC_BASE: usize = PERIPHERAL_BASE + 0x0030_0000;
/// BCM2837 VideoCore Mailbox 0/1 register base address (`0x3F00_B880`).
pub const MAILBOX_BASE: usize = PERIPHERAL_BASE + 0x0000_B880;
/// BCM2837 PWM0 audio controller base address (`0x3F20_C000`).
pub const PWM_BASE: usize = PERIPHERAL_BASE + 0x0020_C000;
/// BCM2837 Synopsys DWC2 USB OTG controller base (`0x3F98_0000`, `16 KiB` covers core/host/DFIFO0).
pub const USB_DWC2_BASE: usize = PERIPHERAL_BASE + 0x0098_0000;
pub const USB_DWC2_SIZE: usize = 0x4000;
pub const USB_DWC2_IRQ: u32 = 9;
/// BCM2837 VideoCore uncached L2-bypass bus alias offset (`0xC000_0000`).
pub const VC_BUS_ALIAS_OFFSET: usize = 0xC000_0000;
/// BCM2837 EMMC GPU interrupt number (`IRQ2` bit `30`).
pub const EMMC_IRQ: u32 = 62;

/// Physical base of the RPi 3B / QEMU `raspi3b` firmware spin-table mailboxes (`0xD8 + 8 * core`).
const SPIN_TABLE_BASE: usize = 0xD8;

/// Conservative ARM-accessible physical RAM limit on 1 GiB RPi 3B (948 MiB before VideoCore split).
pub const DEFAULT_ARM_RAM_SIZE: usize = 0x3B40_0000;
/// Physical VideoCore RAM aperture (`0x3B40_0000 .. 0x3F00_0000`) on 1 GiB RPi 3B.
pub const VCRAM_PHYS_BASE: usize = DEFAULT_ARM_RAM_SIZE;
pub const VCRAM_PHYS_SIZE: usize = PERIPHERAL_BASE - DEFAULT_ARM_RAM_SIZE;
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
    /// Initializes early board peripherals (UART and interrupt controller) using
    /// `mmio_virt_offset` (`0` for identity mapping, `0xFFFF_0000_0000_0000` for higher-half).
    pub fn init(mmio_virt_offset: usize) -> Self {
        mmio::set_mmio_virt_offset(mmio_virt_offset);

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
            has_wifi: cfg!(feature = "wireless"),
            has_bluetooth: cfg!(feature = "wireless"),
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

    fn uart_mmio_region(&self) -> MemoryRegion {
        #[cfg(not(feature = "mini-uart"))]
        {
            MemoryRegion::new(PL011_UART0_BASE, PAGE_SIZE, MemoryKind::DeviceMmio)
        }
        #[cfg(feature = "mini-uart")]
        {
            MemoryRegion::new(AUX_BASE, PAGE_SIZE, MemoryKind::DeviceMmio)
        }
    }

    fn sd_mmio_region(&self) -> Option<(MemoryRegion, u32)> {
        Some((
            MemoryRegion::new(EMMC_BASE, PAGE_SIZE, MemoryKind::DeviceMmio),
            EMMC_IRQ,
        ))
    }

    fn mailbox_mmio_region(&self) -> Option<(MemoryRegion, usize)> {
        Some((
            MemoryRegion::new(MAILBOX_BASE, PAGE_SIZE, MemoryKind::DeviceMmio),
            VC_BUS_ALIAS_OFFSET,
        ))
    }

    fn framebuffer_phys_window(&self) -> Option<(usize, usize)> {
        Some((VCRAM_PHYS_BASE, VCRAM_PHYS_SIZE))
    }

    fn audio_mmio_region(&self) -> Option<MemoryRegion> {
        Some(MemoryRegion::new(
            PWM_BASE,
            PAGE_SIZE,
            MemoryKind::DeviceMmio,
        ))
    }

    fn usb_mmio_region(&self) -> Option<(MemoryRegion, u32)> {
        Some((
            MemoryRegion::new(USB_DWC2_BASE, USB_DWC2_SIZE, MemoryKind::DeviceMmio),
            USB_DWC2_IRQ,
        ))
    }

    fn wake_secondary_core(&self, core_id: usize, entry_phys: usize) -> Result<(), SmpError> {
        bcm2837_wake_secondary_core(core_id, entry_phys)
    }

    fn halt(&self) -> ! {
        halt_forever()
    }
}

/// Shared BCM2837 firmware spin-table secondary-core wakeup usable across RPi 3B & Zero 2 W.
pub fn bcm2837_wake_secondary_core(core_id: usize, entry_phys: usize) -> Result<(), SmpError> {
    if core_id == 0 || core_id >= 4 {
        return Err(SmpError::InvalidCore);
    }

    let mailbox_virt = 0xFFFF_0000_0000_0000usize + SPIN_TABLE_BASE + core_id * 8;
    #[cfg(target_arch = "aarch64")]
    // SAFETY: `mailbox_virt` maps BCM2837 AArch64 firmware spin-table slot
    // (`0xE0`/`0xE8`/`0xF0`) in page 0 via `TTBR1_EL1`. Cleaning cache to PoC
    // (`dc civac`) + `dsb sy` + `sev` ensures sleeping EL2 cores observe `entry_phys`.
    unsafe {
        core::ptr::write_volatile(mailbox_virt as *mut u64, entry_phys as u64);
        core::arch::asm!(
            "dc civac, {addr}",
            "dsb sy",
            "sev",
            addr = in(reg) mailbox_virt,
            options(nostack)
        );
    }
    #[cfg(not(target_arch = "aarch64"))]
    let _ = (mailbox_virt, entry_phys);

    Ok(())
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
