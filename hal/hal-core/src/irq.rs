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

    /// Returns the highest-priority currently pending interrupt source for `core_id`.
    fn pending_irq(&self, core_id: usize) -> IrqSource;
}
