//! Board selection bridge connecting `kernel/` to `hal-core` implementations.
//!
//! No MMIO addresses or SoC register definitions belong in `kernel/`.

use hal_core::{Board, InterruptController, SerialConsole, Timer};

use crate::mem::HIGHER_HALF_OFFSET;

#[cfg(not(feature = "board-raspi3b"))]
compile_error!("Enable a board feature (e.g. `board-raspi3b`) to build Delta Kernel.");

/// Initializes the active board implementation via `hal-core::Board`.
pub fn init() -> impl Board {
    #[cfg(feature = "board-raspi3b")]
    {
        board_raspi3b::Raspi3bBoard::init(HIGHER_HALF_OFFSET)
    }
}

/// Returns a serial console handle usable inside panic handlers.
pub fn emergency_console() -> impl SerialConsole {
    #[cfg(feature = "board-raspi3b")]
    {
        board_raspi3b::Raspi3bBoard::emergency_console()
    }
}

/// Returns an interrupt controller handle for IRQ dispatch.
pub fn irq_handle() -> impl InterruptController {
    #[cfg(feature = "board-raspi3b")]
    {
        board_raspi3b::Raspi3bBoard::irq_handle()
    }
}

/// Returns a system timer handle for IRQ acknowledgements and ticks.
pub fn timer_handle() -> impl Timer {
    #[cfg(feature = "board-raspi3b")]
    {
        board_raspi3b::Raspi3bBoard::timer_handle()
    }
}
