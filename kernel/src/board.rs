//! Board selection bridge connecting `kernel/` to `hal-core` implementations.
//!
//! No MMIO addresses or SoC register definitions belong in `kernel/`.

use hal_core::{Board, BoardInfo, InterruptController, SerialConsole, Timer};

use crate::mem::HIGHER_HALF_OFFSET;
use crate::sync::IrqSpinLock;

static BOARD_INFO: IrqSpinLock<Option<BoardInfo>> = IrqSpinLock::new(None);
static FB_WINDOW: IrqSpinLock<Option<(usize, usize)>> = IrqSpinLock::new(None);

/// Records static [`BoardInfo`] and optional display framebuffer physical window from boot.
pub fn register_metadata(info: BoardInfo, fb_window: Option<(usize, usize)>) {
    *BOARD_INFO.lock() = Some(info);
    *FB_WINDOW.lock() = fb_window;
}

/// Returns the registered [`BoardInfo`] snapshot if initialized.
pub fn info() -> Option<BoardInfo> {
    *BOARD_INFO.lock()
}

/// Returns the allowed VideoCore/display framebuffer physical window `(base, size)`.
pub fn framebuffer_phys_window() -> Option<(usize, usize)> {
    *FB_WINDOW.lock()
}

#[cfg(all(not(feature = "board-raspi3b"), not(feature = "board-raspi02w")))]
compile_error!(
    "Enable a board feature (`board-raspi3b` or `board-raspi02w`) to build Delta Kernel."
);

/// Initializes the active board implementation via `hal-core::Board`.
pub fn init() -> impl Board {
    #[cfg(feature = "board-raspi3b")]
    {
        board_raspi3b::Raspi3bBoard::init(HIGHER_HALF_OFFSET)
    }
    #[cfg(all(feature = "board-raspi02w", not(feature = "board-raspi3b")))]
    {
        board_raspi02w::Raspi02wBoard::init(HIGHER_HALF_OFFSET)
    }
}

/// Returns a serial console handle usable inside panic handlers.
pub fn emergency_console() -> impl SerialConsole {
    #[cfg(feature = "board-raspi3b")]
    {
        board_raspi3b::Raspi3bBoard::emergency_console()
    }
    #[cfg(all(feature = "board-raspi02w", not(feature = "board-raspi3b")))]
    {
        board_raspi02w::Raspi02wBoard::emergency_console()
    }
}

/// Returns an interrupt controller handle for IRQ dispatch.
pub fn irq_handle() -> impl InterruptController {
    #[cfg(feature = "board-raspi3b")]
    {
        board_raspi3b::Raspi3bBoard::irq_handle()
    }
    #[cfg(all(feature = "board-raspi02w", not(feature = "board-raspi3b")))]
    {
        board_raspi02w::Raspi02wBoard::irq_handle()
    }
}

/// Returns a system timer handle for IRQ acknowledgements and ticks.
pub fn timer_handle() -> impl Timer {
    #[cfg(feature = "board-raspi3b")]
    {
        board_raspi3b::Raspi3bBoard::timer_handle()
    }
    #[cfg(all(feature = "board-raspi02w", not(feature = "board-raspi3b")))]
    {
        board_raspi02w::Raspi02wBoard::timer_handle()
    }
}
