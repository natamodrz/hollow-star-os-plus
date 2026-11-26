//! Hardware-independent system timer trait for kernel ticks and scheduling.

/// System tick / countdown timer interface.
pub trait Timer {
    /// Returns the hardware counter frequency in Hz (e.g. from `CNTFRQ_EL0`).
    fn frequency_hz(&self) -> u64;

    /// Returns the current monotonic hardware tick count.
    fn counter(&self) -> u64;

    /// Arms the timer to raise an interrupt after `ticks` counter cycles.
    fn arm_after_ticks(&mut self, ticks: u64);

    /// Disarms and masks the timer interrupt.
    fn disarm(&mut self);

    /// Arms the timer to raise an interrupt after `ms` milliseconds.
    fn arm_after_ms(&mut self, ms: u64) {
        let freq = self.frequency_hz();
        let ticks = (freq.saturating_mul(ms) / 1000).max(1);
        self.arm_after_ticks(ticks);
    }
}
