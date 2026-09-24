use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::Theme;
use super::common::{self, mix_white, scale};
use crate::app::AppState;
use crate::config::Config;

const GREEN: (u8, u8, u8) = (0, 255, 70);
const DENSITY: f32 = 0.5;
const SPEED: f32 = 1.0;
/// Larger gaps (a stall, a pause) would make every drop jump at once.
const MAX_STEP: Duration = Duration::from_millis(200);

/// Tiny xorshift generator, so no `rand` crate is needed.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

struct Drop {
    head: f32,
    len: u16,
    cells_per_s: f32,
    active: bool,
}

pub struct Rain {
    width: u16,
    height: u16,
    drops: Vec<Drop>,
    glyphs: Vec<char>,
    rng: Rng,
}

fn random_glyph(rng: &mut Rng) -> char {
    // Halfwidth katakana (U+FF66..U+FF9D) are one cell wide, unlike the fullwidth block.
    let pick = match rng.below(4) {
        0 | 1 => 0xFF66 + rng.below(56) as u32,
        2 => '0' as u32 + rng.below(10) as u32,
        _ => 'A' as u32 + rng.below(26) as u32,
    };
    char::from_u32(pick).unwrap_or('0')
}

fn new_drop(rng: &mut Rng, height: u16) -> Drop {
    Drop {
        head: 0.0,
        len: 4 + rng.below(usize::from(height / 2).max(1)) as u16,
        cells_per_s: rng.range(6.0, 18.0),
        active: true,
    }
}

impl Rain {
    pub fn new(seed: u64) -> Self {
        Self {
            width: 0,
            height: 0,
            drops: Vec::new(),
            glyphs: Vec::new(),
            rng: Rng::new(seed),
        }
    }

    pub fn fit(&mut self, width: u16, height: u16, density: f32) {
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        let rng = &mut self.rng;
        self.glyphs = (0..usize::from(width) * usize::from(height))
            .map(|_| random_glyph(rng))
            .collect();
        self.drops = (0..width)
            .map(|_| {
                let mut d = new_drop(rng, height);
                d.active = rng.unit() < density;
                d.head = rng.range(0.0, f32::from(height));
                d
            })
            .collect();
    }

    pub fn step(&mut self, dt: f32, density: f32, speed: f32) {
        let height = f32::from(self.height);
        for d in &mut self.drops {
            if d.active {
                d.head += d.cells_per_s * speed * dt;
                if d.head - f32::from(d.len) > height {
                    d.active = false;
                }
            } else if self.rng.unit() < density * dt {
                *d = new_drop(&mut self.rng, self.height);
            }
        }
        for _ in 0..self.glyphs.len() / 40 {
            let i = self.rng.below(self.glyphs.len());
            self.glyphs[i] = random_glyph(&mut self.rng);
        }
    }

    /// Writes only the cells a drop covers; the rest keep the black background.
    pub fn render(&self, buf: &mut Buffer, area: Rect, base: (u8, u8, u8)) {
        let w = self.width.min(area.width);
        let h = self.height.min(area.height);
        for (x, d) in self.drops.iter().enumerate().take(usize::from(w)) {
            if !d.active {
                continue;
            }
            let head = d.head as i32;
            for i in 0..d.len {
                let y = head - i32::from(i);
                if y < 0 || y >= i32::from(h) {
                    continue;
                }
                let (x16, y16) = (x as u16, y as u16);
                let glyph = self.glyphs[usize::from(y16) * usize::from(self.width) + x];
                let cell = &mut buf[(area.x + x16, area.y + y16)];
                cell.set_char(glyph);
                if i == 0 {
                    cell.set_fg(mix_white(base, 0.7));
                    cell.modifier.insert(Modifier::BOLD);
                } else {
                    let fade = 1.0 - f32::from(i) / f32::from(d.len);
                    cell.set_fg(scale(base, 0.15 + 0.85 * fade));
                }
            }
        }
    }
}

/// Digital rain behind the metric cards.
pub struct Matrix {
    rain: Rain,
    last_tick: Option<Instant>,
}

impl Matrix {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x5EED_CAFE, |d| d.as_nanos() as u64);
        Self::with_seed(seed)
    }

    pub fn with_seed(seed: u64) -> Self {
        Self {
            rain: Rain::new(seed),
            last_tick: None,
        }
    }
}

impl Theme for Matrix {
    fn name(&self) -> &'static str {
        "matrix"
    }

    fn frame_interval(&self, cfg: &Config) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            1.0 / f64::from(cfg.general.fps.max(1)),
        ))
    }

    /// Advances by real elapsed time, so the rain speed does not depend on fps.
    fn tick(&mut self, area: Rect, _cfg: &Config) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map_or(Duration::ZERO, |t| (now - t).min(MAX_STEP));
        self.last_tick = Some(now);
        self.rain.fit(area.width, area.height, DENSITY);
        self.rain.step(dt.as_secs_f32(), DENSITY, SPEED);
    }

    fn draw(&mut self, frame: &mut Frame, state: &AppState) {
        let area = frame.area();
        self.rain.fit(area.width, area.height, DENSITY);
        let buf = frame.buffer_mut();
        buf.set_style(area, Style::new().bg(Color::Rgb(0, 0, 0)));
        self.rain.render(buf, area, GREEN);
        let body = common::body_area(area, state);
        common::draw_columns(frame, body, state, &common::matrix_palette(GREEN), true);
    }
}
