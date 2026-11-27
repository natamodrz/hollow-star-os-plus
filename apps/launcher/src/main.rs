#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Standalone `.hsapp` Grid Launcher (`/system/apps/Launcher.hsapp/exec`).

use libdelta_ui::delta_abi::{KeyCode, SVC_VFS, SysCaps};
use libdelta_ui::gfx::{
    ACCENT_BLUE, ACCENT_MAGENTA, APP_W, BG, BORDER, MUTED, PANEL, PANEL_ALT, TEXT,
};
use libdelta_ui::{
    ArcSharpStyle, BufWriter, DesktopClient, IconKind, OsConfig, draw_icon_2x, ease_out_cubic,
    lerp_color, sleep_ms, tr,
};

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
    let mut prev_selected = 0usize;
    let mut dirty = true;

    loop {
        if dirty {
            if prev_selected != selected {
                animate_focus(
                    &mut desktop,
                    &cfg,
                    &caps,
                    prev_selected,
                    selected,
                    bundle_count,
                );
                prev_selected = selected;
            }
            render_launcher(&mut desktop, &cfg, &caps, selected, bundle_count, None);
            desktop.present();
            dirty = false;
        }

        let Some((key, ch)) = desktop.poll_key() else {
            sleep_ms(16);
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

const GRID_X: i32 = 16;
const GRID_Y: i32 = 60;
const TILE_W: i32 = 144;
const TILE_H: i32 = 104;
const GAP_X: i32 = 10;
const GAP_Y: i32 = 10;

fn tile_pos(idx: usize) -> (i32, i32) {
    let col = (idx % 4) as i32;
    let row = (idx / 4) as i32;
    (
        GRID_X + col * (TILE_W + GAP_X),
        GRID_Y + row * (TILE_H + GAP_Y),
    )
}

fn animate_focus(
    desktop: &mut DesktopClient,
    cfg: &OsConfig,
    caps: &SysCaps,
    from: usize,
    to: usize,
    bundle_count: u32,
) {
    let (fx, fy) = tile_pos(from);
    let (tx, ty) = tile_pos(to);
    for step in 1..=2u32 {
        let p = ease_out_cubic((step * 255) / 3) as i32;
        let gx = fx + ((tx - fx) * p) / 255;
        let gy = fy + ((ty - fy) * p) / 255;
        let glow = lerp_color(ACCENT_MAGENTA, ACCENT_BLUE, p as u8);
        render_launcher(desktop, cfg, caps, to, bundle_count, Some((gx, gy, glow)));
        desktop.present();
        sleep_ms(10);
    }
}

fn render_launcher(
    desktop: &mut DesktopClient,
    cfg: &OsConfig,
    caps: &SysCaps,
    selected: usize,
    bundle_count: u32,
    glide: Option<(i32, i32, u32)>,
) {
    let lang = cfg.lang;
    let c = &mut desktop.canvas;
    c.clear(BG);

    // Fullscreen Cyberdeck & DIY PDA Header
    c.draw_arc_box(
        16,
        8,
        (APP_W - 32) as i32,
        44,
        ArcSharpStyle::cyberdeck_card(),
        PANEL,
        ACCENT_BLUE,
        ACCENT_MAGENTA,
    );
    c.draw_arc_star(34, 29, 8, ACCENT_BLUE, ACCENT_MAGENTA);
    c.draw_text(
        50,
        21,
        tr(lang, "CYBERDECK LAUNCHER", "LAUNCHER CYBERDECK"),
        ACCENT_BLUE,
    );
    c.fill_gradient_h(26, 48, (APP_W - 52) as i32, 2, ACCENT_BLUE, ACCENT_MAGENTA);

    let mut stats = [0u8; 96];
    let mut sw = BufWriter::new(&mut stats);
    sw.str("PDA/SBC | ");
    sw.str(tr(lang, "Apps: ", "Pakiety: "));
    sw.u32(bundle_count);
    sw.str(" | ");
    sw.str(caps.board_name_str());
    c.draw_text_right((APP_W - 28) as i32, 21, sw.as_str(), MUTED);

    // 4 columns x 3 rows fullscreen tile grid
    for (idx, item) in APPS.iter().enumerate() {
        let (x, y) = tile_pos(idx);
        let active = idx == selected;

        c.draw_arc_box(
            x,
            y,
            TILE_W,
            TILE_H,
            ArcSharpStyle::pda_tile(),
            if active { PANEL_ALT } else { PANEL },
            if active { ACCENT_BLUE } else { BORDER },
            if active { ACCENT_MAGENTA } else { BORDER },
        );
        if active {
            c.fill_gradient_h(
                x + 14,
                y + TILE_H - 8,
                TILE_W - 28,
                3,
                ACCENT_BLUE,
                ACCENT_MAGENTA,
            );
        }

        draw_icon_2x(
            c,
            x + (TILE_W - 32) / 2,
            y + 20,
            item.icon,
            if active { ACCENT_BLUE } else { TEXT },
        );

        let label = tr(lang, item.en, item.pl);
        c.draw_text_centered(
            x + TILE_W / 2,
            y + 62,
            label,
            if active { TEXT } else { MUTED },
            1,
        );

        let mut badge = [0u8; 8];
        let mut bw = BufWriter::new(&mut badge);
        bw.str("[");
        bw.str(item.key_hint);
        bw.str("]");
        c.draw_text(
            x + 10,
            y + 8,
            bw.as_str(),
            if active { ACCENT_MAGENTA } else { MUTED },
        );
    }

    if let Some((gx, gy, glow)) = glide {
        c.draw_arc_box(
            gx,
            gy,
            TILE_W,
            TILE_H,
            ArcSharpStyle::pda_tile(),
            PANEL_ALT,
            glow,
            ACCENT_MAGENTA,
        );
    }

    // Cyberdeck / PDA Identity card in bottom-right 12th slot (`col=3, row=2`)
    let (info_x, info_y) = tile_pos(11);
    c.draw_arc_box(
        info_x,
        info_y,
        TILE_W,
        TILE_H,
        ArcSharpStyle::pda_tile(),
        PANEL,
        ACCENT_BLUE,
        ACCENT_MAGENTA,
    );
    c.draw_arc_star(
        info_x + TILE_W / 2,
        info_y + 24,
        9,
        ACCENT_BLUE,
        ACCENT_MAGENTA,
    );
    c.draw_text_centered(info_x + TILE_W / 2, info_y + 42, "Hollow Star+", TEXT, 1);
    c.draw_text_centered(
        info_x + TILE_W / 2,
        info_y + 60,
        "PDA / SBC Mode",
        ACCENT_BLUE,
        1,
    );
    c.draw_text_centered(
        info_x + TILE_W / 2,
        info_y + 78,
        "v0.6.0-rc.2",
        ACCENT_MAGENTA,
        1,
    );

    c.draw_arc_box(
        16,
        404,
        (APP_W - 32) as i32,
        38,
        ArcSharpStyle::cyberdeck_card(),
        PANEL_ALT,
        BORDER,
        ACCENT_MAGENTA,
    );
    c.draw_text_centered(
        (APP_W / 2) as i32,
        415,
        tr(
            lang,
            "[Arrows/Tab] Select   [Enter / 1-9,0] Launch   [X/Esc] Shell   [F1] Help",
            "[Strzalki/Tab] Wybierz   [Enter / 1-9,0] Uruchom   [X/Esc] Konsola   [F1] Pomoc",
        ),
        TEXT,
        1,
    );
}
