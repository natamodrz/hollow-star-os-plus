#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]

//! Hardware-agnostic traits and device-tree utilities for the Delta Kernel.
//!
//! Board crates implement [`Board`], [`SerialConsole`], [`InterruptController`],
//! and [`Timer`] so that `kernel/` never hardcodes board-specific MMIO addresses.

pub mod board;
pub mod console;
pub mod dtb;
pub mod irq;
pub mod mem;
pub mod timer;

pub use board::{Board, BoardInfo, SmpError};
pub use console::{ConsoleWriter, SerialConsole};
pub use dtb::{Dtb, DtbError, FdtHeader};
pub use irq::{InterruptController, IrqSource};
pub use mem::{MemoryKind, MemoryRegion, PAGE_SHIFT, PAGE_SIZE, PhysAddr, VirtAddr};
pub use timer::Timer;
