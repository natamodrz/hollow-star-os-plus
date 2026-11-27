#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Standalone `1st Setup` Wizard (`/system/apps/Setup.hsapp/exec`).

use libdelta_ui::delta_abi::{KeyCode, SVC_VFS, SysCaps};
use libdelta_ui::gfx::{ACCENT, APP_W, BG, BORDER, CYAN, MUTED, OK_GREEN, PANEL, TEXT};
use libdelta_ui::{BufWriter, DesktopClient, Lang, OsConfig, tr, wireless_enabled};

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let Ok(mut desktop) = DesktopClient::connect() else {
        return 1;
    };
    let vfs = libdelta::wait_for_service(SVC_VFS);
    let caps = libdelta::sys_caps().unwrap_or_else(|_| SysCaps::empty());

    let mut cfg = OsConfig::defaults();
    cfg.load_from_vfs(vfs);

    let mut step = 0usize; // 0: Language, 1: Clock, 2: Ready
    let mut time_field = 0usize; // 0: Hours, 1: Minutes
    let mut dirty = true;

    loop {
        if dirty {
            render_setup(&mut desktop, &cfg, &caps, step, time_field);
            desktop.present();
            dirty = false;
        }

        let Some((key, ch)) = desktop.poll_key() else {
            libdelta_ui::sleep_ms(16);
            continue;
        };

        if key == KeyCode::Esc && cfg.setup_done {
            desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
            return 0;
        }

        match step {
            0 => match key {
                KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                    cfg.lang = if cfg.lang == Lang::En {
                        Lang::Pl
                    } else {
                        Lang::En
                    };
                    cfg.save_to_vfs(vfs);
                    desktop.notify_config_updated();
                    dirty = true;
                }
                KeyCode::Char if ch == b'1' => {
                    cfg.lang = Lang::En;
                    cfg.save_to_vfs(vfs);
                    desktop.notify_config_updated();
                    dirty = true;
                }
                KeyCode::Char if ch == b'2' => {
                    cfg.lang = Lang::Pl;
                    cfg.save_to_vfs(vfs);
                    desktop.notify_config_updated();
                    dirty = true;
                }
                KeyCode::Enter => {
                    step = 1;
                    dirty = true;
                }
                _ => {}
            },
            1 => match key {
                KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                    time_field = 1 - time_field;
                    dirty = true;
                }
                KeyCode::Left => {
                    if time_field == 0 {
                        cfg.clock_hh = (cfg.clock_hh + 23) % 24;
                    } else {
                        cfg.clock_mm = (cfg.clock_mm + 55) % 60;
                    }
                    cfg.save_to_vfs(vfs);
                    desktop.notify_config_updated();
                    dirty = true;
                }
                KeyCode::Right => {
                    if time_field == 0 {
                        cfg.clock_hh = (cfg.clock_hh + 1) % 24;
                    } else {
                        cfg.clock_mm = (cfg.clock_mm + 5) % 60;
                    }
                    cfg.save_to_vfs(vfs);
                    desktop.notify_config_updated();
                    dirty = true;
                }
                KeyCode::Enter => {
                    step = 2;
                    dirty = true;
                }
                KeyCode::Esc => {
                    step = 0;
                    dirty = true;
                }
                _ => {}
            },
            _ => match key {
                KeyCode::Enter => {
                    cfg.setup_done = true;
                    cfg.save_to_vfs(vfs);
                    desktop.notify_config_updated();
                    desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
                    return 0;
                }
                KeyCode::Esc => {
                    step = 1;
                    dirty = true;
                }
                _ => {}
            },
        }
    }
}

fn render_setup(
    desktop: &mut DesktopClient,
    cfg: &OsConfig,
    caps: &SysCaps,
    step: usize,
    time_field: usize,
) {
    let lang = cfg.lang;
    let c = &mut desktop.canvas;
    c.clear(BG);

    // Outer setup card
    c.draw_box(24, 18, (APP_W - 48) as i32, 412, PANEL, ACCENT);
    c.draw_hollow_star(56, 48, 13, ACCENT);
    c.draw_text_scaled(
        82,
        36,
        tr(
            lang,
            "Hollow Star OS+ - 1st Setup",
            "Hollow Star OS+ - Konfiguracja",
        ),
        TEXT,
        1,
    );

    let mut step_buf = [0u8; 40];
    let mut sw = BufWriter::new(&mut step_buf);
    sw.str(tr(lang, "Step ", "Krok "));
    sw.u32((step + 1) as u32);
    sw.str(" / 3");
    c.draw_text_right((APP_W - 42) as i32, 36, sw.as_str(), ACCENT);

    // Progress bar across 3 steps
    for idx in 0..3i32 {
        let bx = 46 + idx * 184;
        let fill = if idx <= step as i32 { ACCENT } else { BORDER };
        c.fill_rect(bx, 68, 174, 4, fill);
    }

    match step {
        0 => {
            c.draw_text(
                46,
                96,
                tr(
                    lang,
                    "Choose interface language:",
                    "Wybierz jezyk interfejsu:",
                ),
                TEXT,
            );
            let en_border = if lang == Lang::En { ACCENT } else { BORDER };
            let pl_border = if lang == Lang::Pl { ACCENT } else { BORDER };

            c.draw_box(46, 134, 548, 56, BG, en_border);
            c.draw_text(
                68,
                154,
                "[1] English (Default)  - Delta Kernel Desktop & POSIX Shell",
                if lang == Lang::En { ACCENT } else { TEXT },
            );

            c.draw_box(46, 206, 548, 56, BG, pl_border);
            c.draw_text(
                68,
                226,
                "[2] Polski             - System Hollow Star OS+ po polsku",
                if lang == Lang::Pl { ACCENT } else { TEXT },
            );

            c.draw_text(
                46,
                300,
                tr(
                    lang,
                    "Board detected: settings stored on /user/config/os.cfg (DeltaFS WAL)",
                    "Ustawienia zapisywane w /user/config/os.cfg (DeltaFS z dziennikiem)",
                ),
                MUTED,
            );
            c.draw_text(
                46,
                390,
                tr(
                    lang,
                    "[Up/Down/Tab/1/2] Switch language   [Enter] Next step   [F1] Help",
                    "[Strzalki/Tab/1/2] Zmien jezyk   [Enter] Dalej   [F1] Pomoc",
                ),
                CYAN,
            );
        }
        1 => {
            c.draw_text(
                46,
                96,
                tr(
                    lang,
                    "Set system clock (HH:MM):",
                    "Ustaw zegar systemowy (GG:MM):",
                ),
                TEXT,
            );
            c.draw_box(
                130,
                148,
                170,
                88,
                BG,
                if time_field == 0 { ACCENT } else { BORDER },
            );
            c.draw_box(
                340,
                148,
                170,
                88,
                BG,
                if time_field == 1 { ACCENT } else { BORDER },
            );

            let mut hb = [0u8; 8];
            let mut hw = BufWriter::new(&mut hb);
            hw.u32_pad2(u32::from(cfg.clock_hh));
            c.draw_text_centered(
                215,
                172,
                hw.as_str(),
                if time_field == 0 { ACCENT } else { TEXT },
                3,
            );
            c.draw_text_centered(215, 246, tr(lang, "Hours", "Godziny"), MUTED, 1);

            c.draw_text_centered(320, 172, ":", MUTED, 3);

            let mut mb = [0u8; 8];
            let mut mw = BufWriter::new(&mut mb);
            mw.u32_pad2(u32::from(cfg.clock_mm));
            c.draw_text_centered(
                425,
                172,
                mw.as_str(),
                if time_field == 1 { ACCENT } else { TEXT },
                3,
            );
            c.draw_text_centered(425, 246, tr(lang, "Minutes", "Minuty"), MUTED, 1);

            c.draw_text(
                46,
                390,
                tr(
                    lang,
                    "[Up/Down] Field   [Left/Right] -/+   [Enter] Next   [Esc] Back",
                    "[Gora/Dol] Pole   [Lewo/Prawo] -/+   [Enter] Dalej   [Esc] Wstecz",
                ),
                CYAN,
            );
        }
        _ => {
            c.draw_text(
                46,
                96,
                tr(
                    lang,
                    "Setup complete! Hardware profile:",
                    "Konfiguracja gotowa! Profil sprzetu:",
                ),
                OK_GREEN,
            );

            c.draw_box(46, 134, 548, 168, BG, BORDER);
            c.draw_text(68, 154, caps.board_name_str(), ACCENT);

            let mut mb = [0u8; 96];
            let mut mw = BufWriter::new(&mut mb);
            mw.str("ARM64 SMP Cortex-A53 | RAM: ");
            mw.u32((caps.ram_total_bytes / (1024 * 1024)) as u32);
            mw.str(" MiB | Free: ");
            mw.u32((caps.ram_free_bytes / (1024 * 1024)) as u32);
            mw.str(" MiB");
            c.draw_text(68, 184, mw.as_str(), TEXT);

            c.draw_text(
                68,
                214,
                if wireless_enabled(caps) {
                    tr(
                        lang,
                        "Wireless Radio: Wi-Fi / Bluetooth detected (ready)",
                        "Lacznosc: Wykryto modul Wi-Fi / Bluetooth",
                    )
                } else {
                    tr(
                        lang,
                        "Wireless Radio: Not present (Wi-Fi/BT UI auto-hidden)",
                        "Lacznosc: Brak Wi-Fi/BT (opcje automatycznie ukryte)",
                    )
                },
                MUTED,
            );

            c.draw_text(
                68,
                244,
                tr(
                    lang,
                    "Partitions: /boot (FAT32)  /system (DeltaFS)  /user (DeltaFS WAL)",
                    "Partycje: /boot (FAT32)  /system (DeltaFS)  /user (DeltaFS WAL)",
                ),
                MUTED,
            );

            c.draw_box(130, 326, 380, 46, PANEL, ACCENT);
            c.draw_text_centered(
                320,
                341,
                tr(
                    lang,
                    "[Enter] Open Hollow Star Launcher",
                    "[Enter] Otworz Launcher Hollow Star",
                ),
                ACCENT,
                1,
            );
        }
    }
}
