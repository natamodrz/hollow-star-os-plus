//! Classic retro games (`Brick Breaker`, `Tetris`, `Racing`) rendered in `640x452` app surface.

use libdelta_ui::gfx::{self, APP_W, Canvas};
use libdelta_ui::{BufWriter, Lang, tr};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GameId {
    Brick,
    Tetris,
    Racing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Action,
}

pub struct GameState {
    pub active: GameId,
    pub score: u32,
    pub game_over: bool,
    // Brick Breaker
    paddle_x: i32,
    ball_x: i32,
    ball_y: i32,
    ball_vx: i32,
    ball_vy: i32,
    bricks: [[bool; 10]; 4],
    // Tetris
    board: [[u8; 10]; 16],
    piece_x: i32,
    piece_y: i32,
    piece_kind: u8,
    tick_ctr: u32,
    // Racing
    car_lane: i32,
    obs_lane: i32,
    obs_y: i32,
    rng: u32,
}

impl GameState {
    pub const fn new(active: GameId) -> Self {
        Self {
            active,
            score: 0,
            game_over: false,
            paddle_x: 280,
            ball_x: 320,
            ball_y: 310,
            ball_vx: 3,
            ball_vy: -3,
            bricks: [[true; 10]; 4],
            board: [[0; 10]; 16],
            piece_x: 4,
            piece_y: 0,
            piece_kind: 0,
            tick_ctr: 0,
            car_lane: 1,
            obs_lane: 0,
            obs_y: 66,
            rng: 0x1234_5678,
        }
    }

    pub fn reset(&mut self, id: GameId) {
        *self = Self::new(id);
    }

    fn next_rand(&mut self) -> u32 {
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        self.rng >> 16
    }

    pub fn on_key(&mut self, key: Key) {
        if self.game_over && key == Key::Action {
            let id = self.active;
            self.reset(id);
            return;
        }
        match self.active {
            GameId::Brick => match key {
                Key::Left => self.paddle_x = (self.paddle_x - 24).max(70),
                Key::Right => self.paddle_x = (self.paddle_x + 24).min(510),
                _ => {}
            },
            GameId::Tetris => match key {
                Key::Left => self.piece_x = (self.piece_x - 1).max(0),
                Key::Right => self.piece_x = (self.piece_x + 1).min(7),
                Key::Down | Key::Action => self.step_tetris(),
                Key::Up => self.piece_kind = (self.piece_kind + 1) % 3,
            },
            GameId::Racing => match key {
                Key::Left if self.car_lane > 0 => self.car_lane -= 1,
                Key::Right if self.car_lane < 2 => self.car_lane += 1,
                _ => {}
            },
        }
    }

    pub fn tick(&mut self) {
        if self.game_over {
            return;
        }
        self.tick_ctr += 1;
        match self.active {
            GameId::Brick => self.step_brick(),
            GameId::Tetris if self.tick_ctr.is_multiple_of(8) => self.step_tetris(),
            GameId::Racing => self.step_racing(),
            _ => {}
        }
    }

    fn step_brick(&mut self) {
        self.ball_x += self.ball_vx;
        self.ball_y += self.ball_vy;
        if self.ball_x <= 64 || self.ball_x >= 568 {
            self.ball_vx = -self.ball_vx;
        }
        if self.ball_y <= 64 {
            self.ball_vy = self.ball_vy.abs();
        }
        if self.ball_y >= 384
            && self.ball_y <= 398
            && self.ball_x >= self.paddle_x
            && self.ball_x <= self.paddle_x + 64
        {
            self.ball_vy = -self.ball_vy.abs();
        }
        if self.ball_y > 416 {
            self.game_over = true;
            return;
        }
        for row in 0..4 {
            for col in 0..10 {
                if !self.bricks[row][col] {
                    continue;
                }
                let bx = 70 + (col as i32) * 50;
                let by = 80 + (row as i32) * 20;
                if self.ball_x >= bx
                    && self.ball_x <= bx + 46
                    && self.ball_y >= by
                    && self.ball_y <= by + 14
                {
                    self.bricks[row][col] = false;
                    self.ball_vy = -self.ball_vy;
                    self.score += 10;
                }
            }
        }
    }

    fn step_tetris(&mut self) {
        let ny = self.piece_y + 1;
        let collides = ny >= 15 || self.board[ny as usize][self.piece_x as usize] != 0;
        if collides {
            let py = self.piece_y.clamp(0, 15) as usize;
            let px = self.piece_x.clamp(0, 8) as usize;
            self.board[py][px] = self.piece_kind + 1;
            self.board[py][px + 1] = self.piece_kind + 1;
            if py == 0 {
                self.game_over = true;
                return;
            }
            for r in 0..16 {
                if self.board[r].iter().all(|&c| c != 0) {
                    for rr in (1..=r).rev() {
                        self.board[rr] = self.board[rr - 1];
                    }
                    self.board[0] = [0; 10];
                    self.score += 50;
                }
            }
            self.piece_y = 0;
            self.piece_x = (self.next_rand() % 7) as i32 + 1;
            self.piece_kind = (self.next_rand() % 3) as u8;
            self.score += 5;
        } else {
            self.piece_y = ny;
        }
    }

    fn step_racing(&mut self) {
        self.obs_y += 6;
        if self.obs_y > 400 {
            self.obs_y = 66;
            self.obs_lane = (self.next_rand() % 3) as i32;
            self.score += 15;
        }
        if self.obs_y >= 332 && self.obs_y <= 378 && self.obs_lane == self.car_lane {
            self.game_over = true;
        }
    }

    pub fn draw(&self, canvas: &mut Canvas, lang: Lang, high_score: u32) {
        canvas.draw_box(36, 16, (APP_W - 72) as i32, 416, gfx::PANEL, gfx::BORDER);

        let mut hdr = [0u8; 64];
        let mut w = BufWriter::new(&mut hdr);
        let title = match self.active {
            GameId::Brick => "BRICK BREAKER",
            GameId::Tetris => "TETRIS",
            GameId::Racing => tr(lang, "RACING", "WYŚCIGI"),
        };
        w.str(title);
        w.str("  |  ");
        w.str(tr(lang, "Score: ", "Wynik: "));
        w.u32(self.score);
        w.str("  ");
        w.str(tr(lang, "Best: ", "Rekord: "));
        w.u32(high_score.max(self.score));
        canvas.draw_text(52, 28, w.as_str(), gfx::ACCENT);
        canvas.draw_text_right((APP_W - 52) as i32, 28, "[Esc] Back", gfx::MUTED);

        match self.active {
            GameId::Brick => {
                let colors = [gfx::WARN_RED, gfx::ACCENT, gfx::OK_GREEN, gfx::CYAN];
                for (row, &row_color) in colors.iter().enumerate() {
                    for (col, &alive) in self.bricks[row].iter().enumerate() {
                        if alive {
                            canvas.fill_rect(
                                70 + (col as i32) * 50,
                                80 + (row as i32) * 20,
                                46,
                                14,
                                row_color,
                            );
                        }
                    }
                }
                canvas.fill_rect(self.paddle_x, 392, 64, 10, gfx::TEXT);
                canvas.fill_rect(self.ball_x, self.ball_y, 8, 8, gfx::ACCENT);
            }
            GameId::Tetris => {
                canvas.draw_box(218, 66, 204, 324, gfx::BG, gfx::BORDER);
                let palette = [gfx::BORDER, gfx::CYAN, gfx::ACCENT, gfx::OK_GREEN];
                for (r, row_cells) in self.board.iter().enumerate() {
                    for (c, &cell) in row_cells.iter().enumerate() {
                        if cell != 0 {
                            canvas.fill_rect(
                                220 + (c as i32) * 20,
                                68 + (r as i32) * 20,
                                18,
                                18,
                                palette[usize::from(cell).min(3)],
                            );
                        }
                    }
                }
                let pc = palette[usize::from(self.piece_kind + 1).min(3)];
                canvas.fill_rect(220 + self.piece_x * 20, 68 + self.piece_y * 20, 18, 18, pc);
                canvas.fill_rect(
                    220 + (self.piece_x + 1) * 20,
                    68 + self.piece_y * 20,
                    18,
                    18,
                    pc,
                );
            }
            GameId::Racing => {
                canvas.draw_box(190, 62, 260, 344, gfx::BG, gfx::BORDER);
                canvas.fill_rect(276, 62, 2, 344, gfx::BORDER);
                canvas.fill_rect(362, 62, 2, 344, gfx::BORDER);
                let obs_x = 212 + self.obs_lane * 86;
                canvas.fill_rect(obs_x, self.obs_y, 40, 32, gfx::WARN_RED);
                let car_x = 212 + self.car_lane * 86;
                canvas.fill_rect(car_x, 352, 40, 36, gfx::OK_GREEN);
            }
        }

        if self.game_over {
            canvas.draw_box(170, 190, 300, 74, gfx::BG, gfx::WARN_RED);
            canvas.draw_text_centered(320, 204, "GAME OVER", gfx::WARN_RED, 2);
            canvas.draw_text_centered(
                320,
                238,
                tr(
                    lang,
                    "[Space/Enter] Retry  [Esc] Menu",
                    "[Spacja/Enter] Od nowa  [Esc] Menu",
                ),
                gfx::TEXT,
                1,
            );
        }
    }
}
