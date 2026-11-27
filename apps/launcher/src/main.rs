#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Standalone `.hsapp` Grid Launcher (`/system/apps/Launcher.hsapp/exec`).

use libdelta_ui::delta_abi::{KeyCode, SVC_VFS, SysCaps};
use libdelta_ui::gfx::{ACCENT, APP_W, BG, BORDER, CYAN, MUTED, OK_GREEN, PANEL, TEXT};
use libdelta_ui::{BufWriter, DesktopClient, IconKind, OsConfig, draw_icon_2x, tr};

struct AppEntry {
    key_hint: &'static str,
    en: &'static str,
    pl: &'static str,
    icon: IconKind,
    exec_path: &'static str,
    arg: u64,
}

const APPS: [AppEntry; 11] = [
    AppEntry {
        key_hint: "1",
        en: "Terminal",
        pl: "Terminal",
        icon: IconKind::Terminal,
        exec_path: "/system/apps/Terminal.hsapp/exec",
        arg: 0,
    },
    AppEntry {
        key_hint: "2",
        en: "Sys Info",
        pl: "System",
        icon: IconKind::SysInfo,
        exec_path: "/system/apps/Apps.hsapp/exec",
        arg: 0,
    },
    AppEntry {
        key_hint: "3",
        en: "Notes .md",
        pl: "Notatki .md",
        icon: IconKind::Notes,
        exec_path: "/system/apps/Apps.hsapp/exec",
        arg: 1,
    },
    AppEntry {
        key_hint: "4",
        en: "Files",
        pl: "Pliki",
        icon: IconKind::Files,
        exec_path: "/system/apps/Apps.hsapp/exec",
        arg: 2,
    },
    AppEntry {
        key_hint: "5",
        en: "Media RT",
        pl: "Audio RT",
        icon: IconKind::Media,
        exec_path: "/system/apps/Apps.hsapp/exec",
        arg: 3,
    },
    AppEntry {
        key_hint: "6",
        en: "Calc",
        pl: "Kalkulator",
        icon: IconKind::Calc,
        exec_path: "/system/apps/Apps.hsapp/exec",
        arg: 4,
    },
    AppEntry {
        key_hint: "7",
        en: "Games",
        pl: "Gry",
        icon: IconKind::Games,
        exec_path: "/system/apps/Apps.hsapp/exec",
        arg: 5,
    },
    AppEntry {
        key_hint: "8",
        en: "Settings",
        pl: "Ustawienia",
        icon: IconKind::Settings,
        exec_path: "/system/apps/Apps.hsapp/exec",
        arg: 6,
    },
    AppEntry {
        key_hint: "9",
        en: "Hello .hsapp",
        pl: "Hello .hsapp",
        icon: IconKind::Hsapp,
        exec_path: "/system/apps/Hello.hsapp/exec",
        arg: 0,
    },
    AppEntry {
        key_hint: "0",
        en: "1st Setup",
        pl: "Konfiguracja",
        icon: IconKind::Setup,
        exec_path: "/system/apps/Setup.hsapp/exec",
        arg: 0,
    },
    AppEntry {
        key_hint: "X",
        en: "UART Shell",
        pl: "Konsola UART",
        icon: IconKind::Shell,
        exec_path: "",
        arg: 0,
    },
];

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let Ok(mut desktop) = DesktopClient::connect() else {
        return 1;
    };
    let vfs = libdelta::wait_for_service(SVC_VFS);
    let caps = libdelta::sys_caps().unwrap_or_else(|_| SysCaps::empty());

    let mut cfg = OsConfig::defaults();
    cfg.load_from_vfs(vfs);

    let bundle_count = count_hsapps(vfs, "/system/apps") + count_hsapps(vfs, "/user/apps");
    let mut selected = 0usize;
    let mut dirty = true;

    loop {
        if dirty {
            render_launcher(&mut desktop, &cfg, &caps, selected, bundle_count);
            desktop.present();
            dirty = false;
        }

        let Some((key, ch)) = desktop.poll_key() else {
            libdelta_ui::sleep_ms(16);
            continue;
        };

        match key {
            KeyCode::Right => {
                selected = (selected + 1) % APPS.len();
                dirty = true;
            }
            KeyCode::Tab => {
                selected = (selected + 1) % APPS.len();
                dirty = true;
            }
            KeyCode::Left => {
                selected = (selected + APPS.len() - 1) % APPS.len();
                dirty = true;
            }
            KeyCode::Down => {
                if selected + 4 < APPS.len() {
                    selected += 4;
                } else {
                    selected %= 4;
                }
                dirty = true;
            }
            KeyCode::Up => {
                if selected >= 4 {
                    selected -= 4;
                } else {
                    let row_candidate = selected + 8;
                    selected = if row_candidate < APPS.len() {
                        row_candidate
                    } else {
                        selected + 4
                    };
                }
                dirty = true;
            }
            KeyCode::Enter => {
                let entry = &APPS[selected];
                if entry.exec_path.is_empty() {
                    desktop.request_exit_to_shell();
                    return 0;
                }
                desktop.request_launch(entry.exec_path, entry.arg);
                return 0;
            }
            KeyCode::Esc => {
                desktop.request_exit_to_shell();
                return 0;
            }
            KeyCode::Char => {
                let idx = match ch {
                    b'1'..=b'9' => Some(usize::from(ch - b'1')),
                    b'0' => Some(9),
                    b'x' | b'X' => Some(10),
                    _ => None,
                };
                if let Some(target) = idx {
                    let entry = &APPS[target];
                    if entry.exec_path.is_empty() {
                        desktop.request_exit_to_shell();
                        return 0;
                    }
                    desktop.request_launch(entry.exec_path, entry.arg);
                    return 0;
                }
            }
            _ => {}
        }
    }
}

fn count_hsapps(vfs: usize, dir: &str) -> u32 {
    let mut buf = [0u8; 1024];
    let Ok(n) = libdelta::vfs_readdir(vfs, dir, &mut buf) else {
        return 0;
    };
    let Ok(listing) = core::str::from_utf8(&buf[..n]) else {
        return 0;
    };
    listing.lines().filter(|l| l.contains(".hsapp")).count() as u32
}

fn render_launcher(
    desktop: &mut DesktopClient,
    cfg: &OsConfig,
    caps: &SysCaps,
    selected: usize,
    bundle_count: u32,
) {
    let lang = cfg.lang;
    let c = &mut desktop.canvas;
    c.clear(BG);

    // Header bar
    c.draw_box(16, 10, (APP_W - 32) as i32, 42, PANEL, BORDER);
    c.draw_text(
        28,
        24,
        tr(
            lang,
            "App Launcher (.hsapp Grid)",
            "Launcher Aplikacji (.hsapp)",
        ),
        ACCENT,
    );

    let mut stats = [0u8; 80];
    let mut sw = BufWriter::new(&mut stats);
    sw.str(tr(lang, "Bundles: ", "Pakiety: "));
    sw.u32(bundle_count);
    sw.str(" | ");
    sw.str(caps.board_name_str());
    c.draw_text_right((APP_W - 28) as i32, 24, sw.as_str(), MUTED);

    // 4 columns x 3 rows grid
    let start_x = 16;
    let start_y = 62;
    let tile_w = 144;
    let tile_h = 104;
    let gap_x = 10;
    let gap_y = 10;

    for (idx, item) in APPS.iter().enumerate() {
        let col = (idx % 4) as i32;
        let row = (idx / 4) as i32;
        let x = start_x + col * (tile_w + gap_x);
        let y = start_y + row * (tile_h + gap_y);
        let active = idx == selected;

        c.draw_box(
            x,
            y,
            tile_w,
            tile_h,
            if active { 0x001B_2433 } else { PANEL },
            if active { ACCENT } else { BORDER },
        );

        draw_icon_2x(
            c,
            x + (tile_w - 32) / 2,
            y + 18,
            item.icon,
            if active { ACCENT } else { TEXT },
        );

        let label = tr(lang, item.en, item.pl);
        c.draw_text_centered(
            x + tile_w / 2,
            y + 62,
            label,
            if active { ACCENT } else { TEXT },
            1,
        );

        let mut badge = [0u8; 8];
        let mut bw = BufWriter::new(&mut badge);
        bw.str("[");
        bw.str(item.key_hint);
        bw.str("]");
        c.draw_text(x + 8, y + 8, bw.as_str(), if active { CYAN } else { MUTED });
    }

    // Info card in bottom-right 12th slot (`col=3, row=2`)
    let info_x = start_x + 3 * (tile_w + gap_x);
    let info_y = start_y + 2 * (tile_h + gap_y);
    c.draw_box(info_x, info_y, tile_w, tile_h, BG, BORDER);
    c.draw_text_centered(info_x + tile_w / 2, info_y + 22, "Delta Kernel", MUTED, 1);
    c.draw_text_centered(info_x + tile_w / 2, info_y + 44, "Modular EL0", OK_GREEN, 1);
    c.draw_text_centered(info_x + tile_w / 2, info_y + 68, "v0.6.0-rc.2", CYAN, 1);

    c.draw_box(16, 408, (APP_W - 32) as i32, 34, PANEL, BORDER);
    c.draw_text_centered(
        (APP_W / 2) as i32,
        418,
        tr(
            lang,
            "[Arrows/Tab] Navigate   [Enter / 1-9,0] Launch   [X/Esc] UART Shell   [F1] Help",
            "[Strzalki/Tab] Wybor   [Enter / 1-9,0] Uruchom   [X/Esc] Konsola   [F1] Pomoc",
        ),
        TEXT,
        1,
    );
}
