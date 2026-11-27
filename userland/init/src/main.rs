#![no_std]
#![no_main]

//! First EL0 userspace process (`init`, Mach `launchd` role) for Hollow Star OS+.
//!
//! Waits for core EL0 system extensions (`svc.uart`, `svc.vfs`, `svc.fb`, `svc.audio`, `svc.kbd`),
//! loads `/system/Library/Extensions/DeltaDiag.dkex/exec` from DeltaFS, and starts
//! `/system/bin/shell.elf` (which verifies storage and launches `/system/bin/compositor.elf`).

use libdelta::delta_abi::{SVC_AUDIO, SVC_FB, SVC_KBD, SVC_UART, SVC_VFS};

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let uart_cap = libdelta::wait_for_service(SVC_UART);
    let _vfs_cap = libdelta::wait_for_service(SVC_VFS);
    let _fb_cap = libdelta::wait_for_service(SVC_FB);
    let _audio_cap = libdelta::wait_for_service(SVC_AUDIO);
    let _kbd_cap = libdelta::wait_for_service(SVC_KBD);

    libdelta::print_via(
        uart_cap,
        "[el0/init] Core extensions online; loading /system/Library/Extensions/DeltaDiag.dkex...\n",
    );

    if let Ok(dkex_pid) = libdelta::spawn_from_vfs(
        "DeltaDiag.dkex",
        "/system/Library/Extensions/DeltaDiag.dkex/exec",
    ) {
        let _ = libdelta::wait_pid(dkex_pid);
    }

    let Ok(shell_pid) = libdelta::spawn_from_vfs("shell", "/system/bin/shell.elf") else {
        libdelta::print_via(
            uart_cap,
            "[el0/init] Failed to load /system/bin/shell.elf from DeltaFS!\n",
        );
        return 1;
    };

    libdelta::wait_pid(shell_pid).unwrap_or(1)
}
