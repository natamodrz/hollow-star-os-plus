//! Hardware-independent interrupt controller trait and IRQ source enum.

/// Identified interrupt source reported by a board's interrupt controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqSource {
    /// Per-core system timer tick interrupt.
    Timer,
    /// Peripheral/GPU IRQ number forwarded by the platform controller.
    Peripheral(u32),
    /// No pending interrupt observed (spurious wakeup).
    None,
    /// Unclassified raw interrupt status bits.
    Unknown(u32),
}

/// Platform interrupt controller abstraction (e.g. BCM2837 Local Intc or GIC).
pub trait InterruptController {
    /// Initializes the interrupt controller into a known masked state.
    fn init(&mut self);

    /// Routes and enables the per-core timer interrupt on `core_id`.
    fn enable_timer_irq(&mut self, core_id: usize);

    /// Masks/disables the per-core timer interrupt on `core_id`.
    fn disable_timer_irq(&mut self, core_id: usize);

    /// Enables a shared peripheral/GPU IRQ line `irq`.
    fn enable_peripheral_irq(&mut self, irq: u32);

    /// Masks a shared peripheral/GPU IRQ line `irq`.
    fn disable_peripheral_irq(&mut self, irq: u32);

    /// Returns the lowest-numbered currently pending peripheral IRQ if any.
    fn pending_peripheral_irq(&self) -> Option<u32>;

    /// Returns the highest-priority currently pending interrupt source for `core_id`.
    fn pending_irq(&self, core_id: usize) -> IrqSource;
}
