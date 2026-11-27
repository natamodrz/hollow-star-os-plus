#![no_std]
#![no_main]

//! First EL0 userspace process (`init`) for Hollow Star OS+.
//!
//! Waits for core EL0 system servers (`svc.uart`, `svc.vfs`, `svc.fb`, `svc.audio`) and
//! launches `/system/bin/shell.elf` (which verifies storage and starts `HollowStar.hsapp`).

use libdelta::delta_abi::{SVC_AUDIO, SVC_FB, SVC_UART, SVC_VFS};

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let uart_cap = libdelta::wait_for_service(SVC_UART);
    let _vfs_cap = libdelta::wait_for_service(SVC_VFS);
    let _fb_cap = libdelta::wait_for_service(SVC_FB);
    let _audio_cap = libdelta::wait_for_service(SVC_AUDIO);

    libdelta::print_via(
        uart_cap,
        "[el0/init] System servers online (svc.uart, svc.sd, svc.vfs, svc.fb, svc.audio); launching /system/bin/shell.elf...\n",
    );

    let Ok(shell_pid) = libdelta::spawn_from_vfs("shell", "/system/bin/shell.elf") else {
        libdelta::print_via(
            uart_cap,
            "[el0/init] Failed to load /system/bin/shell.elf from DeltaFS!\n",
        );
        return 1;
    };

    libdelta::wait_pid(shell_pid).unwrap_or(1)
}
