//! Board-level platform abstraction implemented by board support crates.

use crate::console::SerialConsole;
use crate::irq::InterruptController;
use crate::mem::MemoryRegion;
use crate::timer::Timer;

/// Errors returned when waking secondary SMP cores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmpError {
    /// Core index is zero (primary) or exceeds board core count.
    InvalidCore,
    /// Board does not support secondary core wakeup.
    Unsupported,
}

/// Static metadata describing the active target board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardInfo {
    /// Human-readable board name (e.g. `"Raspberry Pi 3B"`).
    pub name: &'static str,
    /// System-on-Chip identifier (e.g. `"BCM2837"`).
    pub soc: &'static str,
    /// Active early serial console controller name.
    pub uart_mode: &'static str,
    /// Number of CPU cores on the board.
    pub cpu_count: usize,
    /// Physical base address of main system RAM.
    pub ram_base: usize,
    /// Default physical RAM size in bytes when no DTB overrides it.
    pub ram_size_bytes: usize,
}

/// Platform abstraction decoupling `kernel/` from board-specific hardware details.
pub trait Board {
    /// Board-specific serial console type.
    type Console: SerialConsole;
    /// Board-specific interrupt controller type.
    type Irq: InterruptController;
    /// Board-specific tick timer type.
    type Timer: Timer;

    /// Returns descriptive metadata for this board.
    fn info(&self) -> BoardInfo;

    /// Returns mutable access to the active early serial console.
    fn console(&mut self) -> &mut Self::Console;

    /// Returns mutable access to the interrupt controller.
    fn irq_controller(&mut self) -> &mut Self::Irq;

    /// Returns mutable access to the system timer.
    fn timer(&mut self) -> &mut Self::Timer;

    /// Returns static physical memory & MMIO region descriptors for this board.
    fn memory_regions(&self) -> &[MemoryRegion];

    /// Wakes secondary CPU core `core_id` (`1..cpu_count`) to begin executing at
    /// physical address `entry_phys`.
    fn wake_secondary_core(&self, core_id: usize, entry_phys: usize) -> Result<(), SmpError>;

    /// Permanently halts the calling CPU core in a low-power state.
    fn halt(&self) -> !;
}
