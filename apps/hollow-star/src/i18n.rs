//! Bilingual localization (`English` default + `Polski`) and `/user/config/os.cfg` persistence.

use libdelta::delta_abi::SysCaps;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En = 0,
    Pl = 1,
}

#[derive(Clone, Copy)]
pub struct OsConfig {
    pub setup_done: bool,
    pub lang: Lang,
    pub clock_hh: u8,
    pub clock_mm: u8,
    pub volume: u8,
    pub brightness: u8,
    pub audio_route: u8,
    pub high_score_brick: u32,
    pub high_score_tetris: u32,
    pub high_score_race: u32,
}

impl OsConfig {
    pub const fn defaults() -> Self {
        Self {
            setup_done: false,
            lang: Lang::En,
            clock_hh: 12,
            clock_mm: 0,
            volume: 80,
            brightness: 100,
            audio_route: 0,
            high_score_brick: 0,
            high_score_tetris: 0,
            high_score_race: 0,
        }
    }

    pub fn load_from_vfs(&mut self, vfs: usize) {
        let mut buf = [0u8; 256];
        let Ok(n) = libdelta::vfs_read_file(vfs, "/user/config/os.cfg", &mut buf) else {
            return;
        };
        let Ok(text) = core::str::from_utf8(&buf[..n]) else {
            return;
        };
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "setup_done" => self.setup_done = v.trim() == "1",
                    "lang" => {
                        self.lang = if v.trim() == "pl" { Lang::Pl } else { Lang::En };
                    }
                    "hh" => self.clock_hh = parse_u32(v.trim()).min(23) as u8,
                    "mm" => self.clock_mm = parse_u32(v.trim()).min(59) as u8,
                    "vol" => self.volume = parse_u32(v.trim()).min(100) as u8,
                    "bright" => self.brightness = parse_u32(v.trim()).clamp(20, 100) as u8,
                    "route" => self.audio_route = parse_u32(v.trim()).min(1) as u8,
                    "hs_brick" => self.high_score_brick = parse_u32(v.trim()),
                    "hs_tetris" => self.high_score_tetris = parse_u32(v.trim()),
                    "hs_race" => self.high_score_race = parse_u32(v.trim()),
                    _ => {}
                }
            }
        }
    }

    pub fn save_to_vfs(&self, vfs: usize) {
        let _ = libdelta::vfs_mkdir(vfs, "/user/config");
        let mut buf = [0u8; 256];
        let mut fmt = BufWriter::new(&mut buf);
        fmt.str("setup_done=");
        fmt.str(if self.setup_done { "1\n" } else { "0\n" });
        fmt.str("lang=");
        fmt.str(if self.lang == Lang::Pl {
            "pl\n"
        } else {
            "en\n"
        });
        fmt.str("hh=");
        fmt.u32(u32::from(self.clock_hh));
        fmt.str("\nmm=");
        fmt.u32(u32::from(self.clock_mm));
        fmt.str("\nvol=");
        fmt.u32(u32::from(self.volume));
        fmt.str("\nbright=");
        fmt.u32(u32::from(self.brightness));
        fmt.str("\nroute=");
        fmt.u32(u32::from(self.audio_route));
        fmt.str("\nhs_brick=");
        fmt.u32(self.high_score_brick);
        fmt.str("\nhs_tetris=");
        fmt.u32(self.high_score_tetris);
        fmt.str("\nhs_race=");
        fmt.u32(self.high_score_race);
        fmt.str("\n");
        let _ = libdelta::vfs_write_file(vfs, "/user/config/os.cfg", fmt.as_bytes());
    }
}

pub fn tr<'a>(lang: Lang, en: &'a str, pl: &'a str) -> &'a str {
    match lang {
        Lang::En => en,
        Lang::Pl => pl,
    }
}

pub fn wireless_enabled(caps: &SysCaps) -> bool {
    caps.has_wifi != 0 || caps.has_bluetooth != 0
}

pub struct BufWriter<'a> {
    buf: &'a mut [u8],
    pub pos: usize,
}

impl<'a> BufWriter<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn str(&mut self, s: &str) {
        let n = s.len().min(self.buf.len().saturating_sub(self.pos));
        self.buf[self.pos..self.pos + n].copy_from_slice(&s.as_bytes()[..n]);
        self.pos += n;
    }

    pub fn u32(&mut self, mut val: u32) {
        if val == 0 {
            self.str("0");
            return;
        }
        let mut digits = [0u8; 10];
        let mut i = digits.len();
        while val > 0 && i > 0 {
            i -= 1;
            digits[i] = b'0' + (val % 10) as u8;
            val /= 10;
        }
        if let Ok(s) = core::str::from_utf8(&digits[i..]) {
            self.str(s);
        }
    }

    pub fn u32_pad2(&mut self, val: u32) {
        if val < 10 {
            self.str("0");
        }
        self.u32(val);
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.pos]).unwrap_or("")
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.pos]
    }
}

fn parse_u32(s: &str) -> u32 {
    let mut acc = 0u32;
    for b in s.as_bytes() {
        if b.is_ascii_digit() {
            acc = acc.saturating_mul(10).saturating_add(u32::from(b - b'0'));
        } else {
            break;
        }
    }
    acc
}
