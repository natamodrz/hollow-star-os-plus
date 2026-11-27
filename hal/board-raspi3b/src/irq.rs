//! BCM2837 ARM Local & Peripheral Interrupt Controller driver.

use hal_core::{InterruptController, IrqSource};

use crate::mmio::MmioBlock;
use crate::{ARM_LOCAL_BASE, PERIPHERAL_BASE};

const BCM2837_PERIPH_INTC_BASE: usize = PERIPHERAL_BASE + 0x0000_B200;
const IRQ_PENDING_1: usize = 0x04;
const IRQ_PENDING_2: usize = 0x08;
const ENABLE_IRQS_1: usize = 0x10;
const ENABLE_IRQS_2: usize = 0x14;
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

    fn enable_peripheral_irq(&mut self, irq: u32) {
        if irq < 32 {
            self.periph.write(ENABLE_IRQS_1, 1u32 << irq);
        } else if irq < 64 {
            self.periph.write(ENABLE_IRQS_2, 1u32 << (irq - 32));
        }
    }

    fn disable_peripheral_irq(&mut self, irq: u32) {
        if irq < 32 {
            self.periph.write(DISABLE_IRQS_1, 1u32 << irq);
        } else if irq < 64 {
            self.periph.write(DISABLE_IRQS_2, 1u32 << (irq - 32));
        }
    }

    fn pending_peripheral_irq(&self) -> Option<u32> {
        let p1 = self.periph.read(IRQ_PENDING_1);
        if p1 != 0 {
            return Some(p1.trailing_zeros());
        }
        let p2 = self.periph.read(IRQ_PENDING_2);
        if p2 != 0 {
            return Some(32 + p2.trailing_zeros());
        }
        None
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
            if let Some(gpu_irq) = self.pending_peripheral_irq() {
                IrqSource::Peripheral(gpu_irq)
            } else {
                IrqSource::Unknown(source)
            }
        } else {
            IrqSource::Unknown(source)
        }
    }
}
