//! BCM2837 ARM Local & Peripheral Interrupt Controller driver.

use hal_core::{InterruptController, IrqSource};

use crate::mmio::MmioBlock;
use crate::{ARM_LOCAL_BASE, PERIPHERAL_BASE};

const BCM2837_PERIPH_INTC_BASE: usize = PERIPHERAL_BASE + 0x0000_B200;
const DISABLE_IRQS_1: usize = 0x1C;
const DISABLE_IRQS_2: usize = 0x20;
const DISABLE_BASIC_IRQS: usize = 0x24;

const CORE_TIMER_IRQCNTL_BASE: usize = 0x40;
const CORE_MAILBOX_IRQCNTL_BASE: usize = 0x50;
const CORE_IRQ_SOURCE_BASE: usize = 0x60;

const TIMER_CNTPNSIRQ_BIT: u32 = 1 << 1;
const SOURCE_GPU_IRQ_BIT: u32 = 1 << 8;

/// Interrupt controller for BCM2837 (ARM Local Peripherals + GPU Intc).
pub struct Bcm2837IrqController {
    local: MmioBlock,
    periph: MmioBlock,
}

impl Bcm2837IrqController {
    /// Creates an interrupt controller handle bound to BCM2837 MMIO windows.
    pub const fn new() -> Self {
        // SAFETY: `ARM_LOCAL_BASE` (`0x4000_0000`) and `BCM2837_PERIPH_INTC_BASE`
        // (`0x3F00_B200`) are fixed BCM2837 physical MMIO bases mapped as Device memory.
        unsafe {
            Self {
                local: MmioBlock::new(ARM_LOCAL_BASE),
                periph: MmioBlock::new(BCM2837_PERIPH_INTC_BASE),
            }
        }
    }
}

impl Default for Bcm2837IrqController {
    fn default() -> Self {
        Self::new()
    }
}

impl InterruptController for Bcm2837IrqController {
    fn init(&mut self) {
        // Mask all GPU/peripheral IRQs until EL0/kernel drivers explicitly enable them.
        self.periph.write(DISABLE_IRQS_1, u32::MAX);
        self.periph.write(DISABLE_IRQS_2, u32::MAX);
        self.periph.write(DISABLE_BASIC_IRQS, u32::MAX);

        // Mask local timer & mailbox IRQs across all 4 Cortex-A53 cores.
        for core in 0..4 {
            self.local.write(CORE_TIMER_IRQCNTL_BASE + core * 4, 0);
            self.local.write(CORE_MAILBOX_IRQCNTL_BASE + core * 4, 0);
        }
    }

    fn enable_timer_irq(&mut self, core_id: usize) {
        if core_id >= 4 {
            return;
        }
        let offset = CORE_TIMER_IRQCNTL_BASE + core_id * 4;
        let current = self.local.read(offset);
        self.local.write(offset, current | TIMER_CNTPNSIRQ_BIT);
    }

    fn disable_timer_irq(&mut self, core_id: usize) {
        if core_id >= 4 {
            return;
        }
        let offset = CORE_TIMER_IRQCNTL_BASE + core_id * 4;
        let current = self.local.read(offset);
        self.local.write(offset, current & !TIMER_CNTPNSIRQ_BIT);
    }

    fn pending_irq(&self, core_id: usize) -> IrqSource {
        if core_id >= 4 {
            return IrqSource::None;
        }
        let source = self.local.read(CORE_IRQ_SOURCE_BASE + core_id * 4);
        if source == 0 {
            IrqSource::None
        } else if (source & TIMER_CNTPNSIRQ_BIT) != 0 {
            IrqSource::Timer
        } else if (source & SOURCE_GPU_IRQ_BIT) != 0 {
            IrqSource::Peripheral(source)
        } else {
            IrqSource::Unknown(source)
        }
    }
}
