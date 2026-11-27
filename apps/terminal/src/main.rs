#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Graphical Terminal Emulator (`/system/apps/Terminal.hsapp/exec`).
//!
//! Uses crisp `8x14` monospace rendering, command history, and full VFS/DeltaFS inspection.

use libdelta_ui::calibri::MONO_WIDTH;
use libdelta_ui::delta_abi::{KeyCode, SVC_VFS, SysCaps, VfsPartitionStat};
use libdelta_ui::gfx::{
    ACCENT, ACCENT_AMBER, ACCENT_YELLOW, APP_W, BG, BORDER, CYAN, MUTED, OK_GREEN, PANEL,
    PANEL_ALT, TEXT, WARN_RED,
};
use libdelta_ui::{ArcSharpStyle, BufWriter, DesktopClient, OsConfig, tr};

const MAX_COLS: usize = 74;
const VIEW_ROWS: usize = 20;
const SCROLL_LINES: usize = 48;
const HIST_LEN: usize = 8;

struct TermLine {
    text: [u8; MAX_COLS],
    len: usize,
    color: u32,
}

impl TermLine {
    const fn empty() -> Self {
        Self {
            text: [0; MAX_COLS],
            len: 0,
            color: TEXT,
        }
    }
}

struct Terminal {
    lines: [TermLine; SCROLL_LINES],
    head: usize,
    count: usize,
    input: [u8; 64],
    input_len: usize,
    history: [[u8; 64]; HIST_LEN],
    hist_lens: [usize; HIST_LEN],
    hist_count: usize,
    hist_pos: Option<usize>,
    cursor_vis: bool,
    blink_ticks: u32,
}

impl Terminal {
    const fn new() -> Self {
        Self {
            lines: [const { TermLine::empty() }; SCROLL_LINES],
            head: 0,
            count: 0,
            input: [0; 64],
            input_len: 0,
            history: [[0; 64]; HIST_LEN],
            hist_lens: [0; HIST_LEN],
            hist_count: 0,
            hist_pos: None,
            cursor_vis: true,
            blink_ticks: 0,
        }
    }

    fn clear_lines(&mut self) {
        self.head = 0;
        self.count = 0;
    }

    fn push_raw(&mut self, s: &str, color: u32) {
        let idx = if self.count < SCROLL_LINES {
            let i = (self.head + self.count) % SCROLL_LINES;
            self.count += 1;
            i
        } else {
            let i = self.head;
            self.head = (self.head + 1) % SCROLL_LINES;
            i
        };
        let n = s.len().min(MAX_COLS);
        self.lines[idx].text[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.lines[idx].len = n;
        self.lines[idx].color = color;
    }

    fn println_wrapped(&mut self, text: &str, color: u32) {
        for raw_line in text.split('\n') {
            if raw_line.is_empty() {
                continue;
            }
            let bytes = raw_line.as_bytes();
            let mut off = 0usize;
            while off < bytes.len() {
                let end = (off + MAX_COLS).min(bytes.len());
                if let Ok(chunk) = core::str::from_utf8(&bytes[off..end]) {
                    self.push_raw(chunk, color);
                }
                off = end;
            }
        }
    }

    fn save_history(&mut self, cmd: &[u8]) {
        if cmd.is_empty() {
            return;
        }
        let n = cmd.len().min(64);
        for i in (1..HIST_LEN).rev() {
            self.history[i] = self.history[i - 1];
            self.hist_lens[i] = self.hist_lens[i - 1];
        }
        self.history[0][..n].copy_from_slice(&cmd[..n]);
        self.hist_lens[0] = n;
        self.hist_count = (self.hist_count + 1).min(HIST_LEN);
        self.hist_pos = None;
    }
}

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let Ok(mut desktop) = DesktopClient::connect() else {
        return 1;
    };
    let vfs = libdelta::wait_for_service(SVC_VFS);
    let caps = libdelta::sys_caps().unwrap_or_else(|_| SysCaps::empty());

    let mut cfg = OsConfig::defaults();
    cfg.load_from_vfs(vfs);

    let mut term = Terminal::new();
    term.println_wrapped(
        "Hollow Star OS+ Terminal Emulator v0.6.0-rc.2 (Monospace 8x14)",
        ACCENT,
    );
    term.println_wrapped(
        "Type 'help' for VFS/DeltaFS/system commands, or press [Esc] for Launcher.",
        MUTED,
    );

    let mut dirty = true;
    loop {
        if dirty {
            render_terminal(&mut desktop, &cfg, &term);
            desktop.present();
            dirty = false;
        }

        let Some((key, ch)) = desktop.poll_key() else {
            term.blink_ticks = term.blink_ticks.wrapping_add(1);
            if term.blink_ticks >= 30 {
                term.blink_ticks = 0;
                term.cursor_vis = !term.cursor_vis;
                dirty = true;
            }
            libdelta_ui::sleep_ms(16);
            continue;
        };

        term.cursor_vis = true;
        term.blink_ticks = 0;

        match key {
            KeyCode::Esc => {
                desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
                return 0;
            }
            KeyCode::CtrlC => {
                term.input_len = 0;
                term.println_wrapped("^C", WARN_RED);
                dirty = true;
            }
            KeyCode::Backspace => {
                if term.input_len > 0 {
                    term.input_len -= 1;
                    dirty = true;
                }
            }
            KeyCode::Up => {
                if term.hist_count > 0 {
                    let next = match term.hist_pos {
                        None => 0,
                        Some(p) if p + 1 < term.hist_count => p + 1,
                        Some(p) => p,
                    };
                    term.hist_pos = Some(next);
                    let len = term.hist_lens[next];
                    term.input[..len].copy_from_slice(&term.history[next][..len]);
                    term.input_len = len;
                    dirty = true;
                }
            }
            KeyCode::Down => {
                if let Some(p) = term.hist_pos {
                    if p == 0 {
                        term.hist_pos = None;
                        term.input_len = 0;
                    } else {
                        let next = p - 1;
                        term.hist_pos = Some(next);
                        let len = term.hist_lens[next];
                        term.input[..len].copy_from_slice(&term.history[next][..len]);
                        term.input_len = len;
                    }
                    dirty = true;
                }
            }
            KeyCode::Enter => {
                let mut cmd_copy = [0u8; 64];
                let cmd_len = term.input_len;
                cmd_copy[..cmd_len].copy_from_slice(&term.input[..cmd_len]);
                term.input_len = 0;

                let cmd = core::str::from_utf8(&cmd_copy[..cmd_len])
                    .unwrap_or("")
                    .trim();
                let mut echo = [0u8; 80];
                let mut ew = BufWriter::new(&mut echo);
                ew.str("delta> ");
                ew.str(cmd);
                term.println_wrapped(ew.as_str(), CYAN);

                if !cmd.is_empty() {
                    term.save_history(cmd.as_bytes());
                    if let Some(target_elf) = exec_term_cmd(&mut term, vfs, &caps, cmd) {
                        if target_elf.is_empty() {
                            desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
                        } else {
                            desktop.request_launch(target_elf, 0);
                        }
                        return 0;
                    }
                }
                dirty = true;
            }
            KeyCode::Char if (0x20..=0x7E).contains(&ch) && term.input_len < term.input.len() => {
                term.input[term.input_len] = ch;
                term.input_len += 1;
                dirty = true;
            }
            _ => {}
        }
    }
}

/// Executes a terminal command; returns `Some(path)` if the caller should launch another `.hsapp`
/// (or empty string to return to `Launcher.hsapp`).
fn exec_term_cmd<'a>(
    term: &mut Terminal,
    vfs: usize,
    caps: &SysCaps,
    cmd: &'a str,
) -> Option<&'a str> {
    let (op, arg) = match cmd.split_once(' ') {
        Some((a, b)) => (a, b.trim()),
        None => (cmd, ""),
    };

    let mut io_buf = [0u8; 2048];
    match op {
        "help" | "?" => {
            term.println_wrapped(
                "Commands: help, clear, ls [dir], cat <file>, write <file> <txt>",
                TEXT,
            );
            term.println_wrapped(
                "  mkdir <dir>, rm <file>, df, sysinfo, dkexstat, dylibs, uname",
                TEXT,
            );
            term.println_wrapped("  expand, run <elf>, exit", TEXT);
        }
        "dkexstat" => match libdelta::vfs_readdir(vfs, "/system/Library/Extensions", &mut io_buf) {
            Ok(n) => {
                if let Ok(listing) = core::str::from_utf8(&io_buf[..n]) {
                    term.println_wrapped(
                        "Loaded .dkex Extensions (/system/Library/Extensions):",
                        ACCENT,
                    );
                    for line in listing.lines() {
                        let name = line
                            .trim()
                            .trim_start_matches("[DIR]")
                            .trim_start_matches("[APP]")
                            .trim()
                            .trim_end_matches('/');
                        if name.ends_with(".dkex") {
                            let mut pbuf = [0u8; 96];
                            let mut pw = BufWriter::new(&mut pbuf);
                            pw.str("/system/Library/Extensions/");
                            pw.str(name);
                            pw.str("/Info.plist");
                            let mut plist_buf = [0u8; 512];
                            let mut out = [0u8; 74];
                            let mut ow = BufWriter::new(&mut out);
                            ow.str(name);
                            if let Ok(pn) =
                                libdelta::vfs_read_file(vfs, pw.as_str(), &mut plist_buf)
                                && let Ok(pstr) = core::str::from_utf8(&plist_buf[..pn])
                            {
                                if let Some(svc) =
                                    libdelta::dyld::parse_plist_string(pstr, "DeltaService")
                                {
                                    ow.str(" -> ");
                                    ow.str(svc);
                                }
                                if let Some(bid) =
                                    libdelta::dyld::parse_plist_string(pstr, "CFBundleIdentifier")
                                {
                                    ow.str(" (");
                                    ow.str(bid);
                                    ow.str(")");
                                }
                            }
                            term.println_wrapped(ow.as_str(), CYAN);
                        }
                    }
                }
            }
            Err(_) => {
                term.println_wrapped("dkexstat: /system/Library/Extensions missing", WARN_RED)
            }
        },
        "dylibs" => {
            if let Ok(kit) = libdelta::dyld::Dylib::open(vfs, "/system/lib/libdelta_kit.dylib") {
                // SAFETY: `delta_kit_abi_version` is `extern "C" fn() -> u32`.
                let abi = unsafe { kit.call_u32("delta_kit_abi_version") }.unwrap_or(0);
                let mut line = [0u8; 74];
                let mut w = BufWriter::new(&mut line);
                w.str("libdelta_kit.dylib: syms=");
                w.u32(kit.symbol_count() as u32);
                w.str(", bytes=");
                w.u32(kit.mapped_bytes() as u32);
                w.str(", abi=0x");
                let hex = b"0123456789ABCDEF";
                for sh in (0..8).rev() {
                    let b = [hex[((abi >> (sh * 4)) & 0xF) as usize]];
                    if let Ok(s) = core::str::from_utf8(&b) {
                        w.str(s);
                    }
                }
                term.println_wrapped(w.as_str(), OK_GREEN);
            } else {
                term.println_wrapped(
                    "dylibs: failed to map /system/lib/libdelta_kit.dylib",
                    WARN_RED,
                );
            }
        }
        "clear" | "cls" => {
            term.clear_lines();
        }
        "ls" => {
            let path = if arg.is_empty() { "/" } else { arg };
            match libdelta::vfs_readdir(vfs, path, &mut io_buf) {
                Ok(n) => {
                    if let Ok(s) = core::str::from_utf8(&io_buf[..n]) {
                        if s.is_empty() {
                            term.println_wrapped("(empty)", MUTED);
                        } else {
                            term.println_wrapped(s, TEXT);
                        }
                    }
                }
                Err(_) => term.println_wrapped("ls: directory not found", WARN_RED),
            }
        }
        "cat" => {
            if arg.is_empty() {
                term.println_wrapped("usage: cat <path>", WARN_RED);
            } else {
                match libdelta::vfs_read_file(vfs, arg, &mut io_buf) {
                    Ok(n) => {
                        if let Ok(s) = core::str::from_utf8(&io_buf[..n]) {
                            term.println_wrapped(s, TEXT);
                        } else {
                            term.println_wrapped("(binary file)", MUTED);
                        }
                    }
                    Err(_) => term.println_wrapped("cat: unable to read file", WARN_RED),
                }
            }
        }
        "write" => {
            if let Some((path, data)) = arg.split_once(' ') {
                match libdelta::vfs_write_file(vfs, path.trim(), data.as_bytes()) {
                    Ok(_) => term.println_wrapped("write: OK (DeltaFS committed)", OK_GREEN),
                    Err(_) => term.println_wrapped("write: failed", WARN_RED),
                }
            } else {
                term.println_wrapped("usage: write <path> <text>", WARN_RED);
            }
        }
        "mkdir" => match libdelta::vfs_mkdir(vfs, arg) {
            Ok(()) => term.println_wrapped("mkdir: OK", OK_GREEN),
            Err(_) => term.println_wrapped("mkdir: failed", WARN_RED),
        },
        "rm" => match libdelta::vfs_remove(vfs, arg) {
            Ok(()) => term.println_wrapped("rm: OK", OK_GREEN),
            Err(_) => term.println_wrapped("rm: failed", WARN_RED),
        },
        "df" => {
            // SAFETY: `VfsPartitionStat` is POD zero-initializable.
            let mut stats: [VfsPartitionStat; 4] = [unsafe { core::mem::zeroed() }; 4];
            let n = libdelta::vfs_statfs(vfs, &mut stats).unwrap_or(0);
            for st in &stats[..n] {
                let mut line = [0u8; 74];
                let mut w = BufWriter::new(&mut line);
                let m = core::str::from_utf8(&st.mount[..st.mount_len as usize]).unwrap_or("?");
                let f = core::str::from_utf8(&st.fs_type[..st.fs_len as usize]).unwrap_or("?");
                w.str(m);
                w.str(" -> ");
                w.str(f);
                w.str("  (");
                w.u32((st.total_bytes.saturating_sub(st.free_bytes) / 1024) as u32);
                w.str("/");
                w.u32((st.total_bytes / 1024) as u32);
                w.str(" KiB)");
                term.println_wrapped(w.as_str(), TEXT);
            }
        }
        "uname" => {
            if let Ok(n) = libdelta::vfs_read_file(vfs, "/proc/version", &mut io_buf)
                && let Ok(s) = core::str::from_utf8(&io_buf[..n])
            {
                term.println_wrapped(s, TEXT);
            } else {
                term.println_wrapped("Delta Kernel AArch64 v0.6.0-rc.2", TEXT);
            }
        }
        "sysinfo" => {
            let mut line = [0u8; 74];
            let mut w = BufWriter::new(&mut line);
            w.str("Board: ");
            w.str(caps.board_name_str());
            w.str(" | RAM ");
            w.u32((caps.ram_free_bytes / (1024 * 1024)) as u32);
            w.str("/");
            w.u32((caps.ram_total_bytes / (1024 * 1024)) as u32);
            w.str(" MiB");
            term.println_wrapped(w.as_str(), TEXT);

            let mut up = [0u8; 48];
            let mut uw = BufWriter::new(&mut up);
            uw.str("Uptime: ");
            uw.u32((caps.uptime_ms / 1000) as u32);
            uw.str(" s | Wi-Fi: ");
            uw.str(if caps.has_wifi != 0 { "Yes" } else { "No" });
            term.println_wrapped(uw.as_str(), MUTED);
        }
        "expand" => match libdelta::vfs_expand_user(vfs, 0) {
            Ok(bytes) => {
                let mut line = [0u8; 74];
                let mut w = BufWriter::new(&mut line);
                w.str("/user DeltaFS expanded to ");
                w.u32((bytes / 1024) as u32);
                w.str(" KiB");
                term.println_wrapped(w.as_str(), OK_GREEN);
            }
            Err(_) => term.println_wrapped("expand: failed or already at card end", WARN_RED),
        },
        "run" => {
            if arg.is_empty() {
                term.println_wrapped("usage: run </system/apps/.../exec>", WARN_RED);
            } else {
                return Some(arg);
            }
        }
        "exit" | "quit" => return Some(""),
        _ => {
            term.println_wrapped("Unknown command. Type 'help'.", WARN_RED);
        }
    }
    None
}

fn render_terminal(desktop: &mut DesktopClient, cfg: &OsConfig, term: &Terminal) {
    let lang = cfg.lang;
    let c = &mut desktop.canvas;
    c.clear(BG);

    // Header chrome (Calibri proportional font)
    c.draw_arc_box(
        12,
        8,
        (APP_W - 24) as i32,
        34,
        ArcSharpStyle::cyberdeck_card(),
        PANEL,
        ACCENT_YELLOW,
        ACCENT_AMBER,
    );
    c.draw_text(
        24,
        18,
        tr(
            lang,
            "Terminal Emulator (Monospace 8x14)",
            "Emulator Terminala (Monospace 8x14)",
        ),
        ACCENT_YELLOW,
    );
    c.draw_text_right(
        (APP_W - 24) as i32,
        18,
        tr(lang, "[Esc / exit] Launcher", "[Esc / exit] Launcher"),
        MUTED,
    );

    // Console body (sharp-cornered inner terminal frame matching #131926 theme)
    c.draw_arc_box(
        12,
        48,
        (APP_W - 24) as i32,
        368,
        ArcSharpStyle::sharp(),
        0x000F_1420,
        BORDER,
        BORDER,
    );

    let visible = term.count.min(VIEW_ROWS);
    let skip = term.count.saturating_sub(VIEW_ROWS);
    let mut y = 58;
    for i in 0..visible {
        let idx = (term.head + skip + i) % SCROLL_LINES;
        let ln = &term.lines[idx];
        if let Ok(s) = core::str::from_utf8(&ln.text[..ln.len]) {
            c.draw_mono_text(22, y, s, ln.color);
        }
        y += 17;
    }

    // Active prompt line at bottom of terminal viewport
    c.fill_rect(14, 386, (APP_W - 28) as i32, 24, PANEL_ALT);
    c.draw_mono_text(22, 391, "delta> ", ACCENT_YELLOW);
    let input_str = core::str::from_utf8(&term.input[..term.input_len]).unwrap_or("");
    c.draw_mono_text(22 + 7 * MONO_WIDTH, 391, input_str, TEXT);

    if term.cursor_vis {
        let cur_x = 22 + (7 + term.input_len as i32) * MONO_WIDTH;
        c.fill_rect(cur_x.min(610), 392, MONO_WIDTH - 1, 13, ACCENT_AMBER);
    }

    c.draw_text(
        18,
        426,
        tr(
            lang,
            "[Up/Down] History   [Ctrl+C] Clear line   [Enter] Execute   [F1] Help",
            "[Gora/Dol] Historia   [Ctrl+C] Wyczysc   [Enter] Wykonaj   [F1] Pomoc",
        ),
        MUTED,
    );
}
