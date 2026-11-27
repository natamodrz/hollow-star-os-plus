#![no_std]
#![no_main]

//! First EL0 userspace process (`init`) for Hollow Star OS+.
//!
//! Waits for core EL0 system servers (`svc.uart`, `svc.vfs`) and launches the
//! Unix-like shell (`/BIN/shell.elf`) directly from the FAT32 SD card.

use libdelta::delta_abi::{SVC_UART, SVC_VFS};

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let uart_cap = libdelta::wait_for_service(SVC_UART);
    let _vfs_cap = libdelta::wait_for_service(SVC_VFS);

    libdelta::print_via(
        uart_cap,
        "[el0/init] System servers online (svc.uart, svc.sd, svc.vfs); launching /BIN/shell.elf...\n",
    );

    let Ok(shell_pid) = libdelta::spawn_from_vfs("shell", "/BIN/shell.elf") else {
        libdelta::print_via(
            uart_cap,
            "[el0/init] Failed to load /BIN/shell.elf from FAT32!\n",
        );
        return 1;
    };

    libdelta::wait_pid(shell_pid).unwrap_or(1)
}
