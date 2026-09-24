use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use crate::data::Rng;

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
        let flicker = self.glyphs.len() / 40;
        for _ in 0..flicker {
            let i = self.rng.below(self.glyphs.len());
            self.glyphs[i] = random_glyph(&mut self.rng);
        }
    }

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

fn new_drop(rng: &mut Rng, height: u16) -> Drop {
    Drop {
        head: 0.0,
        len: 4 + rng.below(usize::from(height / 2).max(1)) as u16,
        cells_per_s: rng.range(6.0, 18.0),
        active: true,
    }
}

pub fn scale((r, g, b): (u8, u8, u8), k: f32) -> Color {
    let f = |c: u8| (f32::from(c) * k).clamp(0.0, 255.0) as u8;
    Color::Rgb(f(r), f(g), f(b))
}

pub fn mix_white((r, g, b): (u8, u8, u8), k: f32) -> Color {
    let f = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * k) as u8;
    Color::Rgb(f(r), f(g), f(b))
}
