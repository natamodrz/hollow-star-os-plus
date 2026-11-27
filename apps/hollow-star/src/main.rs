#![no_std]
#![no_main]

//! Hollow Star OS+ Desktop Compositor, `1st Setup` Wizard, and Core Apps (`HollowStar.hsapp/exec`).

mod calc;
mod font;
mod games;
mod gfx;
mod i18n;
mod icons;

use games::{GameId, GameState, Key as GameKey};
use gfx::{Canvas, SCREEN_H, SCREEN_W};
use i18n::{BufWriter, Lang, OsConfig, tr, wireless_enabled};
use icons::{IconKind, draw_icon_2x};
use libdelta::delta_abi::{
    AudioPacket, FbPacket, IpcMessage, MSG_AUDIO_PLAY_TONE, MSG_AUDIO_SET_ROUTE, MSG_AUDIO_SET_VOL,
    MSG_AUDIO_STATUS, MSG_AUDIO_STOP, MSG_FB_GET_INFO, MSG_FB_PRESENT, MSG_UART_READ, SVC_AUDIO,
    SVC_FB, SVC_UART, SVC_VFS, SysCaps, VfsPartitionStat, pack_msg, unpack_msg,
};

const FB_SHM_BYTES: usize = SCREEN_W * SCREEN_H * 4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    SetupLang,
    SetupTime,
    SetupWelcome,
    Launcher,
    SysInfo,
    NotesStage1,
    NotesEditor,
    Explorer,
    MediaPlayer,
    Calculator,
    GamesMenu,
    GamePlay,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputKey {
    Char(u8),
    Enter,
    Backspace,
    Tab,
    Esc,
    Up,
    Down,
    Left,
    Right,
    F1,
    F2,
    CtrlS,
    CtrlQ,
}

struct AppContext {
    uart: usize,
    vfs: usize,
    fb: usize,
    audio: usize,
    fb_shm_id: u64,
    canvas: Canvas,
    cfg: OsConfig,
    caps: SysCaps,
    fb_info: FbPacket,
    audio_info: AudioPacket,
    part_stats: [VfsPartitionStat; 4],
    part_count: usize,
    screen: Screen,
    help_open: bool,
    menu_idx: usize,
    launcher_notice: [u8; 48],
    launcher_notice_len: usize,
    discovered_apps: [u8; 96],
    discovered_len: usize,
    // Setup & Settings cursors
    setup_cursor: usize,
    // Explorer
    exp_path: [u8; 64],
    exp_path_len: usize,
    exp_buf: [u8; 1024],
    exp_len: usize,
    exp_status: [u8; 48],
    exp_status_len: usize,
    // Markdown Notes
    note_name: [u8; 24],
    note_name_len: usize,
    note_body: [u8; 512],
    note_body_len: usize,
    note_preview: bool,
    note_saved_flash: bool,
    // Calculator
    calc_expr: [u8; 48],
    calc_len: usize,
    calc_res: [u8; 32],
    calc_res_len: usize,
    calc_frac_mode: bool,
    // Media & Games
    media_track: usize,
    game: GameState,
    game_menu_idx: usize,
}

#[unsafe(no_mangle)]
fn delta_main() -> usize {
    let uart = libdelta::wait_for_service(SVC_UART);
    let vfs = libdelta::wait_for_service(SVC_VFS);
    let fb = libdelta::wait_for_service(SVC_FB);
    let audio = libdelta::wait_for_service(SVC_AUDIO);

    let Ok((fb_shm_id, fb_va)) = libdelta::shm_create(FB_SHM_BYTES) else {
        libdelta::print_via(uart, "[el0/HollowStar.hsapp] Failed to allocate FB Shm\n");
        return 1;
    };
    if libdelta::shm_allow_port(fb_shm_id, fb).is_err() {
        let _ = libdelta::shm_unmap(fb_shm_id);
        return 2;
    }

    // SAFETY: `fb_va` exclusively maps `FB_SHM_BYTES` (`640x480x4`) in our `TTBR0`.
    let canvas = unsafe { Canvas::from_shm(fb_va) };
    let mut cfg = OsConfig::defaults();
    cfg.load_from_vfs(vfs);
    let caps = libdelta::sys_caps().unwrap_or(SysCaps::empty());

    let mut ctx = AppContext {
        uart,
        vfs,
        fb,
        audio,
        fb_shm_id,
        canvas,
        cfg,
        caps,
        fb_info: query_fb_info(fb),
        audio_info: call_audio(audio, MSG_AUDIO_SET_VOL, 0, 0, cfg.volume, cfg.audio_route),
        part_stats: [unsafe { core::mem::zeroed() }; 4],
        part_count: 0,
        screen: if cfg.setup_done {
            Screen::Launcher
        } else {
            Screen::SetupLang
        },
        help_open: false,
        menu_idx: 0,
        launcher_notice: [0; 48],
        launcher_notice_len: 0,
        discovered_apps: [0; 96],
        discovered_len: 0,
        setup_cursor: 0,
        exp_path: [0; 64],
        exp_path_len: 1,
        exp_buf: [0; 1024],
        exp_len: 0,
        exp_status: [0; 48],
        exp_status_len: 0,
        note_name: [0; 24],
        note_name_len: 0,
        note_body: [0; 512],
        note_body_len: 0,
        note_preview: false,
        note_saved_flash: false,
        calc_expr: [0; 48],
        calc_len: 0,
        calc_res: [0; 32],
        calc_res_len: 0,
        calc_frac_mode: false,
        media_track: 0,
        game: GameState::new(GameId::Brick),
        game_menu_idx: 0,
    };
    ctx.exp_path[0] = b'/';
    ctx.set_default_note();
    ctx.refresh_storage();
    ctx.scan_hsapp_bundles();

    // Trigger RT Audio startup chime (verifies Priority 110 mixer during boot) & present first frame.
    ctx.audio_info = call_audio(
        audio,
        MSG_AUDIO_PLAY_TONE,
        659,
        25,
        cfg.volume,
        cfg.audio_route,
    );
    ctx.render();
    present_frame(&ctx);

    let mut msg = [0u8; 160];
    let mut w = BufWriter::new(&mut msg);
    w.str("[el0/HollowStar.hsapp] Compositor ");
    w.u32(ctx.fb_info.width);
    w.str("x");
    w.u32(ctx.fb_info.height);
    w.str(" online (RT audio mixed=");
    w.u32(ctx.audio_info.frames_mixed as u32);
    w.str(", mode=");
    w.str(if ctx.cfg.setup_done {
        "Desktop"
    } else {
        "1st_Setup"
    });
    w.str(")\n");
    libdelta::print_via(uart, w.as_str());
    libdelta::print_via(
        uart,
        "[boot] Milestone M6 (Multimedia, DeltaFS & UI) ready.\n",
    );

    ctx.event_loop();
    let _ = libdelta::shm_unmap(fb_shm_id);
    0
}

impl AppContext {
    fn set_default_note(&mut self) {
        let name = b"hello.md";
        self.note_name[..name.len()].copy_from_slice(name);
        self.note_name_len = name.len();
        let body = b"# Notes\nDeltaFS keeps notes safe.\n";
        self.note_body[..body.len()].copy_from_slice(body);
        self.note_body_len = body.len();
    }

    fn refresh_storage(&mut self) {
        self.caps = libdelta::sys_caps().unwrap_or(self.caps);
        self.fb_info = query_fb_info(self.fb);
        self.part_count = libdelta::vfs_statfs(self.vfs, &mut self.part_stats).unwrap_or(0);
    }

    fn scan_hsapp_bundles(&mut self) {
        let mut dir_buf = [0u8; 512];
        let mut out = BufWriter::new(&mut self.discovered_apps);
        let mut first = true;
        for dir_path in ["/system/apps", "/user/apps"] {
            if let Ok(n) = libdelta::vfs_readdir(self.vfs, dir_path, &mut dir_buf)
                && let Ok(text) = core::str::from_utf8(&dir_buf[..n])
            {
                for line in text.lines() {
                    if let Some(rest) = line.strip_prefix("[DIR]  ") {
                        let name = rest.trim_end_matches('/');
                        if name.ends_with(".hsapp") {
                            if !first {
                                out.str(", ");
                            }
                            out.str(name);
                            first = false;
                        }
                    }
                }
            }
        }
        self.discovered_len = out.pos;
    }

    fn set_launcher_notice(&mut self, msg: &str) {
        let b = msg.as_bytes();
        let n = b.len().min(self.launcher_notice.len());
        self.launcher_notice[..n].copy_from_slice(&b[..n]);
        self.launcher_notice_len = n;
    }

    fn launch_menu_item(&mut self, idx: usize) -> bool {
        match idx {
            0 => {
                self.refresh_storage();
                self.screen = Screen::SysInfo;
            }
            1 => self.screen = Screen::NotesStage1,
            2 => {
                self.load_explorer_dir();
                self.screen = Screen::Explorer;
            }
            3 => {
                self.audio_info = call_audio(self.audio, MSG_AUDIO_STATUS, 0, 0, 0, 0);
                self.screen = Screen::MediaPlayer;
            }
            4 => self.screen = Screen::Calculator,
            5 => self.screen = Screen::GamesMenu,
            6 => {
                self.setup_cursor = 0;
                self.screen = Screen::Settings;
            }
            7 => {
                if let Ok(pid) = libdelta::spawn_from_vfs("hello", "/system/apps/Hello.hsapp/exec")
                {
                    let _ = libdelta::wait_pid(pid);
                    self.set_launcher_notice("Hello.hsapp/exec OK");
                } else {
                    self.set_launcher_notice("Hello.hsapp launch ERR");
                }
            }
            _ => return true,
        }
        false
    }

    fn load_explorer_dir(&mut self) {
        let path = core::str::from_utf8(&self.exp_path[..self.exp_path_len]).unwrap_or("/");
        if let Ok(n) = libdelta::vfs_readdir(self.vfs, path, &mut self.exp_buf) {
            self.exp_len = n;
        } else {
            self.exp_len = 0;
        }
    }

    fn event_loop(&mut self) {
        let mut esc_state = 0u8;
        let mut esc_buf = [0u8; 8];
        let mut esc_len = 0usize;
        let mut esc_idle = 0u32;
        let mut tick_count = 0u32;

        loop {
            if let Some(byte) = read_uart_byte(self.uart) {
                esc_idle = 0;
                if esc_state == 1 && byte != b'[' && byte != b'O' {
                    esc_state = 0;
                    if self.handle_key(InputKey::Esc) {
                        return;
                    }
                    self.render();
                    present_frame(self);
                }
                if let Some(key) = decode_key(byte, &mut esc_state, &mut esc_buf, &mut esc_len) {
                    if self.handle_key(key) {
                        return;
                    }
                    self.render();
                    present_frame(self);
                }
            } else {
                if esc_state == 1 {
                    esc_idle += 1;
                    if esc_idle >= 32 {
                        esc_state = 0;
                        esc_idle = 0;
                        if self.handle_key(InputKey::Esc) {
                            return;
                        }
                        self.render();
                        present_frame(self);
                    }
                }
                tick_count = tick_count.wrapping_add(1);
                if self.screen == Screen::GamePlay
                    && !self.help_open
                    && tick_count.is_multiple_of(4)
                {
                    self.game.tick();
                    if self.game.game_over {
                        self.update_high_score();
                    }
                    self.render();
                    present_frame(self);
                }
                libdelta::yield_now();
            }
        }
    }

    fn update_high_score(&mut self) {
        let s = self.game.score;
        let changed = match self.game.active {
            GameId::Brick if s > self.cfg.high_score_brick => {
                self.cfg.high_score_brick = s;
                true
            }
            GameId::Tetris if s > self.cfg.high_score_tetris => {
                self.cfg.high_score_tetris = s;
                true
            }
            GameId::Racing if s > self.cfg.high_score_race => {
                self.cfg.high_score_race = s;
                true
            }
            _ => false,
        };
        if changed {
            self.cfg.save_to_vfs(self.vfs);
        }
    }

    fn handle_key(&mut self, key: InputKey) -> bool {
        if key == InputKey::CtrlQ {
            return true;
        }
        if key == InputKey::F1 {
            self.help_open = !self.help_open;
            return false;
        }
        if self.help_open {
            if key == InputKey::Esc || key == InputKey::Enter {
                self.help_open = false;
            }
            return false;
        }

        match self.screen {
            Screen::SetupLang => match key {
                InputKey::Up
                | InputKey::Down
                | InputKey::Left
                | InputKey::Right
                | InputKey::Tab => {
                    self.cfg.lang = if self.cfg.lang == Lang::En {
                        Lang::Pl
                    } else {
                        Lang::En
                    };
                }
                InputKey::Enter => {
                    // Per FEATURES_AND_UX.md: skip Wi-Fi step automatically on non-wireless boards.
                    self.screen = Screen::SetupTime;
                    self.setup_cursor = 0;
                }
                _ => {}
            },
            Screen::SetupTime => match key {
                InputKey::Tab | InputKey::Left | InputKey::Right => {
                    self.setup_cursor = 1 - self.setup_cursor;
                }
                InputKey::Up => {
                    if self.setup_cursor == 0 {
                        self.cfg.clock_hh = (self.cfg.clock_hh + 1) % 24;
                    } else {
                        self.cfg.clock_mm = (self.cfg.clock_mm + 1) % 60;
                    }
                }
                InputKey::Down => {
                    if self.setup_cursor == 0 {
                        self.cfg.clock_hh = (self.cfg.clock_hh + 23) % 24;
                    } else {
                        self.cfg.clock_mm = (self.cfg.clock_mm + 59) % 60;
                    }
                }
                InputKey::Enter => {
                    self.screen = Screen::SetupWelcome;
                }
                InputKey::Esc => self.screen = Screen::SetupLang,
                _ => {}
            },
            Screen::SetupWelcome => {
                if key == InputKey::Enter || key == InputKey::Char(b' ') {
                    self.cfg.setup_done = true;
                    self.cfg.save_to_vfs(self.vfs);
                    self.audio_info = call_audio(
                        self.audio,
                        MSG_AUDIO_PLAY_TONE,
                        880,
                        25,
                        self.cfg.volume,
                        self.cfg.audio_route,
                    );
                    self.screen = Screen::Launcher;
                }
            }
            Screen::Launcher => match key {
                InputKey::Left => self.menu_idx = (self.menu_idx + 8) % 9,
                InputKey::Right | InputKey::Tab => self.menu_idx = (self.menu_idx + 1) % 9,
                InputKey::Up => self.menu_idx = (self.menu_idx + 6) % 9,
                InputKey::Down => self.menu_idx = (self.menu_idx + 3) % 9,
                InputKey::Enter => {
                    if self.launch_menu_item(self.menu_idx) {
                        return true;
                    }
                }
                InputKey::Char(c) if (b'1'..=b'9').contains(&c) => {
                    let idx = usize::from(c - b'1');
                    self.menu_idx = idx;
                    if self.launch_menu_item(idx) {
                        return true;
                    }
                }
                InputKey::Char(b'q') | InputKey::Char(b'Q') => return true,
                _ => {}
            },
            Screen::SysInfo => match key {
                InputKey::Esc => self.screen = Screen::Launcher,
                InputKey::Char(b'e') | InputKey::Char(b'E') => {
                    let _ = libdelta::vfs_expand_user(self.vfs, 0);
                    self.refresh_storage();
                }
                InputKey::Char(b'r') | InputKey::Char(b'R') => self.refresh_storage(),
                _ => {}
            },
            Screen::NotesStage1 => match key {
                InputKey::Esc => self.screen = Screen::Launcher,
                InputKey::Backspace if self.note_name_len > 0 => self.note_name_len -= 1,
                InputKey::Char(c) if (0x21..0x7F).contains(&c) && self.note_name_len < 20 => {
                    self.note_name[self.note_name_len] = c;
                    self.note_name_len += 1;
                }
                InputKey::Enter => {
                    self.note_preview = false;
                    self.note_saved_flash = false;
                    self.screen = Screen::NotesEditor;
                }
                _ => {}
            },
            Screen::NotesEditor => match key {
                InputKey::Esc => self.screen = Screen::Launcher,
                InputKey::Tab => self.note_preview = !self.note_preview,
                InputKey::CtrlS => self.save_current_note(),
                InputKey::Backspace if !self.note_preview && self.note_body_len > 0 => {
                    self.note_body_len -= 1;
                    self.note_saved_flash = false;
                }
                InputKey::Enter if !self.note_preview && self.note_body_len < 500 => {
                    self.note_body[self.note_body_len] = b'\n';
                    self.note_body_len += 1;
                    self.note_saved_flash = false;
                }
                InputKey::Char(c)
                    if !self.note_preview
                        && (0x20..0x7F).contains(&c)
                        && self.note_body_len < 500 =>
                {
                    self.note_body[self.note_body_len] = c;
                    self.note_body_len += 1;
                    self.note_saved_flash = false;
                }
                _ => {}
            },
            Screen::Explorer => match key {
                InputKey::Esc => self.screen = Screen::Launcher,
                InputKey::Char(b'1') => self.set_exp_path("/boot"),
                InputKey::Char(b'2') => self.set_exp_path("/system"),
                InputKey::Char(b'3') => self.set_exp_path("/user"),
                InputKey::Char(b'4') => self.set_exp_path("/system/apps/Hello.hsapp"),
                InputKey::Char(b'h') | InputKey::Char(b'H') => {
                    let msg = if let Ok(pid) =
                        libdelta::spawn_from_vfs("hello", "/system/apps/Hello.hsapp/exec")
                    {
                        let _ = libdelta::wait_pid(pid);
                        "Launched Hello.hsapp/exec OK"
                    } else {
                        "Launch failed"
                    };
                    let mb = msg.as_bytes();
                    let n = mb.len().min(self.exp_status.len());
                    self.exp_status[..n].copy_from_slice(&mb[..n]);
                    self.exp_status_len = n;
                }
                _ => {}
            },
            Screen::MediaPlayer => match key {
                InputKey::Esc => self.screen = Screen::Launcher,
                InputKey::Char(b' ') | InputKey::Enter => {
                    const TRACK_NOTES: [u16; 4] = [440, 523, 659, 880];
                    let freq = TRACK_NOTES[self.media_track % TRACK_NOTES.len()];
                    self.media_track = (self.media_track + 1) % 4;
                    self.audio_info = call_audio(
                        self.audio,
                        MSG_AUDIO_PLAY_TONE,
                        freq,
                        40,
                        self.cfg.volume,
                        self.cfg.audio_route,
                    );
                }
                InputKey::Char(b's') | InputKey::Char(b'S') => {
                    self.audio_info = call_audio(self.audio, MSG_AUDIO_STOP, 0, 0, 0, 0);
                }
                InputKey::Up | InputKey::Right => {
                    self.cfg.volume = (self.cfg.volume + 5).min(100);
                    self.audio_info = call_audio(
                        self.audio,
                        MSG_AUDIO_SET_VOL,
                        0,
                        0,
                        self.cfg.volume,
                        self.cfg.audio_route,
                    );
                    self.cfg.save_to_vfs(self.vfs);
                }
                InputKey::Down | InputKey::Left => {
                    self.cfg.volume = self.cfg.volume.saturating_sub(5);
                    self.audio_info = call_audio(
                        self.audio,
                        MSG_AUDIO_SET_VOL,
                        0,
                        0,
                        self.cfg.volume,
                        self.cfg.audio_route,
                    );
                    self.cfg.save_to_vfs(self.vfs);
                }
                InputKey::Tab => {
                    self.cfg.audio_route = 1 - self.cfg.audio_route;
                    self.audio_info = call_audio(
                        self.audio,
                        MSG_AUDIO_SET_ROUTE,
                        0,
                        0,
                        self.cfg.volume,
                        self.cfg.audio_route,
                    );
                    self.cfg.save_to_vfs(self.vfs);
                }
                _ => {}
            },
            Screen::Calculator => match key {
                InputKey::Esc => self.screen = Screen::Launcher,
                InputKey::F2 => {
                    self.calc_frac_mode = !self.calc_frac_mode;
                    self.recalc();
                }
                InputKey::Backspace if self.calc_len > 0 => {
                    self.calc_len -= 1;
                    self.recalc();
                }
                InputKey::Enter => self.recalc(),
                InputKey::Char(c) if (0x20..0x7F).contains(&c) && self.calc_len < 44 => {
                    self.calc_expr[self.calc_len] = c;
                    self.calc_len += 1;
                    self.recalc();
                }
                _ => {}
            },
            Screen::GamesMenu => match key {
                InputKey::Esc => self.screen = Screen::Launcher,
                InputKey::Up => self.game_menu_idx = (self.game_menu_idx + 2) % 3,
                InputKey::Down => self.game_menu_idx = (self.game_menu_idx + 1) % 3,
                InputKey::Enter => {
                    let gid = match self.game_menu_idx {
                        0 => GameId::Brick,
                        1 => GameId::Tetris,
                        _ => GameId::Racing,
                    };
                    self.game.reset(gid);
                    self.screen = Screen::GamePlay;
                }
                _ => {}
            },
            Screen::GamePlay => match key {
                InputKey::Esc => self.screen = Screen::GamesMenu,
                InputKey::Left => self.game.on_key(GameKey::Left),
                InputKey::Right => self.game.on_key(GameKey::Right),
                InputKey::Up => self.game.on_key(GameKey::Up),
                InputKey::Down => self.game.on_key(GameKey::Down),
                InputKey::Char(b' ') | InputKey::Enter => self.game.on_key(GameKey::Action),
                _ => {}
            },
            Screen::Settings => match key {
                InputKey::Esc => self.screen = Screen::Launcher,
                InputKey::Up => self.setup_cursor = (self.setup_cursor + 4) % 5,
                InputKey::Down => self.setup_cursor = (self.setup_cursor + 1) % 5,
                InputKey::Left | InputKey::Right | InputKey::Enter => match self.setup_cursor {
                    0 => {
                        self.cfg.lang = if self.cfg.lang == Lang::En {
                            Lang::Pl
                        } else {
                            Lang::En
                        };
                        self.cfg.save_to_vfs(self.vfs);
                    }
                    1 => {
                        let delta = if key == InputKey::Left { -10i16 } else { 10 };
                        let next = (i16::from(self.cfg.brightness) + delta).clamp(20, 100) as u8;
                        self.cfg.brightness = next;
                        self.cfg.save_to_vfs(self.vfs);
                    }
                    2 => {
                        let delta = if key == InputKey::Left { -5i16 } else { 5 };
                        self.cfg.volume = (i16::from(self.cfg.volume) + delta).clamp(0, 100) as u8;
                        self.audio_info = call_audio(
                            self.audio,
                            MSG_AUDIO_SET_VOL,
                            0,
                            0,
                            self.cfg.volume,
                            self.cfg.audio_route,
                        );
                        self.cfg.save_to_vfs(self.vfs);
                    }
                    3 => {
                        let _ = libdelta::vfs_expand_user(self.vfs, 0);
                        self.refresh_storage();
                    }
                    _ => {
                        self.cfg.setup_done = false;
                        self.screen = Screen::SetupLang;
                    }
                },
                _ => {}
            },
        }
        false
    }

    fn set_exp_path(&mut self, p: &str) {
        let b = p.as_bytes();
        let n = b.len().min(self.exp_path.len());
        self.exp_path[..n].copy_from_slice(&b[..n]);
        self.exp_path_len = n;
        self.load_explorer_dir();
    }

    fn save_current_note(&mut self) {
        let _ = libdelta::vfs_mkdir(self.vfs, "/user/notes");
        let mut pbuf = [0u8; 48];
        let mut pw = BufWriter::new(&mut pbuf);
        pw.str("/user/notes/");
        let name = core::str::from_utf8(&self.note_name[..self.note_name_len]).unwrap_or("note.md");
        pw.str(name);
        if libdelta::vfs_write_file(self.vfs, pw.as_str(), &self.note_body[..self.note_body_len])
            .is_ok()
        {
            self.note_saved_flash = true;
        }
    }

    fn recalc(&mut self) {
        let expr = core::str::from_utf8(&self.calc_expr[..self.calc_len]).unwrap_or("");
        match calc::eval_expr(expr) {
            Ok(scaled) => {
                self.calc_res_len = if self.calc_frac_mode {
                    calc::format_fraction(scaled, &mut self.calc_res)
                } else {
                    calc::format_decimal(scaled, &mut self.calc_res)
                };
            }
            Err(msg) => {
                let b = msg.as_bytes();
                let n = b.len().min(self.calc_res.len());
                self.calc_res[..n].copy_from_slice(&b[..n]);
                self.calc_res_len = n;
            }
        }
    }

    fn render(&mut self) {
        self.canvas.clear(gfx::BG);
        self.draw_status_bar();

        match self.screen {
            Screen::SetupLang => self.draw_setup_lang(),
            Screen::SetupTime => self.draw_setup_time(),
            Screen::SetupWelcome => self.draw_setup_welcome(),
            Screen::Launcher => self.draw_launcher(),
            Screen::SysInfo => self.draw_sys_info(),
            Screen::NotesStage1 => self.draw_notes_stage1(),
            Screen::NotesEditor => self.draw_notes_editor(),
            Screen::Explorer => self.draw_explorer(),
            Screen::MediaPlayer => self.draw_media(),
            Screen::Calculator => self.draw_calculator(),
            Screen::GamesMenu => self.draw_games_menu(),
            Screen::GamePlay => {
                let hi = match self.game.active {
                    GameId::Brick => self.cfg.high_score_brick,
                    GameId::Tetris => self.cfg.high_score_tetris,
                    GameId::Racing => self.cfg.high_score_race,
                };
                self.game.draw(&mut self.canvas, self.cfg.lang, hi);
            }
            Screen::Settings => self.draw_settings(),
        }

        if self.help_open {
            self.draw_f1_help();
        }
    }

    fn draw_status_bar(&mut self) {
        self.canvas
            .draw_box(0, 0, SCREEN_W as i32, 28, gfx::PANEL, gfx::BORDER);
        self.canvas.draw_hollow_star(16, 14, 9, gfx::ACCENT);
        self.canvas.draw_text(
            32,
            10,
            concat!("HOLLOW STAR OS+ v", env!("CARGO_PKG_VERSION")),
            gfx::TEXT,
        );

        let mut right = [0u8; 64];
        let mut w = BufWriter::new(&mut right);
        w.str(if self.cfg.lang == Lang::En {
            "[EN] "
        } else {
            "[PL] "
        });
        w.str(if self.cfg.audio_route == 0 {
            "JACK:"
        } else {
            "HDMI:"
        });
        w.u32(u32::from(self.cfg.volume));
        w.str("% ");
        if wireless_enabled(&self.caps) {
            w.str("WIFI ");
        }
        w.u32_pad2(u32::from(self.cfg.clock_hh));
        w.str(":");
        w.u32_pad2(u32::from(self.cfg.clock_mm));
        w.str(" [F1]");
        self.canvas.draw_text(336, 10, w.as_str(), gfx::CYAN);
    }

    fn draw_setup_lang(&mut self) {
        let l = self.cfg.lang;
        self.canvas
            .draw_box(80, 64, 480, 340, gfx::PANEL, gfx::ACCENT);
        self.canvas.draw_hollow_star(320, 128, 34, gfx::ACCENT);
        self.canvas.draw_text_scaled(
            136,
            182,
            tr(l, "1st Setup: Language", "Pierwsza konfiguracja: Język"),
            gfx::TEXT,
            1,
        );
        let en_col = if l == Lang::En {
            gfx::ACCENT
        } else {
            gfx::BORDER
        };
        let pl_col = if l == Lang::Pl {
            gfx::ACCENT
        } else {
            gfx::BORDER
        };
        self.canvas.draw_box(160, 224, 320, 36, gfx::BG, en_col);
        self.canvas
            .draw_text(184, 238, "English (Default)", gfx::TEXT);
        self.canvas.draw_box(160, 272, 320, 36, gfx::BG, pl_col);
        self.canvas.draw_text(184, 286, "Polski", gfx::TEXT);
        self.canvas.draw_text(
            148,
            344,
            tr(
                l,
                "[Arrows/Tab] Switch   [Enter] Next",
                "[Strzałki/Tab] Zmień   [Enter] Dalej",
            ),
            gfx::MUTED,
        );
    }

    fn draw_setup_time(&mut self) {
        let l = self.cfg.lang;
        self.canvas
            .draw_box(80, 64, 480, 340, gfx::PANEL, gfx::ACCENT);
        self.canvas.draw_text(
            120,
            96,
            tr(
                l,
                "1st Setup (Step 2): System Clock",
                "Konfiguracja (Krok 2): Zegar",
            ),
            gfx::ACCENT,
        );
        let mut tbuf = [0u8; 16];
        let mut w = BufWriter::new(&mut tbuf);
        w.u32_pad2(u32::from(self.cfg.clock_hh));
        w.str(" : ");
        w.u32_pad2(u32::from(self.cfg.clock_mm));
        self.canvas.draw_box(220, 180, 200, 64, gfx::BG, gfx::CYAN);
        self.canvas
            .draw_text_scaled(264, 204, w.as_str(), gfx::TEXT, 2);
        self.canvas.draw_text(
            132,
            304,
            tr(
                l,
                "[Up/Down] Adjust  [Tab] HH/MM  [Enter] Next",
                "[Góra/Dół] Zmień  [Tab] GG/MM  [Enter] Dalej",
            ),
            gfx::MUTED,
        );
    }

    fn draw_setup_welcome(&mut self) {
        let l = self.cfg.lang;
        self.canvas
            .draw_box(80, 64, 480, 340, gfx::PANEL, gfx::ACCENT);
        self.canvas.draw_hollow_star(320, 168, 56, gfx::ACCENT);
        self.canvas
            .draw_text_scaled(240, 246, tr(l, "Welcome!", "Witaj!"), gfx::ACCENT, 2);
        self.canvas.draw_text(
            146,
            296,
            tr(
                l,
                "Hollow Star OS+ / Delta Microkernel ready.",
                "Hollow Star OS+ / Mikrojądro Delta gotowe.",
            ),
            gfx::TEXT,
        );
        self.canvas.draw_text(
            188,
            348,
            tr(l, "Press [Enter] to start", "Naciśnij [Enter], aby zacząć"),
            gfx::OK_GREEN,
        );
    }

    fn draw_launcher(&mut self) {
        let l = self.cfg.lang;
        let items: [(&str, &str, IconKind); 9] = [
            (
                tr(l, "1. Sys Info", "1. System"),
                "RAM/Temp/FS",
                IconKind::SysInfo,
            ),
            (
                tr(l, "2. Notes", "2. Notatki"),
                ".md Editor",
                IconKind::Notes,
            ),
            (
                tr(l, "3. Files", "3. Pliki"),
                "DeltaFS/FAT32",
                IconKind::Files,
            ),
            (
                tr(l, "4. Media RT", "4. Audio RT"),
                "Priority 110",
                IconKind::Media,
            ),
            (tr(l, "5. Calc", "5. Kalkul."), "+-*/^ sqrt", IconKind::Calc),
            (
                tr(l, "6. Games", "6. Gry"),
                "3 Retro Games",
                IconKind::Games,
            ),
            (
                tr(l, "7. Settings", "7. Opcje"),
                "EN/PL/Bright",
                IconKind::Settings,
            ),
            ("8. Hello.hsapp", ".hsapp Bundle", IconKind::Hsapp),
            (
                tr(l, "9. Shell", "9. Powłoka"),
                "delta> CLI",
                IconKind::Shell,
            ),
        ];

        self.canvas
            .draw_box(24, 40, 592, 420, gfx::PANEL, gfx::BORDER);
        self.canvas.draw_text(
            42,
            54,
            tr(
                l,
                "App Launcher (.hsapp Grid)",
                "Launcher aplikacji (Siatka .hsapp)",
            ),
            gfx::ACCENT,
        );

        let notice =
            core::str::from_utf8(&self.launcher_notice[..self.launcher_notice_len]).unwrap_or("");
        if !notice.is_empty() {
            self.canvas.draw_text(380, 54, notice, gfx::OK_GREEN);
        }

        for (idx, (title, sub, icon)) in items.iter().enumerate() {
            let col = (idx % 3) as i32;
            let row = (idx / 3) as i32;
            let x = 42 + col * 190;
            let y = 78 + row * 102;
            let selected = idx == self.menu_idx;
            let border = if selected { gfx::ACCENT } else { gfx::BORDER };
            let fill = if selected { gfx::ACCENT_DIM } else { gfx::BG };
            self.canvas.draw_box(x, y, 176, 88, fill, border);
            let icon_col = if selected { gfx::ACCENT } else { gfx::CYAN };
            draw_icon_2x(&mut self.canvas, x + 72, y + 12, *icon, icon_col);
            self.canvas.draw_text(x + 14, y + 52, title, gfx::TEXT);
            let sub_col = if selected { gfx::TEXT } else { gfx::MUTED };
            self.canvas.draw_text(x + 14, y + 68, sub, sub_col);
        }

        let disc = core::str::from_utf8(&self.discovered_apps[..self.discovered_len]).unwrap_or("");
        let mut dline = [0u8; 72];
        let mut dw = BufWriter::new(&mut dline);
        dw.str(".hsapp: ");
        dw.str(disc);
        self.canvas.draw_text(42, 396, dw.as_str(), gfx::CYAN);

        self.canvas.draw_text(
            42,
            426,
            tr(
                l,
                "[Arrows/1-9] Select  [Enter] Launch  [F1] Help  [Q] Shell",
                "[Strzałki/1-9] Wybór  [Enter] Uruchom  [F1] Pomoc  [Q] Powłoka",
            ),
            gfx::MUTED,
        );
    }

    fn draw_sys_info(&mut self) {
        let l = self.cfg.lang;
        self.canvas
            .draw_box(28, 44, 584, 412, gfx::PANEL, gfx::BORDER);
        self.canvas.draw_text(
            48,
            60,
            tr(l, "System Monitor & Storage", "Monitor systemu i partycji"),
            gfx::ACCENT,
        );

        let mut line = [0u8; 80];
        let mut w = BufWriter::new(&mut line);
        w.str("Board: ");
        w.str(self.caps.board_name_str());
        w.str(" | Cores: ");
        w.u32(u32::from(self.caps.cpu_count));
        self.canvas.draw_text(48, 92, w.as_str(), gfx::TEXT);

        let total_mb = (self.caps.ram_total_bytes / (1024 * 1024)) as u32;
        let free_mb = (self.caps.ram_free_bytes / (1024 * 1024)) as u32;
        let used_mb = total_mb.saturating_sub(free_mb);
        let temp_c = (self.fb_info.soc_temp_mc / 1000).max(0) as u32;
        let mut w2 = BufWriter::new(&mut line);
        w2.str("RAM: ");
        w2.u32(used_mb);
        w2.str(" / ");
        w2.u32(total_mb);
        w2.str(" MiB   SoC Temp: ");
        w2.u32(temp_c);
        w2.str(" C");
        self.canvas.draw_text(48, 120, w2.as_str(), gfx::CYAN);

        for i in 0..self.part_count {
            let st = &self.part_stats[i];
            let m = core::str::from_utf8(&st.mount[..st.mount_len as usize]).unwrap_or("?");
            let f = core::str::from_utf8(&st.fs_type[..st.fs_len as usize]).unwrap_or("?");
            let tot_k = (st.total_bytes / 1024) as u32;
            let free_k = (st.free_bytes / 1024) as u32;
            let mut pw = BufWriter::new(&mut line);
            pw.str(m);
            pw.str(" [");
            pw.str(f);
            pw.str("] Total=");
            pw.u32(tot_k);
            pw.str(" KiB  Free=");
            pw.u32(free_k);
            pw.str(" KiB");
            self.canvas
                .draw_box(48, 160 + (i as i32) * 44, 544, 34, gfx::BG, gfx::BORDER);
            self.canvas
                .draw_text(60, 173 + (i as i32) * 44, pw.as_str(), gfx::OK_GREEN);
        }

        self.canvas.draw_text(
            48,
            428,
            tr(
                l,
                "[E] Expand /user to SD end  [R] Refresh  [Esc] Back",
                "[E] Rozszerz /user do końca SD  [R] Odśwież  [Esc] Wróć",
            ),
            gfx::MUTED,
        );
    }

    fn draw_notes_stage1(&mut self) {
        let l = self.cfg.lang;
        self.canvas
            .draw_box(60, 72, 520, 320, gfx::PANEL, gfx::BORDER);
        self.canvas.draw_text(
            84,
            96,
            tr(
                l,
                "Markdown Notes - New File (/user/notes)",
                "Notatki Markdown - Nowy plik (/user/notes)",
            ),
            gfx::ACCENT,
        );
        self.canvas.draw_box(84, 152, 472, 40, gfx::BG, gfx::CYAN);
        let name = core::str::from_utf8(&self.note_name[..self.note_name_len]).unwrap_or("");
        self.canvas.draw_text(100, 168, name, gfx::TEXT);
        self.canvas.draw_text(
            84,
            240,
            tr(
                l,
                "[Type filename]  [Enter] Open Editor  [Esc] Back",
                "[Wpisz nazwę]  [Enter] Otwórz edytor  [Esc] Wróć",
            ),
            gfx::MUTED,
        );
    }

    fn draw_notes_editor(&mut self) {
        let l = self.cfg.lang;
        self.canvas
            .draw_box(28, 44, 584, 412, gfx::PANEL, gfx::BORDER);
        let hdr = if self.note_preview {
            tr(
                l,
                "Markdown Preview (Tab: Edit)",
                "Podgląd Markdown (Tab: Edycja)",
            )
        } else if self.note_saved_flash {
            tr(
                l,
                "Saved to DeltaFS! (Ctrl+S: Save)",
                "Zapisano w DeltaFS! (Ctrl+S: Zapisz)",
            )
        } else {
            tr(
                l,
                "Markdown Editor (Ctrl+S: Save, Tab: Preview)",
                "Edytor Markdown (Ctrl+S: Zapisz, Tab: Podgląd)",
            )
        };
        self.canvas.draw_text(48, 60, hdr, gfx::ACCENT);
        self.canvas.draw_box(48, 88, 544, 320, gfx::BG, gfx::BORDER);
        let text = core::str::from_utf8(&self.note_body[..self.note_body_len]).unwrap_or("");
        let mut y = 102i32;
        for line in text.lines() {
            let color = if self.note_preview && line.starts_with('#') {
                gfx::ACCENT
            } else {
                gfx::TEXT
            };
            self.canvas.draw_text(60, y, line, color);
            y += 14;
            if y > 390 {
                break;
            }
        }
        self.canvas.draw_text(48, 428, "[Esc] Back", gfx::MUTED);
    }

    fn draw_explorer(&mut self) {
        let l = self.cfg.lang;
        self.canvas
            .draw_box(28, 44, 584, 412, gfx::PANEL, gfx::BORDER);
        let path = core::str::from_utf8(&self.exp_path[..self.exp_path_len]).unwrap_or("/");
        self.canvas.draw_text(48, 60, path, gfx::ACCENT);
        self.canvas.draw_box(48, 88, 544, 290, gfx::BG, gfx::BORDER);

        let text = core::str::from_utf8(&self.exp_buf[..self.exp_len]).unwrap_or("");
        let mut y = 100i32;
        for line in text.lines() {
            self.canvas.draw_text(60, y, line, gfx::TEXT);
            y += 14;
            if y > 362 {
                break;
            }
        }
        let st = core::str::from_utf8(&self.exp_status[..self.exp_status_len]).unwrap_or("");
        if !st.is_empty() {
            self.canvas.draw_text(48, 392, st, gfx::OK_GREEN);
        }
        self.canvas.draw_text(
            48,
            428,
            tr(
                l,
                "[1]/boot [2]/system [3]/user [4].hsapp [H]Run Hello [Esc]Back",
                "[1]/boot [2]/system [3]/user [4].hsapp [H]Uruchom Hello [Esc]Wróć",
            ),
            gfx::MUTED,
        );
    }

    fn draw_media(&mut self) {
        let l = self.cfg.lang;
        self.canvas
            .draw_box(48, 52, 544, 384, gfx::PANEL, gfx::BORDER);
        self.canvas.draw_text(
            72,
            72,
            tr(
                l,
                "Delta RT Audio Player (Priority 110)",
                "Odtwarzacz Audio Delta RT (Priorytet 110)",
            ),
            gfx::ACCENT,
        );

        let mut buf = [0u8; 64];
        let mut w = BufWriter::new(&mut buf);
        w.str("Route: ");
        w.str(if self.cfg.audio_route == 0 {
            tr(l, "3.5mm Wired Jack (Auto)", "Jack 3.5mm (Auto)")
        } else {
            "HDMI Audio"
        });
        w.str("  |  Vol: ");
        w.u32(u32::from(self.cfg.volume));
        w.str("%");
        self.canvas.draw_text(72, 114, w.as_str(), gfx::TEXT);

        // Volume bar
        self.canvas.draw_box(72, 144, 400, 20, gfx::BG, gfx::BORDER);
        self.canvas.fill_rect(
            74,
            146,
            i32::from(self.cfg.volume) * 396 / 100,
            16,
            gfx::OK_GREEN,
        );

        // Visualizer bars
        for i in 0..16 {
            let h = if self.audio_info.playing != 0 {
                24 + (((i * 19 + self.media_track * 17) % 64) as i32)
            } else {
                8
            };
            self.canvas
                .fill_rect(72 + (i as i32) * 28, 300 - h, 20, h, gfx::CYAN);
        }

        let mut sw = BufWriter::new(&mut buf);
        sw.str("RT Frames Mixed: ");
        sw.u32(self.audio_info.frames_mixed as u32);
        sw.str("  Freq: ");
        sw.u32(u32::from(self.audio_info.freq_hz));
        sw.str(" Hz");
        self.canvas.draw_text(72, 332, sw.as_str(), gfx::ACCENT);

        self.canvas.draw_text(
            72,
            396,
            tr(
                l,
                "[Space] Play Note  [S] Stop  [Up/Down] Vol  [Tab] Route",
                "[Spacja] Graj  [S] Stop  [Góra/Dół] Głośność  [Tab] Wyjście",
            ),
            gfx::MUTED,
        );
    }

    fn draw_calculator(&mut self) {
        let l = self.cfg.lang;
        self.canvas
            .draw_box(60, 64, 520, 340, gfx::PANEL, gfx::BORDER);
        self.canvas.draw_text(
            84,
            88,
            tr(l, "Expression Calculator", "Kalkulator wyrażeń"),
            gfx::ACCENT,
        );
        self.canvas.draw_box(84, 132, 472, 44, gfx::BG, gfx::CYAN);
        let expr = core::str::from_utf8(&self.calc_expr[..self.calc_len]).unwrap_or("");
        self.canvas.draw_text(98, 150, expr, gfx::TEXT);

        self.canvas.draw_box(84, 196, 472, 56, gfx::BG, gfx::BORDER);
        let res = core::str::from_utf8(&self.calc_res[..self.calc_res_len]).unwrap_or("0");
        self.canvas.draw_text_scaled(98, 214, res, gfx::OK_GREEN, 2);

        self.canvas.draw_text(
            84,
            310,
            tr(
                l,
                "Supports: + - * / ^ ( ) sqrt()   [F2] Fraction/Dec",
                "Obsługa: + - * / ^ ( ) sqrt()   [F2] Ułamek/Dziesiętny",
            ),
            gfx::MUTED,
        );
    }

    fn draw_games_menu(&mut self) {
        let l = self.cfg.lang;
        self.canvas
            .draw_box(120, 80, 400, 300, gfx::PANEL, gfx::BORDER);
        self.canvas.draw_text(
            148,
            108,
            tr(l, "Classic Games", "Klasyczne Gry"),
            gfx::ACCENT,
        );
        let titles = [
            "1. Brick Breaker",
            "2. Tetris",
            tr(l, "3. Racing", "3. Wyścigi"),
        ];
        for (i, t) in titles.iter().enumerate() {
            let y = 152 + (i as i32) * 52;
            let b = if i == self.game_menu_idx {
                gfx::ACCENT
            } else {
                gfx::BORDER
            };
            self.canvas.draw_box(148, y, 344, 38, gfx::BG, b);
            self.canvas.draw_text(168, y + 14, t, gfx::TEXT);
        }
    }

    fn draw_settings(&mut self) {
        let l = self.cfg.lang;
        self.canvas
            .draw_box(48, 52, 544, 384, gfx::PANEL, gfx::BORDER);
        self.canvas
            .draw_text(72, 74, tr(l, "Settings", "Ustawienia"), gfx::ACCENT);
        let mut buf = [0u8; 48];
        let mut rows: [&str; 5] = ["", "", "", "", ""];
        rows[0] = if l == Lang::En {
            "Language: English"
        } else {
            "Język: Polski"
        };
        let mut b2 = [0u8; 48];
        let mut w2 = BufWriter::new(&mut b2);
        w2.str(tr(l, "Brightness: ", "Jasność: "));
        w2.u32(u32::from(self.cfg.brightness));
        w2.str("%");
        let r1 = w2.as_str();
        let mut w3 = BufWriter::new(&mut buf);
        w3.str(tr(l, "Volume: ", "Głośność: "));
        w3.u32(u32::from(self.cfg.volume));
        w3.str("%");
        let r2 = w3.as_str();
        rows[1] = r1;
        rows[2] = r2;
        rows[3] = tr(
            l,
            "Expand /user DeltaFS to SD end",
            "Rozszerz /user DeltaFS do końca SD",
        );
        rows[4] = tr(l, "Re-run 1st Setup Wizard", "Uruchom ponownie 1st Setup");

        for (i, txt) in rows.iter().enumerate() {
            let y = 114 + (i as i32) * 46;
            let border = if i == self.setup_cursor {
                gfx::ACCENT
            } else {
                gfx::BORDER
            };
            self.canvas.draw_box(72, y, 496, 36, gfx::BG, border);
            self.canvas.draw_text(90, y + 14, txt, gfx::TEXT);
        }
        self.canvas.draw_text(
            72,
            396,
            tr(
                l,
                "[Left/Right/Enter] Change  [Esc] Back (Wireless hidden: N/A)",
                "[Lewo/Prawo/Enter] Zmień  [Esc] Wróć",
            ),
            gfx::MUTED,
        );
    }

    fn draw_f1_help(&mut self) {
        let l = self.cfg.lang;
        self.canvas.draw_box(96, 96, 448, 280, gfx::BG, gfx::ACCENT);
        self.canvas.draw_text_scaled(
            124,
            120,
            tr(l, "F1 HELP / CONTROLS", "F1 POMOC / STEROWANIE"),
            gfx::ACCENT,
            1,
        );
        self.canvas.draw_text(
            124,
            160,
            tr(
                l,
                "Arrows / Tab : Navigate / Switch",
                "Strzałki / Tab : Nawigacja / Przełączanie",
            ),
            gfx::TEXT,
        );
        self.canvas.draw_text(
            124,
            184,
            tr(
                l,
                "Enter / Space: Confirm / Action",
                "Enter / Spacja : Zatwierdź / Akcja",
            ),
            gfx::TEXT,
        );
        self.canvas.draw_text(
            124,
            208,
            tr(
                l,
                "Esc          : Back / Close",
                "Esc            : Wróć / Zamknij",
            ),
            gfx::TEXT,
        );
        self.canvas.draw_text(
            124,
            232,
            tr(
                l,
                "Ctrl+S / F2  : Save Note / Calc Fraction",
                "Ctrl+S / F2    : Zapisz notatkę / Ułamek",
            ),
            gfx::TEXT,
        );
        self.canvas.draw_text(
            124,
            256,
            tr(
                l,
                "Ctrl+Q / Q   : Return to Unix Shell",
                "Ctrl+Q / Q     : Powrót do powłoki Unix",
            ),
            gfx::TEXT,
        );
        self.canvas.draw_text(
            124,
            330,
            tr(
                l,
                "Press [F1] or [Esc] to resume",
                "Naciśnij [F1] lub [Esc], aby zamknąć",
            ),
            gfx::OK_GREEN,
        );
    }
}

fn query_fb_info(fb: usize) -> FbPacket {
    let mut pkt = FbPacket {
        shm_id: 0,
        width: SCREEN_W as u32,
        height: SCREEN_H as u32,
        pitch: (SCREEN_W * 4) as u32,
        bpp: 32,
        shm_bytes: FB_SHM_BYTES as u32,
        soc_temp_mc: 42_000,
        brightness: 100,
        hw_active: 0,
        _pad: [0; 2],
        status: 0,
    };
    let req = pack_msg(MSG_FB_GET_INFO, &pkt);
    let mut reply = IpcMessage::new(0);
    if libdelta::port_call(fb, &req, &mut reply).is_ok()
        && let Some(rp) = unpack_msg::<FbPacket>(&reply)
    {
        pkt = rp;
    }
    pkt
}

fn present_frame(ctx: &AppContext) {
    let pkt = FbPacket {
        shm_id: ctx.fb_shm_id,
        width: SCREEN_W as u32,
        height: SCREEN_H as u32,
        pitch: ctx.fb_info.pitch,
        bpp: 32,
        shm_bytes: FB_SHM_BYTES as u32,
        soc_temp_mc: 0,
        brightness: ctx.cfg.brightness,
        hw_active: ctx.fb_info.hw_active,
        _pad: [0; 2],
        status: 0,
    };
    let req = pack_msg(MSG_FB_PRESENT, &pkt);
    let mut reply = IpcMessage::new(0);
    let _ = libdelta::port_call(ctx.fb, &req, &mut reply);
}

fn call_audio(
    audio: usize,
    tag: u64,
    freq_hz: u16,
    duration_ms: u16,
    volume: u8,
    route: u8,
) -> AudioPacket {
    let mut pkt = AudioPacket {
        shm_id: 0,
        frames_mixed: 0,
        sample_count: 0,
        freq_hz,
        duration_ms,
        volume,
        route,
        playing: 0,
        jack_plugged: 1,
        rt_priority: 110,
        _pad: [0; 3],
        status: 0,
    };
    let req = pack_msg(tag, &pkt);
    let mut reply = IpcMessage::new(0);
    if libdelta::port_call(audio, &req, &mut reply).is_ok()
        && let Some(rp) = unpack_msg::<AudioPacket>(&reply)
    {
        pkt = rp;
    }
    pkt
}

fn read_uart_byte(uart: usize) -> Option<u8> {
    let req = IpcMessage::new(MSG_UART_READ);
    let mut reply = IpcMessage::new(0);
    if libdelta::port_call(uart, &req, &mut reply).is_ok() && reply.len > 0 {
        return Some(reply.data[0]);
    }
    None
}

fn decode_key(
    b: u8,
    esc_state: &mut u8,
    esc_buf: &mut [u8; 8],
    esc_len: &mut usize,
) -> Option<InputKey> {
    if *esc_state == 0 {
        return match b {
            0x1B => {
                *esc_state = 1;
                *esc_len = 0;
                None
            }
            0x13 => Some(InputKey::CtrlS),
            0x11 => Some(InputKey::CtrlQ),
            b'\r' | b'\n' => Some(InputKey::Enter),
            0x08 | 0x7F => Some(InputKey::Backspace),
            b'\t' => Some(InputKey::Tab),
            b'~' => Some(InputKey::F1),
            other => Some(InputKey::Char(other)),
        };
    }
    if *esc_state == 1 {
        if b == b'[' || b == b'O' {
            *esc_state = 2;
            esc_buf[0] = b;
            *esc_len = 1;
            return None;
        }
        *esc_state = 0;
        return Some(InputKey::Esc);
    }
    if *esc_len < esc_buf.len() {
        esc_buf[*esc_len] = b;
        *esc_len += 1;
    }
    let slice = &esc_buf[..*esc_len];
    if slice == b"[A" || slice == b"OA" {
        *esc_state = 0;
        return Some(InputKey::Up);
    }
    if slice == b"[B" || slice == b"OB" {
        *esc_state = 0;
        return Some(InputKey::Down);
    }
    if slice == b"[C" || slice == b"OC" {
        *esc_state = 0;
        return Some(InputKey::Right);
    }
    if slice == b"[D" || slice == b"OD" {
        *esc_state = 0;
        return Some(InputKey::Left);
    }
    if slice == b"OP" || slice == b"[11~" {
        *esc_state = 0;
        return Some(InputKey::F1);
    }
    if slice == b"OQ" || slice == b"[12~" {
        *esc_state = 0;
        return Some(InputKey::F2);
    }
    if b.is_ascii_alphabetic() || b == b'~' || *esc_len >= 6 {
        *esc_state = 0;
    }
    None
}
