#![no_std]
#![no_main]

//! Unix-like EL0 Shell (`/system/bin/shell.elf`) loaded dynamically from DeltaFS System partition.

use libdelta::delta_abi::{
    IpcMessage, KbdPacket, KeyCode, MSG_KBD_ACK, MSG_KBD_READ, MSG_UART_READ, SVC_KBD, SVC_UART,
    SVC_VFS, VfsPartitionStat, unpack_msg,
};

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let uart = libdelta::wait_for_service(SVC_UART);
    let vfs = libdelta::wait_for_service(SVC_VFS);

    libdelta::print_via(
        uart,
        "[el0/shell] Loaded /system/bin/shell.elf from DeltaFS System partition!\n",
    );

    if run_m6_storage_and_ui(uart, vfs).is_err() {
        libdelta::print_via(uart, "[el0/shell] M6 verification FAILED!\n");
        return 1;
    }

    interactive_loop(uart, vfs);
    0
}

fn run_m6_storage_and_ui(uart: usize, vfs: usize) -> Result<(), isize> {
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

    // 5. Launch Modular Desktop Compositor (`/system/bin/compositor.elf`)
    let gui_pid = libdelta::spawn_from_vfs("compositor", "/system/bin/compositor.elf")?;
    let _ = libdelta::wait_pid(gui_pid);
    libdelta::print_via(
        uart,
        "[el0/shell] Returned from Desktop Compositor to interactive shell (type 'gui' or 'help').\n",
    );

    Ok(())
}

fn interactive_loop(uart: usize, vfs: usize) {
    let mut line = [0u8; 128];
    let mut len = 0usize;
    let mut kbd = libdelta::service_lookup(SVC_KBD).ok();
    libdelta::print_via(uart, "delta> ");

    loop {
        let ch = if let Some(b) = read_uart_byte(uart) {
            Some(b)
        } else {
            if kbd.is_none() {
                kbd = libdelta::service_lookup(SVC_KBD).ok();
            }
            kbd.and_then(read_kbd_byte)
        };

        let Some(ch) = ch else {
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
                "Commands: help, gui, launcher, df, ls [path], cat <path>, write <path> <text>, mkdir <path>, rm <path>, run <elf>, uname, exit\n",
            );
        }
        "gui" | "launcher" => {
            if let Ok(pid) = libdelta::spawn_from_vfs("compositor", "/system/bin/compositor.elf") {
                let _ = libdelta::wait_pid(pid);
            }
        }
        "df" => {
            let mut stats: [VfsPartitionStat; 4] = [unsafe { core::mem::zeroed() }; 4];
            let n = libdelta::vfs_statfs(vfs, &mut stats).unwrap_or(0);
            for st in &stats[..n] {
                let m = core::str::from_utf8(&st.mount[..st.mount_len as usize]).unwrap_or("?");
                let f = core::str::from_utf8(&st.fs_type[..st.fs_len as usize]).unwrap_or("?");
                libdelta::print_via(uart, m);
                libdelta::print_via(uart, " -> ");
                libdelta::print_via(uart, f);
                libdelta::print_via(uart, "\n");
            }
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
        "mkdir" => match libdelta::vfs_mkdir(vfs, arg) {
            Ok(()) => libdelta::print_via(uart, "mkdir: OK\n"),
            Err(_) => libdelta::print_via(uart, "mkdir: failed\n"),
        },
        "rm" => match libdelta::vfs_remove(vfs, arg) {
            Ok(()) => libdelta::print_via(uart, "rm: OK\n"),
            Err(_) => libdelta::print_via(uart, "rm: failed\n"),
        },
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

fn read_kbd_byte(kbd: usize) -> Option<u8> {
    let req = IpcMessage::new(MSG_KBD_READ);
    let mut reply = IpcMessage::new(0);
    if libdelta::port_call(kbd, &req, &mut reply).is_ok()
        && reply.tag == MSG_KBD_ACK
        && let Some(pkt) = unpack_msg::<KbdPacket>(&reply)
    {
        match KeyCode::from_u8(pkt.kind) {
            KeyCode::Char => Some(pkt.ch),
            KeyCode::Enter => Some(b'\n'),
            KeyCode::Backspace => Some(0x08),
            _ => None,
        }
    } else {
        None
    }
}
