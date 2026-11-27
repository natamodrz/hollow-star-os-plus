#![no_std]
#![no_main]

//! Boot Session Supervisor (`/system/bin/shell.elf`) loaded from DeltaFS System partition.
//!
//! Verifies 3-partition storage (`/boot`, `/system`, `/user`), runs DeltaFS WAL & `.hsapp`
//! sanity checks, and keeps the graphical Cyberdeck Desktop (`/system/bin/compositor.elf`)
//! active without exposing a raw interactive UART console.

use libdelta::delta_abi::{SVC_UART, SVC_VFS};

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let uart = libdelta::wait_for_service(SVC_UART);
    let vfs = libdelta::wait_for_service(SVC_VFS);

    libdelta::print_via(
        uart,
        "[el0/shell] Loaded /system/bin/shell.elf from DeltaFS System partition!\n",
    );

    if run_storage_and_compositor(uart, vfs).is_err() {
        libdelta::print_via(uart, "[el0/shell] Storage verification FAILED!\n");
        return 1;
    }

    0
}

fn run_storage_and_compositor(uart: usize, vfs: usize) -> Result<(), isize> {
    let mut buf = [0u8; 2048];

    // 1. Verify separated 3-partition root (/boot, /system, /user, /dev, /proc)
    let dir_len = libdelta::vfs_readdir(vfs, "/", &mut buf)?;
    libdelta::print_via(uart, "[el0/shell] ls / (boot/system/user):\n");
    if let Ok(listing) = core::str::from_utf8(&buf[..dir_len]) {
        libdelta::print_via(uart, listing);
    }

    // 2. Read Markdown note from /user DeltaFS partition
    if let Ok(n) = libdelta::vfs_read_file(vfs, "/user/notes/welcome.md", &mut buf)
        && let Ok(text) = core::str::from_utf8(&buf[..n])
        && let Some(first_line) = text.lines().next()
    {
        libdelta::print_via(uart, "[el0/shell] cat /user/notes/welcome.md: ");
        libdelta::print_via(uart, first_line);
        libdelta::print_via(uart, "\n");
    }

    // 3. Write & read back test file via DeltaFS write-ahead journal on /user
    let payload = b"DeltaFS WAL journal write/read verified!\n";
    let written = libdelta::vfs_write_file(vfs, "/user/M6_TEST.TXT", payload)?;
    if written != payload.len() {
        return Err(-5);
    }
    let read_back = libdelta::vfs_read_file(vfs, "/user/M6_TEST.TXT", &mut buf)?;
    if &buf[..read_back] != payload {
        return Err(-5);
    }
    libdelta::print_via(
        uart,
        "[el0/shell] DeltaFS WAL write & read-back (/user/M6_TEST.TXT) OK\n",
    );

    // 4. Launch sample .hsapp bundle from /system
    let hello_pid = libdelta::spawn_from_vfs("hello", "/system/apps/Hello.hsapp/exec")?;
    if libdelta::wait_pid(hello_pid)? != 0 {
        return Err(-5);
    }

    // 5. Launch Modular Cyberdeck Compositor (`/system/bin/compositor.elf`) persistently
    loop {
        let gui_pid = libdelta::spawn_from_vfs("compositor", "/system/bin/compositor.elf")?;
        let _ = libdelta::wait_pid(gui_pid);
        libdelta::yield_now();
    }
}
