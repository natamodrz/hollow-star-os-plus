#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Hollow Star OS+ Core Utilities Suite (`/system/apps/Apps.hsapp/exec`).
//!
//! Renders inside the compositor-provided `640x452` `Shm` canvas via `libdelta_ui`:
//! `arg = 0` Sys Info | `1` Markdown Notes | `2` File Explorer |
//! `3` Media Player RT | `4` Calculator | `5` Games | `6` Settings.

mod calc;
mod games;

use games::{GameId, GameState, Key as GameKey};
use libdelta_ui::delta_abi::{
    AudioPacket, FbPacket, IpcMessage, KeyCode, MSG_AUDIO_PLAY_TONE, MSG_FB_GET_INFO, SVC_AUDIO,
    SVC_FB, SVC_VFS, SysCaps, VfsPartitionStat, pack_msg, unpack_msg,
};
use libdelta_ui::gfx::{
    ACCENT, ACCENT_BLUE, ACCENT_MAGENTA, APP_W, BG, BORDER, CYAN, Canvas, MUTED, OK_GREEN, PANEL,
    PANEL_ALT, TEXT,
};
use libdelta_ui::{ArcSharpStyle, BufWriter, DesktopClient, Lang, OsConfig, tr, wireless_enabled};

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppMode {
    SysInfo = 0,
    Notes = 1,
    Files = 2,
    Media = 3,
    Calc = 4,
    GamesMenu = 5,
    Settings = 6,
}

impl AppMode {
    fn from_arg(arg: u64) -> Self {
        match arg {
            0 => Self::SysInfo,
            1 => Self::Notes,
            2 => Self::Files,
            3 => Self::Media,
            4 => Self::Calc,
            5 => Self::GamesMenu,
            6 => Self::Settings,
            _ => Self::SysInfo,
        }
    }
}

struct AppState {
    vfs: usize,
    fb_port: usize,
    audio: usize,
    caps: SysCaps,
    cfg: OsConfig,
    temp_mc: u32,
    mode: AppMode,
    in_gameplay: bool,
    // Notes
    note_buf: [u8; 512],
    note_len: usize,
    note_path: &'static str,
    note_status: &'static str,
    note_filter: [u8; 24],
    note_filter_len: usize,
    note_find_active: bool,
    // Files
    file_dir: [u8; 64],
    file_dir_len: usize,
    file_sel: usize,
    file_preview: [u8; 256],
    file_preview_len: usize,
    // Media
    media_track: usize,
    media_playing: bool,
    // Calc
    calc_expr: [u8; 48],
    calc_len: usize,
    calc_res: [u8; 48],
    calc_res_len: usize,
    calc_fraction: bool,
    // Games & Settings
    game_menu_sel: usize,
    game: GameState,
    settings_sel: usize,
}

impl AppState {
    fn new(
        vfs: usize,
        fb_port: usize,
        audio: usize,
        caps: SysCaps,
        cfg: OsConfig,
        arg: u64,
    ) -> Self {
        let mut s = Self {
            vfs,
            fb_port,
            audio,
            caps,
            cfg,
            temp_mc: query_soc_temp_mc(fb_port).unwrap_or(42_000),
            mode: AppMode::from_arg(arg),
            in_gameplay: false,
            note_buf: [0; 512],
            note_len: 0,
            note_path: "/user/notes/welcome.md",
            note_status: "",
            note_filter: [0; 24],
            note_filter_len: 0,
            note_find_active: false,
            file_dir: [0; 64],
            file_dir_len: 1,
            file_sel: 0,
            file_preview: [0; 256],
            file_preview_len: 0,
            media_track: 0,
            media_playing: false,
            calc_expr: [0; 48],
            calc_len: 0,
            calc_res: [0; 48],
            calc_res_len: 0,
            calc_fraction: false,
            game_menu_sel: 0,
            game: GameState::new(GameId::Brick),
            settings_sel: 0,
        };
        s.file_dir[0] = b'/';
        s.load_note("/user/notes/welcome.md");
        s.set_calc_default();
        s
    }

    fn load_note(&mut self, path: &'static str) {
        self.note_path = path;
        self.note_len = 0;
        if let Ok(n) = libdelta::vfs_read_file(self.vfs, path, &mut self.note_buf) {
            self.note_len = n;
        }
    }

    fn save_note(&mut self) {
        let _ = libdelta::vfs_mkdir(self.vfs, "/user/notes");
        let _ = libdelta::vfs_write_file(self.vfs, self.note_path, &self.note_buf[..self.note_len]);
        self.note_status = tr(self.cfg.lang, "Saved!", "Zapisano!");
    }

    fn set_calc_default(&mut self) {
        let sample = b"sqrt(144)+3/2";
        self.calc_expr[..sample.len()].copy_from_slice(sample);
        self.calc_len = sample.len();
        self.eval_calc();
    }

    fn eval_calc(&mut self) {
        let expr = core::str::from_utf8(&self.calc_expr[..self.calc_len]).unwrap_or("");
        match calc::eval_expr(expr) {
            Ok(v) => {
                self.calc_res_len = if self.calc_fraction {
                    calc::format_fraction(v, &mut self.calc_res)
                } else {
                    calc::format_decimal(v, &mut self.calc_res)
                };
            }
            Err(e) => {
                let b = e.as_bytes();
                let n = b.len().min(self.calc_res.len());
                self.calc_res[..n].copy_from_slice(&b[..n]);
                self.calc_res_len = n;
            }
        }
    }
}

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let Ok(mut desktop) = DesktopClient::connect() else {
        return 1;
    };
    let vfs = libdelta::wait_for_service(SVC_VFS);
    let fb_port = libdelta::wait_for_service(SVC_FB);
    let audio = libdelta::wait_for_service(SVC_AUDIO);
    let caps = libdelta::sys_caps().unwrap_or_else(|_| SysCaps::empty());

    let mut cfg = OsConfig::defaults();
    cfg.load_from_vfs(vfs);

    let mut app = AppState::new(vfs, fb_port, audio, caps, cfg, desktop.launch_arg);
    let mut dirty = true;

    loop {
        if dirty || app.in_gameplay {
            if app.in_gameplay {
                app.game.tick();
                if app.game.game_over {
                    let prev = match app.game.active {
                        GameId::Brick => app.cfg.high_score_brick,
                        GameId::Tetris => app.cfg.high_score_tetris,
                        GameId::Racing => app.cfg.high_score_race,
                    };
                    if app.game.score > prev {
                        match app.game.active {
                            GameId::Brick => app.cfg.high_score_brick = app.game.score,
                            GameId::Tetris => app.cfg.high_score_tetris = app.game.score,
                            GameId::Racing => app.cfg.high_score_race = app.game.score,
                        }
                        app.cfg.save_to_vfs(vfs);
                    }
                }
            }
            render_app(&mut desktop.canvas, &app);
            desktop.present();
            dirty = false;
        }

        let Some((key, ch)) = desktop.poll_key() else {
            libdelta_ui::sleep_ms(if app.in_gameplay { 24 } else { 16 });
            continue;
        };

        if app.in_gameplay {
            match key {
                KeyCode::Esc => {
                    app.in_gameplay = false;
                    dirty = true;
                }
                KeyCode::Left => app.game.on_key(GameKey::Left),
                KeyCode::Right => app.game.on_key(GameKey::Right),
                KeyCode::Up => app.game.on_key(GameKey::Up),
                KeyCode::Down => app.game.on_key(GameKey::Down),
                KeyCode::Enter => app.game.on_key(GameKey::Action),
                KeyCode::Char if ch == b' ' => app.game.on_key(GameKey::Action),
                _ => {}
            }
            continue;
        }

        match app.mode {
            AppMode::SysInfo => match key {
                KeyCode::Esc | KeyCode::Enter => {
                    desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
                    return 0;
                }
                KeyCode::Char if ch == b'r' || ch == b'R' => {
                    app.caps = libdelta::sys_caps().unwrap_or(app.caps);
                    app.temp_mc = query_soc_temp_mc(app.fb_port).unwrap_or(app.temp_mc);
                    dirty = true;
                }
                _ => {}
            },
            AppMode::Notes => {
                app.note_status = "";
                match key {
                    KeyCode::Esc => {
                        if app.note_find_active {
                            app.note_find_active = false;
                        } else {
                            desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
                            return 0;
                        }
                        dirty = true;
                    }
                    KeyCode::CtrlS => {
                        app.save_note();
                        dirty = true;
                    }
                    KeyCode::CtrlN => {
                        app.load_note("/user/notes/scratch.md");
                        app.note_len = 0;
                        app.note_status = tr(app.cfg.lang, "New note", "Nowa notatka");
                        dirty = true;
                    }
                    KeyCode::CtrlF => {
                        app.note_find_active = !app.note_find_active;
                        dirty = true;
                    }
                    KeyCode::Backspace => {
                        if app.note_find_active {
                            if app.note_filter_len > 0 {
                                app.note_filter_len -= 1;
                            }
                        } else if app.note_len > 0 {
                            app.note_len -= 1;
                        }
                        dirty = true;
                    }
                    KeyCode::Enter => {
                        if app.note_find_active {
                            app.note_find_active = false;
                        } else if app.note_len < app.note_buf.len() {
                            app.note_buf[app.note_len] = b'\n';
                            app.note_len += 1;
                        }
                        dirty = true;
                    }
                    KeyCode::Char if (0x20..=0x7E).contains(&ch) => {
                        if app.note_find_active {
                            if app.note_filter_len < app.note_filter.len() {
                                app.note_filter[app.note_filter_len] = ch;
                                app.note_filter_len += 1;
                            }
                        } else if app.note_len < app.note_buf.len() {
                            app.note_buf[app.note_len] = ch;
                            app.note_len += 1;
                        }
                        dirty = true;
                    }
                    _ => {}
                }
            }
            AppMode::Files => match key {
                KeyCode::Esc => {
                    desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
                    return 0;
                }
                KeyCode::Up => {
                    app.file_sel = app.file_sel.saturating_sub(1);
                    dirty = true;
                }
                KeyCode::Down => {
                    app.file_sel += 1;
                    dirty = true;
                }
                KeyCode::Left | KeyCode::Backspace => {
                    pop_dir_segment(&mut app.file_dir, &mut app.file_dir_len);
                    app.file_sel = 0;
                    app.file_preview_len = 0;
                    dirty = true;
                }
                KeyCode::Enter | KeyCode::Right => {
                    activate_file_entry(&mut app);
                    dirty = true;
                }
                KeyCode::Char if ch == b'e' || ch == b'E' => {
                    let _ = libdelta::vfs_expand_user(app.vfs, 0);
                    dirty = true;
                }
                _ => {}
            },
            AppMode::Media => match key {
                KeyCode::Esc => {
                    desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
                    return 0;
                }
                KeyCode::Up => {
                    app.media_track = (app.media_track + 2) % 3;
                    dirty = true;
                }
                KeyCode::Down | KeyCode::Tab => {
                    app.media_track = (app.media_track + 1) % 3;
                    dirty = true;
                }
                KeyCode::Enter => {
                    app.media_playing = true;
                    render_app(&mut desktop.canvas, &app);
                    desktop.present();
                    play_demo_track(
                        app.audio,
                        app.media_track,
                        app.cfg.volume,
                        app.cfg.audio_route,
                    );
                    app.media_playing = false;
                    dirty = true;
                }
                KeyCode::Char if ch == b' ' => {
                    app.media_playing = true;
                    render_app(&mut desktop.canvas, &app);
                    desktop.present();
                    play_demo_track(
                        app.audio,
                        app.media_track,
                        app.cfg.volume,
                        app.cfg.audio_route,
                    );
                    app.media_playing = false;
                    dirty = true;
                }
                _ => {}
            },
            AppMode::Calc => match key {
                KeyCode::Esc => {
                    desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
                    return 0;
                }
                KeyCode::Tab => {
                    app.calc_fraction = !app.calc_fraction;
                    app.eval_calc();
                    dirty = true;
                }
                KeyCode::Backspace => {
                    if app.calc_len > 0 {
                        app.calc_len -= 1;
                        app.eval_calc();
                    }
                    dirty = true;
                }
                KeyCode::Enter => {
                    app.eval_calc();
                    dirty = true;
                }
                KeyCode::Char if ch == b'c' || ch == b'C' => {
                    app.calc_len = 0;
                    app.calc_res_len = 0;
                    dirty = true;
                }
                KeyCode::Char if (0x20..=0x7E).contains(&ch) => {
                    if app.calc_len < app.calc_expr.len() {
                        app.calc_expr[app.calc_len] = ch;
                        app.calc_len += 1;
                        app.eval_calc();
                    }
                    dirty = true;
                }
                _ => {}
            },
            AppMode::GamesMenu => match key {
                KeyCode::Esc => {
                    desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
                    return 0;
                }
                KeyCode::Up => {
                    app.game_menu_sel = (app.game_menu_sel + 2) % 3;
                    dirty = true;
                }
                KeyCode::Down | KeyCode::Tab => {
                    app.game_menu_sel = (app.game_menu_sel + 1) % 3;
                    dirty = true;
                }
                KeyCode::Enter => {
                    let gid = match app.game_menu_sel {
                        0 => GameId::Brick,
                        1 => GameId::Tetris,
                        _ => GameId::Racing,
                    };
                    app.game.reset(gid);
                    app.in_gameplay = true;
                }
                KeyCode::Char if (b'1'..=b'3').contains(&ch) => {
                    let idx = usize::from(ch - b'1');
                    let gid = [GameId::Brick, GameId::Tetris, GameId::Racing][idx];
                    app.game.reset(gid);
                    app.in_gameplay = true;
                }
                _ => {}
            },
            AppMode::Settings => match key {
                KeyCode::Esc => {
                    desktop.request_launch("/system/apps/Launcher.hsapp/exec", 0);
                    return 0;
                }
                KeyCode::Up => {
                    app.settings_sel = (app.settings_sel + 3) % 4;
                    dirty = true;
                }
                KeyCode::Down | KeyCode::Tab => {
                    app.settings_sel = (app.settings_sel + 1) % 4;
                    dirty = true;
                }
                KeyCode::Left => {
                    adjust_setting(&mut app.cfg, app.settings_sel, -1);
                    app.cfg.save_to_vfs(app.vfs);
                    desktop.notify_config_updated();
                    dirty = true;
                }
                KeyCode::Right | KeyCode::Enter => {
                    adjust_setting(&mut app.cfg, app.settings_sel, 1);
                    app.cfg.save_to_vfs(app.vfs);
                    desktop.notify_config_updated();
                    dirty = true;
                }
                KeyCode::Char if ch == b'r' || ch == b'R' => {
                    app.cfg.setup_done = false;
                    app.cfg.save_to_vfs(app.vfs);
                    desktop.notify_config_updated();
                    dirty = true;
                }
                _ => {}
            },
        }
    }
}

fn adjust_setting(cfg: &mut OsConfig, sel: usize, delta: i32) {
    match sel {
        0 => {
            cfg.lang = if cfg.lang == Lang::En {
                Lang::Pl
            } else {
                Lang::En
            };
        }
        1 => {
            let v = i32::from(cfg.volume) + delta * 10;
            cfg.volume = v.clamp(0, 100) as u8;
        }
        2 => {
            cfg.audio_route = 1 - cfg.audio_route;
        }
        _ => {
            let b = i32::from(cfg.brightness) + delta * 10;
            cfg.brightness = b.clamp(30, 100) as u8;
        }
    }
}

fn render_app(c: &mut Canvas, app: &AppState) {
    if app.in_gameplay {
        c.clear(BG);
        let hs = match app.game.active {
            GameId::Brick => app.cfg.high_score_brick,
            GameId::Tetris => app.cfg.high_score_tetris,
            GameId::Racing => app.cfg.high_score_race,
        };
        app.game.draw(c, app.cfg.lang, hs);
        return;
    }

    c.clear(BG);
    let lang = app.cfg.lang;

    match app.mode {
        AppMode::SysInfo => render_sysinfo(c, app, lang),
        AppMode::Notes => render_notes(c, app, lang),
        AppMode::Files => render_files(c, app, lang),
        AppMode::Media => render_media(c, app, lang),
        AppMode::Calc => render_calc(c, app, lang),
        AppMode::GamesMenu => render_games_menu(c, app, lang),
        AppMode::Settings => render_settings(c, app, lang),
    }
}

fn draw_app_header(c: &mut Canvas, title: &str, hint: &str) {
    c.draw_arc_box(
        16,
        10,
        (APP_W - 32) as i32,
        40,
        ArcSharpStyle::cyberdeck_card(),
        PANEL,
        ACCENT_BLUE,
        ACCENT_MAGENTA,
    );
    c.draw_text(28, 23, title, ACCENT_BLUE);
    c.draw_text_right((APP_W - 28) as i32, 23, hint, MUTED);
}

fn render_sysinfo(c: &mut Canvas, app: &AppState, lang: Lang) {
    draw_app_header(
        c,
        tr(lang, "System Info & Telemetry", "Informacje o Systemie"),
        "[R] Refresh  [Esc] Launcher",
    );
    c.draw_box(16, 62, (APP_W - 32) as i32, 370, PANEL, BORDER);

    c.draw_text(34, 82, app.caps.board_name_str(), ACCENT);

    let mut line = [0u8; 80];
    let mut w = BufWriter::new(&mut line);
    w.str(tr(
        lang,
        "Architecture: ARM64 (4x Cortex-A53 SMP) | EL0 Microkernel",
        "Architektura: ARM64 (4x Cortex-A53 SMP) | Mikrojądro EL0",
    ));
    c.draw_text(34, 110, w.as_str(), TEXT);

    let mut ram = [0u8; 64];
    let mut rw = BufWriter::new(&mut ram);
    rw.str(tr(lang, "Memory: ", "Pamięć: "));
    rw.u32((app.caps.ram_free_bytes / (1024 * 1024)) as u32);
    rw.str(" MiB free / ");
    rw.u32((app.caps.ram_total_bytes / (1024 * 1024)) as u32);
    rw.str(" MiB total");
    c.draw_text(34, 138, rw.as_str(), OK_GREEN);

    let mut th = [0u8; 64];
    let mut tw = BufWriter::new(&mut th);
    tw.str(tr(lang, "SoC Temp: ", "Temp SoC: "));
    tw.u32(app.temp_mc / 1000);
    tw.str(" C | Uptime: ");
    tw.u32((app.caps.uptime_ms / 1000) as u32);
    tw.str(" s");
    c.draw_text(34, 166, tw.as_str(), CYAN);

    c.draw_text(
        34,
        194,
        if wireless_enabled(&app.caps) {
            tr(
                lang,
                "Wireless: Wi-Fi / BT available",
                "Łączność: Wi-Fi / BT dostępne",
            )
        } else {
            tr(
                lang,
                "Wireless: None (Raspberry Pi Ethernet)",
                "Łączność: Brak modułu Wi-Fi/BT",
            )
        },
        MUTED,
    );

    c.draw_text(
        34,
        232,
        tr(
            lang,
            "Mounted Filesystems (VFS):",
            "Zamontowane systemy plików (VFS):",
        ),
        ACCENT,
    );
    // SAFETY: `VfsPartitionStat` is POD zero-initializable.
    let mut stats: [VfsPartitionStat; 4] = [unsafe { core::mem::zeroed() }; 4];
    let n = libdelta::vfs_statfs(app.vfs, &mut stats).unwrap_or(0);
    let mut y = 260;
    for st in &stats[..n] {
        let mut sb = [0u8; 72];
        let mut sw = BufWriter::new(&mut sb);
        let m = core::str::from_utf8(&st.mount[..st.mount_len as usize]).unwrap_or("?");
        let f = core::str::from_utf8(&st.fs_type[..st.fs_len as usize]).unwrap_or("?");
        sw.str(m);
        sw.str(" -> ");
        sw.str(f);
        sw.str(" (");
        sw.u32((st.total_bytes.saturating_sub(st.free_bytes) / 1024) as u32);
        sw.str("/");
        sw.u32((st.total_bytes / 1024) as u32);
        sw.str(" KiB)");
        c.draw_text(46, y, sw.as_str(), TEXT);
        y += 26;
    }

    c.draw_text(34, 396, "[Esc / Enter] Launcher", MUTED);
}

fn render_notes(c: &mut Canvas, app: &AppState, lang: Lang) {
    draw_app_header(
        c,
        tr(lang, "Markdown Notes", "Notatki Markdown"),
        "[Ctrl+S] Save  [Ctrl+N] New  [Ctrl+F] Find  [Esc]",
    );
    c.draw_box(16, 62, (APP_W - 32) as i32, 344, BG, BORDER);

    c.draw_text(28, 72, app.note_path, CYAN);
    if !app.note_status.is_empty() {
        c.draw_text_right((APP_W - 28) as i32, 72, app.note_status, OK_GREEN);
    }

    if app.note_find_active {
        c.draw_box(28, 94, (APP_W - 56) as i32, 24, PANEL, ACCENT);
        let q = core::str::from_utf8(&app.note_filter[..app.note_filter_len]).unwrap_or("");
        c.draw_text(36, 99, "Find: ", ACCENT);
        c.draw_text(78, 99, q, TEXT);
    }

    let text = core::str::from_utf8(&app.note_buf[..app.note_len]).unwrap_or("");
    let filter = core::str::from_utf8(&app.note_filter[..app.note_filter_len]).unwrap_or("");
    let mut y = if app.note_find_active { 126 } else { 100 };

    for line in text.lines() {
        if y > 382 {
            break;
        }
        let color = if !filter.is_empty() && line.contains(filter) {
            OK_GREEN
        } else if line.starts_with('#') {
            ACCENT
        } else if line.starts_with('-') || line.starts_with('*') {
            CYAN
        } else {
            TEXT
        };
        c.draw_text(28, y, line, color);
        y += 20;
    }

    c.draw_text(
        20,
        418,
        tr(
            lang,
            "Type ASCII text directly | Ctrl+S writes DeltaFS WAL journal",
            "Pisz tekst bezpośrednio | Ctrl+S zapisuje dziennik DeltaFS",
        ),
        MUTED,
    );
}

fn render_files(c: &mut Canvas, app: &AppState, lang: Lang) {
    draw_app_header(
        c,
        tr(
            lang,
            "File Explorer (DeltaFS / FAT32)",
            "Eksplorator Plików",
        ),
        "[Up/Down] [Enter] Open  [Back] Up  [E] Expand  [Esc]",
    );
    let dir = core::str::from_utf8(&app.file_dir[..app.file_dir_len]).unwrap_or("/");
    c.draw_text(20, 58, dir, CYAN);

    c.draw_box(16, 80, 290, 354, PANEL, BORDER);
    c.draw_box(316, 80, (APP_W - 332) as i32, 354, BG, BORDER);

    let mut listing_buf = [0u8; 1024];
    let n = libdelta::vfs_readdir(app.vfs, dir, &mut listing_buf).unwrap_or(0);
    let listing = core::str::from_utf8(&listing_buf[..n]).unwrap_or("");

    let mut y = 92;
    for (idx, line) in listing.lines().enumerate() {
        if y > 410 {
            break;
        }
        let selected = idx == app.file_sel;
        if selected {
            c.fill_rect(20, y - 2, 282, 20, PANEL_ALT);
        }
        c.draw_text(28, y, line, if selected { ACCENT } else { TEXT });
        y += 22;
    }

    c.draw_text(330, 92, tr(lang, "Preview:", "Podgląd:"), ACCENT);
    let prev = core::str::from_utf8(&app.file_preview[..app.file_preview_len]).unwrap_or("");
    let mut py = 118;
    for pline in prev.lines() {
        if py > 410 {
            break;
        }
        c.draw_mono_text(330, py, pline, MUTED);
        py += 16;
    }
}

fn activate_file_entry(app: &mut AppState) {
    let dir = core::str::from_utf8(&app.file_dir[..app.file_dir_len]).unwrap_or("/");
    let mut listing_buf = [0u8; 1024];
    let n = libdelta::vfs_readdir(app.vfs, dir, &mut listing_buf).unwrap_or(0);
    let listing = core::str::from_utf8(&listing_buf[..n]).unwrap_or("");
    let Some(entry_line) = listing.lines().nth(app.file_sel) else {
        return;
    };
    let clean_name = entry_line
        .trim()
        .trim_end_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("");
    if clean_name.is_empty() {
        return;
    }

    let mut next_path = [0u8; 64];
    let mut np_len = 0usize;
    let base = dir.as_bytes();
    let max_b = base.len().min(60);
    next_path[..max_b].copy_from_slice(&base[..max_b]);
    np_len += max_b;
    if np_len == 0 || next_path[np_len - 1] != b'/' {
        next_path[np_len] = b'/';
        np_len += 1;
    }
    let add = clean_name.as_bytes();
    let cp = add.len().min(64 - np_len);
    next_path[np_len..np_len + cp].copy_from_slice(&add[..cp]);
    np_len += cp;

    let path_str = core::str::from_utf8(&next_path[..np_len]).unwrap_or("/");
    let mut test_dir = [0u8; 64];
    if entry_line.trim_end().ends_with('/')
        || libdelta::vfs_readdir(app.vfs, path_str, &mut test_dir).is_ok()
    {
        app.file_dir[..np_len].copy_from_slice(&next_path[..np_len]);
        app.file_dir_len = np_len;
        app.file_sel = 0;
        app.file_preview_len = 0;
    } else if let Ok(fn_bytes) = libdelta::vfs_read_file(app.vfs, path_str, &mut app.file_preview) {
        app.file_preview_len = fn_bytes;
    }
}

fn pop_dir_segment(buf: &mut [u8; 64], len: &mut usize) {
    while *len > 1 && buf[*len - 1] == b'/' {
        *len -= 1;
    }
    while *len > 1 && buf[*len - 1] != b'/' {
        *len -= 1;
    }
    if *len > 1 {
        *len -= 1;
    } else {
        buf[0] = b'/';
        *len = 1;
    }
}

fn render_media(c: &mut Canvas, app: &AppState, lang: Lang) {
    draw_app_header(
        c,
        tr(lang, "RT Audio Media Player", "Odtwarzacz Audio RT"),
        "[Up/Down] Track  [Enter/Space] Play  [Esc] Launcher",
    );
    let tracks = [
        (
            "Hollow Star Theme (BeOS Synth Chime)",
            "440 / 554 / 659 Hz RT PWM",
        ),
        (
            "Midnight Orbit (Arpeggio)",
            "330 / 440 / 659 / 880 Hz RT PWM",
        ),
        (
            "Retro Arcade Boot (Fanfare)",
            "523 / 659 / 784 / 1046 Hz RT PWM",
        ),
    ];

    let mut y = 78;
    for (idx, (name, sub)) in tracks.iter().enumerate() {
        let sel = idx == app.media_track;
        c.draw_box(
            24,
            y,
            (APP_W - 48) as i32,
            72,
            PANEL,
            if sel { ACCENT } else { BORDER },
        );
        c.draw_text(42, y + 16, name, if sel { ACCENT } else { TEXT });
        c.draw_text(42, y + 40, sub, MUTED);
        if sel && app.media_playing {
            c.draw_text_right((APP_W - 42) as i32, y + 26, "PLAYING RT...", OK_GREEN);
        }
        y += 88;
    }

    c.draw_text(
        24,
        368,
        tr(
            lang,
            "Audio server priority tier: Real-Time 110 (Delta Scheduler)",
            "Priorytet serwera dźwięku: Real-Time 110 (Harmonogram Delta)",
        ),
        CYAN,
    );
}

fn play_demo_track(audio: usize, track: usize, volume: u8, route: u8) {
    let notes: &[(u16, u16)] = match track {
        0 => &[(440, 120), (554, 120), (659, 140), (880, 180)],
        1 => &[(330, 90), (440, 90), (659, 90), (880, 120), (659, 90)],
        _ => &[(523, 80), (659, 80), (784, 80), (1046, 200)],
    };
    for &(freq, ms) in notes {
        let pkt = AudioPacket {
            shm_id: 0,
            frames_mixed: 0,
            sample_count: 0,
            freq_hz: freq,
            duration_ms: ms,
            volume: volume.max(25),
            route,
            playing: 0,
            jack_plugged: 0,
            rt_priority: 110,
            _pad: [0; 3],
            status: 0,
        };
        let req = pack_msg(MSG_AUDIO_PLAY_TONE, &pkt);
        let mut reply = IpcMessage::new(0);
        let _ = libdelta::port_call(audio, &req, &mut reply);
    }
}

fn render_calc(c: &mut Canvas, app: &AppState, lang: Lang) {
    draw_app_header(
        c,
        tr(lang, "Scientific Calculator", "Kalkulator Naukowy"),
        "[Type expr] [Tab] Dec/Frac [C] Clear [Esc]",
    );
    c.draw_box(32, 76, (APP_W - 64) as i32, 86, PANEL, BORDER);
    let expr = core::str::from_utf8(&app.calc_expr[..app.calc_len]).unwrap_or("");
    c.draw_mono_text(48, 92, expr, CYAN);

    let res = core::str::from_utf8(&app.calc_res[..app.calc_res_len]).unwrap_or("");
    c.draw_text_scaled(48, 124, "= ", MUTED, 1);
    c.draw_text_scaled(72, 120, res, ACCENT, 2);

    c.draw_box(32, 184, (APP_W - 64) as i32, 230, BG, BORDER);
    c.draw_text(
        52,
        206,
        tr(lang, "Supported operators:", "Obsługiwane operatory:"),
        TEXT,
    );
    c.draw_mono_text(52, 238, "+   -   *   /   ^   ( )   sqrt(x)", OK_GREEN);
    c.draw_text(
        52,
        282,
        if app.calc_fraction {
            tr(
                lang,
                "Display Mode: Exact Rational Fraction (a/b)",
                "Tryb: Ułamek zwykły (a/b)",
            )
        } else {
            tr(
                lang,
                "Display Mode: Fixed-Point Decimal",
                "Tryb: Dziesiętny",
            )
        },
        ACCENT,
    );
    c.draw_text(
        52,
        326,
        tr(
            lang,
            "Type directly with keyboard or press [Tab] to toggle Fraction",
            "Wpisuj z klawiatury lub naciśnij [Tab] dla ułamków",
        ),
        MUTED,
    );
}

fn render_games_menu(c: &mut Canvas, app: &AppState, lang: Lang) {
    draw_app_header(
        c,
        tr(lang, "Games Arcade", "Gry Zręcznościowe"),
        "[Up/Down/1-3] Select  [Enter] Play  [Esc] Launcher",
    );
    let games = [
        ("1. Brick Breaker", app.cfg.high_score_brick),
        ("2. Tetris", app.cfg.high_score_tetris),
        (
            tr(lang, "3. Retro Racing", "3. Wyścigi Retro"),
            app.cfg.high_score_race,
        ),
    ];

    let mut y = 84;
    for (idx, (title, hs)) in games.iter().enumerate() {
        let sel = idx == app.game_menu_sel;
        c.draw_box(
            36,
            y,
            (APP_W - 72) as i32,
            78,
            PANEL,
            if sel { ACCENT } else { BORDER },
        );
        c.draw_text_scaled(56, y + 18, title, if sel { ACCENT } else { TEXT }, 1);
        let mut buf = [0u8; 32];
        let mut w = BufWriter::new(&mut buf);
        w.str(tr(lang, "High Score: ", "Rekord: "));
        w.u32(*hs);
        c.draw_text(56, y + 46, w.as_str(), OK_GREEN);
        y += 96;
    }
}

fn render_settings(c: &mut Canvas, app: &AppState, lang: Lang) {
    draw_app_header(
        c,
        tr(lang, "System Settings", "Ustawienia Systemu"),
        "[Up/Down] [Left/Right] Adjust  [R] Re-run Setup  [Esc]",
    );

    let mut vol_s = [0u8; 16];
    let mut vw = BufWriter::new(&mut vol_s);
    vw.u32(u32::from(app.cfg.volume));
    vw.str("%");

    let mut br_s = [0u8; 16];
    let mut bw = BufWriter::new(&mut br_s);
    bw.u32(u32::from(app.cfg.brightness));
    bw.str("%");

    let rows = [
        (
            tr(lang, "Language", "Język interfejsu"),
            if app.cfg.lang == Lang::Pl {
                "Polski"
            } else {
                "English"
            },
        ),
        (tr(lang, "Audio Volume", "Głośność Audio"), vw.as_str()),
        (
            tr(lang, "Audio Route", "Wyjście Audio"),
            if app.cfg.audio_route == 1 {
                "HDMI Digital"
            } else {
                "3.5mm Analog Jack"
            },
        ),
        (tr(lang, "Screen Brightness", "Jasność Ekranu"), bw.as_str()),
    ];

    let mut y = 84;
    for (idx, (label, val)) in rows.iter().enumerate() {
        let sel = idx == app.settings_sel;
        c.draw_box(
            36,
            y,
            (APP_W - 72) as i32,
            62,
            PANEL,
            if sel { ACCENT } else { BORDER },
        );
        c.draw_text(56, y + 22, label, if sel { ACCENT } else { TEXT });
        c.draw_text_right((APP_W - 56) as i32, y + 22, val, OK_GREEN);
        y += 76;
    }

    c.draw_text(
        36,
        406,
        tr(
            lang,
            "Press [R] to reset 1st Setup wizard flag on next Desktop launch.",
            "Naciśnij [R] aby przywrócić kreator 1st Setup przy następnym starcie.",
        ),
        MUTED,
    );
}

fn query_soc_temp_mc(fb_port: usize) -> Option<u32> {
    let req_pkt = FbPacket {
        shm_id: 0,
        width: 0,
        height: 0,
        pitch: 0,
        bpp: 0,
        shm_bytes: 0,
        soc_temp_mc: 0,
        brightness: 100,
        hw_active: 0,
        _pad: [0; 2],
        status: 0,
    };
    let req = pack_msg(MSG_FB_GET_INFO, &req_pkt);
    let mut reply = IpcMessage::new(0);
    libdelta::port_call(fb_port, &req, &mut reply).ok()?;
    let pkt = unpack_msg::<FbPacket>(&reply)?;
    if pkt.soc_temp_mc > 0 {
        Some(pkt.soc_temp_mc as u32)
    } else {
        None
    }
}
