#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

//! Raspberry Pi Zero 2 W (`RP3A0` / `BCM2837`, 512 MiB RAM) board implementation for [`hal_core`].
//!
//! Reuses shared BCM2837 peripheral primitives from [`board_raspi3b`] while applying
//! Zero 2 W 512 MiB memory / VideoCore split boundaries and board metadata.

use board_raspi3b::{
    ARM_LOCAL_BASE, ARM_LOCAL_SIZE, ActiveConsole, EMMC_BASE, EMMC_IRQ, MAILBOX_BASE,
    PERIPHERAL_BASE, PERIPHERAL_SIZE, PWM_BASE, Raspi3bBoard, VC_BUS_ALIAS_OFFSET,
    bcm2837_wake_secondary_core, halt_forever, irq, timer,
};
use hal_core::{Board, BoardInfo, MemoryKind, MemoryRegion, PAGE_SIZE, SmpError};

/// Conservative ARM-accessible physical RAM on 512 MiB RPi Zero 2 W (480 MiB + 32 MiB VideoCore).
pub const DEFAULT_ARM_RAM_SIZE: usize = 0x1E00_0000;
/// VideoCore RAM window (`0x1E00_0000 .. 0x2000_0000`) on 512 MiB RPi Zero 2 W.
pub const VCRAM_PHYS_BASE: usize = DEFAULT_ARM_RAM_SIZE;
pub const VCRAM_PHYS_SIZE: usize = 0x0200_0000;
const FIRMWARE_RESERVED_END: usize = 0x0008_0000;

static RASPI02W_MEMORY_REGIONS: [MemoryRegion; 4] = [
    MemoryRegion::new(0x0000_0000, FIRMWARE_RESERVED_END, MemoryKind::Reserved),
    MemoryRegion::new(
        FIRMWARE_RESERVED_END,
        DEFAULT_ARM_RAM_SIZE - FIRMWARE_RESERVED_END,
        MemoryKind::UsableRam,
    ),
    MemoryRegion::new(PERIPHERAL_BASE, PERIPHERAL_SIZE, MemoryKind::DeviceMmio),
    MemoryRegion::new(ARM_LOCAL_BASE, ARM_LOCAL_SIZE, MemoryKind::DeviceMmio),
];

/// Board handle for Raspberry Pi Zero 2 W implementing [`hal_core::Board`].
pub struct Raspi02wBoard {
    inner: Raspi3bBoard,
}

impl Raspi02wBoard {
    /// Initializes early peripherals via shared BCM2837 HAL with `mmio_virt_offset`.
    pub fn init(mmio_virt_offset: usize) -> Self {
        Self {
            inner: Raspi3bBoard::init(mmio_virt_offset),
        }
    }

    pub fn emergency_console() -> ActiveConsole {
        Raspi3bBoard::emergency_console()
    }

    pub const fn irq_handle() -> irq::Bcm2837IrqController {
        Raspi3bBoard::irq_handle()
    }

    pub const fn timer_handle() -> timer::ArmGenericTimer {
        Raspi3bBoard::timer_handle()
    }
}

impl Board for Raspi02wBoard {
    type Console = ActiveConsole;
    type Irq = irq::Bcm2837IrqController;
    type Timer = timer::ArmGenericTimer;

    fn info(&self) -> BoardInfo {
        let base = self.inner.info();
        BoardInfo {
            name: "Raspberry Pi Zero 2 W",
            soc: "RP3A0 / BCM2837 (4x Cortex-A53)",
            uart_mode: base.uart_mode,
            cpu_count: 4,
            ram_base: 0x0000_0000,
            ram_size_bytes: DEFAULT_ARM_RAM_SIZE,
            has_wifi: cfg!(feature = "wireless"),
            has_bluetooth: cfg!(feature = "wireless"),
        }
    }

    fn console(&mut self) -> &mut Self::Console {
        self.inner.console()
    }

    fn irq_controller(&mut self) -> &mut Self::Irq {
        self.inner.irq_controller()
    }

    fn timer(&mut self) -> &mut Self::Timer {
        self.inner.timer()
    }

    fn memory_regions(&self) -> &[MemoryRegion] {
        &RASPI02W_MEMORY_REGIONS
    }

    fn uart_mmio_region(&self) -> MemoryRegion {
        self.inner.uart_mmio_region()
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

    fn wake_secondary_core(&self, core_id: usize, entry_phys: usize) -> Result<(), SmpError> {
        bcm2837_wake_secondary_core(core_id, entry_phys)
    }

    fn halt(&self) -> ! {
        halt_forever()
    }
}
