#![no_std]
#![no_main]

//! Unix-like EL0 Shell (`shell.elf`) loaded dynamically from FAT32 SD storage.

use libdelta::delta_abi::{IpcMessage, MSG_UART_READ, SVC_UART, SVC_VFS};

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let uart = libdelta::wait_for_service(SVC_UART);
    let vfs = libdelta::wait_for_service(SVC_VFS);

    libdelta::print_via(
        uart,
        "[el0/shell] Loaded /BIN/shell.elf from FAT32 SD card!\n",
    );

    if run_m5_storage_checks(uart, vfs).is_err() {
        libdelta::print_via(uart, "[el0/shell] Storage verification FAILED!\n");
        return 1;
    }

    libdelta::print_via(
        uart,
        "[boot] Milestone M5 (Storage & VFS) ready. Interactive shell active (type 'help').\n",
    );

    interactive_loop(uart, vfs);
    0
}

fn run_m5_storage_checks(uart: usize, vfs: usize) -> Result<(), isize> {
    let mut buf = [0u8; 2048];

    // 1. List root directory (FAT32 + /ext2 + /dev + /proc)
    let dir_len = libdelta::vfs_readdir(vfs, "/", &mut buf)?;
    libdelta::print_via(uart, "[el0/shell] ls /:\n");
    if let Ok(listing) = core::str::from_utf8(&buf[..dir_len]) {
        libdelta::print_via(uart, listing);
    }

    // 2. Read file from Partition 2 (ext2)
    if let Ok(ext_len) = libdelta::vfs_read_file(vfs, "/ext2/welcome.txt", &mut buf)
        && let Ok(text) = core::str::from_utf8(&buf[..ext_len])
    {
        libdelta::print_via(uart, "[el0/shell] cat /ext2/welcome.txt: ");
        libdelta::print_via(uart, text);
        if !text.ends_with('\n') {
            libdelta::print_via(uart, "\n");
        }
    }

    // 3. Write & read back a test file on Partition 1 (FAT32)
    let payload = b"Hollow Star OS+ FAT32 write/read verified!\n";
    let written = libdelta::vfs_write_file(vfs, "/M5_TEST.TXT", payload)?;
    if written != payload.len() {
        return Err(-5);
    }
    let read_back = libdelta::vfs_read_file(vfs, "/M5_TEST.TXT", &mut buf)?;
    if &buf[..read_back] != payload {
        return Err(-5);
    }
    libdelta::print_via(
        uart,
        "[el0/shell] FAT32 write & read-back (/M5_TEST.TXT) OK\n",
    );

    // 4. Launch .hsapp bundle executable from FAT32 (`/Hello.hsapp/exec`)
    let app_pid = libdelta::spawn_from_vfs("hello", "/Hello.hsapp/exec")?;
    let exit_code = libdelta::wait_pid(app_pid)?;
    if exit_code != 0 {
        return Err(-5);
    }

    Ok(())
}

fn interactive_loop(uart: usize, vfs: usize) {
    let mut line = [0u8; 128];
    let mut len = 0usize;
    libdelta::print_via(uart, "delta> ");

    loop {
        let Some(ch) = read_uart_byte(uart) else {
            libdelta::yield_now();
            continue;
        };

        match ch {
            b'\r' | b'\n' => {
                libdelta::print_via(uart, "\n");
                let cmd = core::str::from_utf8(&line[..len]).unwrap_or("").trim();
                if execute_command(uart, vfs, cmd) {
                    return;
                }
                len = 0;
                libdelta::print_via(uart, "delta> ");
            }
            0x08 | 0x7F => {
                if len > 0 {
                    len -= 1;
                    libdelta::print_via(uart, "\x08 \x08");
                }
            }
            b if (0x20..0x7F).contains(&b) && len < line.len() => {
                line[len] = b;
                len += 1;
                if let Ok(s) = core::str::from_utf8(&[b]) {
                    libdelta::print_via(uart, s);
                }
            }
            _ => {}
        }
    }
}

fn execute_command(uart: usize, vfs: usize, cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }
    let (op, arg) = match cmd.split_once(' ') {
        Some((a, b)) => (a, b.trim()),
        None => (cmd, ""),
    };

    let mut buf = [0u8; 2048];
    match op {
        "help" => {
            libdelta::print_via(
                uart,
                "Commands: help, ls [path], cat <path>, write <path> <text>, run <elf>, uname, exit\n",
            );
        }
        "uname" => {
            if let Ok(n) = libdelta::vfs_read_file(vfs, "/proc/version", &mut buf)
                && let Ok(s) = core::str::from_utf8(&buf[..n])
            {
                libdelta::print_via(uart, s);
            }
        }
        "ls" => {
            let path = if arg.is_empty() { "/" } else { arg };
            match libdelta::vfs_readdir(vfs, path, &mut buf) {
                Ok(n) => {
                    if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                        libdelta::print_via(uart, s);
                    }
                }
                Err(_) => libdelta::print_via(uart, "ls: path not found\n"),
            }
        }
        "cat" => match libdelta::vfs_read_file(vfs, arg, &mut buf) {
            Ok(n) => {
                if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                    libdelta::print_via(uart, s);
                    if !s.ends_with('\n') {
                        libdelta::print_via(uart, "\n");
                    }
                }
            }
            Err(_) => libdelta::print_via(uart, "cat: unable to read file\n"),
        },
        "write" => {
            if let Some((path, data)) = arg.split_once(' ') {
                match libdelta::vfs_write_file(vfs, path, data.as_bytes()) {
                    Ok(_) => libdelta::print_via(uart, "write: OK\n"),
                    Err(_) => libdelta::print_via(uart, "write: failed\n"),
                }
            } else {
                libdelta::print_via(uart, "usage: write <path> <text>\n");
            }
        }
        "run" => match libdelta::spawn_from_vfs("app", arg) {
            Ok(pid) => {
                let _ = libdelta::wait_pid(pid);
            }
            Err(_) => libdelta::print_via(uart, "run: failed to launch ELF\n"),
        },
        "exit" => return true,
        _ => libdelta::print_via(uart, "unknown command (type 'help')\n"),
    }
    false
}

fn read_uart_byte(uart: usize) -> Option<u8> {
    let req = IpcMessage::new(MSG_UART_READ);
    let mut reply = IpcMessage::new(0);
    if libdelta::port_call(uart, &req, &mut reply).is_ok() && reply.len > 0 {
        Some(reply.data[0])
    } else {
        None
    }
}
