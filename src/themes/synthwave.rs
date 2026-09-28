use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::border;
use ratatui::widgets::Borders;

use super::common::{self, Palette, scale};
use super::{MAX_STEP, Theme};
use crate::app::AppState;
use crate::config::Config;

const SKY_TOP: (u8, u8, u8) = (14, 2, 34);
const SKY_LOW: (u8, u8, u8) = (72, 10, 90);
pub const FLOOR: Color = Color::Rgb(10, 0, 24);
const GRID: (u8, u8, u8) = (255, 40, 200);
const HORIZON: Color = Color::Rgb(255, 120, 90);
pub const PURPLE: (u8, u8, u8) = (170, 80, 255);
pub const ORANGE: (u8, u8, u8) = (255, 140, 40);
const SUN_TOP: (u8, u8, u8) = (255, 230, 90);
const SUN_LOW: (u8, u8, u8) = (255, 40, 140);
/// Cards are drawn with this border colour, then recoloured by row.
const BORDER_MARK: Color = Color::Rgb(PURPLE.0, PURPLE.1, PURPLE.2);
/// Grid lines that pass the viewer per second.
const SPEED: f32 = 0.35;
/// Vertical grid lines on each side of the centre one.
const RAYS: i32 = 12;

pub fn palette() -> Palette {
    Palette {
        bg: Some(Color::Rgb(24, 6, 44)),
        border: BORDER_MARK,
        title: Color::Rgb(ORANGE.0, ORANGE.1, ORANGE.2),
        label: Color::Rgb(185, 150, 235),
        value: Color::Rgb(255, 226, 190),
        bar: Color::Rgb(255, 70, 180),
        bar_empty: Color::Rgb(70, 25, 95),
        spark: Color::Rgb(0, 225, 255),
        border_set: border::DOUBLE,
        brackets: (" ", " "),
        warn: Color::Rgb(255, 40, 80),
        rise: Color::Rgb(80, 255, 200),
        muted: Color::Rgb(125, 95, 165),
        gauge: common::BLOCK_GAUGE,
        borders: Borders::ALL,
    }
}

pub fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color::Rgb(f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

/// The row of the horizon: a little below the middle of the body.
pub fn horizon(body: Rect) -> u16 {
    body.y + body.height * 3 / 5
}

fn sky(buf: &mut Buffer, area: Rect, horizon: u16) {
    let span = f32::from(horizon.saturating_sub(area.y).max(1));
    for y in area.y..horizon.min(area.bottom()) {
        let t = f32::from(y - area.y) / span;
        let row = Rect::new(area.x, y, area.width, 1);
        buf.set_style(row, Style::new().bg(lerp(SKY_TOP, SKY_LOW, t)));
    }
}

/// A striped half sun sitting on the horizon.
fn sun(buf: &mut Buffer, body: Rect, horizon: u16) {
    let radius = (horizon.saturating_sub(body.y) * 2 / 3).clamp(2, 9);
    let r = f32::from(radius);
    let cx = f32::from(body.x) + f32::from(body.width) / 2.0;
    for dy in 1..=radius {
        // Gaps in the lower half, as on the classic outrun sun.
        if dy <= radius / 2 && dy % 2 == 1 {
            continue;
        }
        let Some(y) = horizon.checked_sub(dy).filter(|y| *y >= body.y) else {
            continue;
        };
        let h = f32::from(dy) - 0.5;
        // Cells are about twice as tall as wide.
        let half = 2.0 * (r * r - h * h).max(0.0).sqrt();
        let color = lerp(SUN_LOW, SUN_TOP, f32::from(dy) / r);
        let from = (cx - half).round().max(f32::from(body.x)) as u16;
        let to = (cx + half).round().min(f32::from(body.right())) as u16;
        for x in from..to {
            buf[(x, y)].set_bg(color);
        }
    }
}

/// The perspective grid under the horizon. `phase` (0..1) moves the
/// cross lines toward the viewer; the rays stay still.
fn floor(buf: &mut Buffer, body: Rect, horizon: u16, phase: f32) {
    let floor = Rect::new(
        body.x,
        horizon,
        body.width,
        body.bottom().saturating_sub(horizon),
    );
    buf.set_style(floor, Style::new().bg(FLOOR));
    for x in body.x..body.right() {
        buf[(x, horizon)].set_char('─').set_fg(HORIZON);
    }
    let depth = body.bottom().saturating_sub(horizon + 1);
    if depth == 0 {
        return;
    }
    let d = f32::from(depth);
    let shade = |dy: u16| scale(GRID, 0.3 + 0.7 * f32::from(dy) / d);
    // A cross line at distance z is d / z rows below the horizon. The row
    // right under the horizon stays open, or the far lines merge into a block.
    let mut z = 1.0 - phase;
    loop {
        let dy = (d / z).round();
        if dy < 2.0 {
            break;
        }
        if dy <= d {
            let dy = dy as u16;
            for x in body.x..body.right() {
                buf[(x, horizon + dy)].set_char('─').set_fg(shade(dy));
            }
        }
        z += 1.0;
    }
    let cx = f32::from(body.x) + f32::from(body.width) / 2.0;
    let spacing = f32::from(body.width) / 5.0;
    for k in -RAYS..=RAYS {
        let glyph = match k.signum() {
            -1 => '╱',
            1 => '╲',
            _ => '│',
        };
        for dy in 1..=depth {
            let x = cx + k as f32 * spacing * f32::from(dy) / d;
            if x < f32::from(body.x) || x >= f32::from(body.right()) {
                continue;
            }
            buf[(x as u16, horizon + dy)]
                .set_char(glyph)
                .set_fg(shade(dy));
        }
    }
}

/// Recolours the card frames from purple at the top to orange at the bottom.
fn gradient_frames(buf: &mut Buffer, body: Rect) {
    let span = f32::from(body.height.max(2) - 1);
    for y in body.y..body.bottom() {
        let color = lerp(PURPLE, ORANGE, f32::from(y - body.y) / span);
        for x in body.x..body.right() {
            let cell = &mut buf[(x, y)];
            if cell.fg == BORDER_MARK {
                cell.set_fg(color);
            }
        }
    }
}

/// A neon sunset: a sky from purple to magenta, a striped sun and a
/// wireframe floor whose lines move toward the viewer. Animated.
pub struct Synthwave {
    phase: f32,
    last_tick: Option<Instant>,
    cells: u32,
}

impl Synthwave {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            last_tick: None,
            cells: 0,
        }
    }

    /// Moves the floor by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        self.phase = (self.phase + dt * SPEED).fract();
    }
}

impl Theme for Synthwave {
    fn name(&self) -> &'static str {
        "synthwave"
    }

    fn frame_interval(&self, cfg: &Config) -> Option<Duration> {
        Some(super::animation_interval(cfg, self.cells))
    }

    fn tick(&mut self, area: Rect, _cfg: &Config) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map_or(Duration::ZERO, |t| (now - t).min(MAX_STEP));
        self.last_tick = Some(now);
        self.cells = u32::from(area.width) * u32::from(area.height);
        self.step(dt.as_secs_f32());
    }

    fn draw(&mut self, frame: &mut Frame, state: &AppState) {
        let area = frame.area();
        let body = common::body_area(area, state);
        let line = horizon(body);
        let buf = frame.buffer_mut();
        sky(buf, area, line);
        sun(buf, body, line);
        floor(buf, body, line, self.phase);
        common::draw_columns(frame, body, state, &palette(), true);
        gradient_frames(frame.buffer_mut(), body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{self, TOO_SMALL, demo};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn text(buf: &Buffer) -> String {
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    fn render(theme: &mut Synthwave, w: u16, h: u16) -> Buffer {
        let state = demo::state("synthwave");
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| ui::draw(f, &state, theme)).unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn sunset_sky_sun_floor_and_readable_cards() {
        let buf = render(&mut Synthwave::new(), 100, 30);
        let t = text(&buf);
        for needle in [
            "╔ CPU ═",
            "╔ Crypto ═",
            "84,853 USDT",
            "+2.1%",
            "NVIDIA GeForce",
        ] {
            assert!(t.contains(needle), "{needle}");
        }
        let line = horizon(Rect::new(0, 0, 100, 29));
        assert_eq!(buf[(0, 0)].bg, lerp(SKY_TOP, SKY_LOW, 0.0));
        assert_eq!(buf[(0, line + 2)].bg, FLOOR);
        assert_eq!(buf[(0, line)].symbol(), "─");
        let sun =
            (8..line).any(|y| (40..60).any(|x| matches!(buf[(x, y)].bg, Color::Rgb(255, _, _))));
        assert!(sun, "the sun shows in the gap under the Disks card");
        let frame_colors: Vec<Color> = buf
            .content()
            .iter()
            .filter(|c| c.symbol() == "║")
            .map(|c| c.fg)
            .collect();
        assert!(!frame_colors.contains(&BORDER_MARK));
        assert!(frame_colors.first() != frame_colors.last(), "a gradient");
    }

    #[test]
    fn only_the_floor_moves_between_frames() {
        let mut theme = Synthwave::new();
        let first = render(&mut theme, 100, 30);
        assert_eq!(first, render(&mut theme, 100, 30), "no tick, no change");
        theme.step(1.0);
        let later = render(&mut theme, 100, 30);
        let line = horizon(Rect::new(0, 0, 100, 29));
        let mut changed = 0;
        for y in 0..30 {
            for x in 0..100 {
                if first[(x, y)] != later[(x, y)] {
                    assert!(y > line, "{x},{y} is above the floor");
                    changed += 1;
                }
            }
        }
        assert!(changed > 0, "the grid moved");
        let mut again = Synthwave::new();
        again.step(1.0);
        assert_eq!(render(&mut again, 100, 30), later, "same time, same frame");
    }

    #[test]
    fn frame_rate_and_small_screens() {
        let mut cfg = Config::default();
        let mut theme = Synthwave::new();
        theme.tick(Rect::new(0, 0, 100, 30), &cfg);
        assert_eq!(
            theme.frame_interval(&cfg),
            Some(Duration::from_secs_f64(1.0 / 15.0))
        );
        cfg.general.fps = 30;
        theme.tick(Rect::new(0, 0, 250, 90), &cfg);
        assert_eq!(theme.frame_interval(&cfg), Some(Duration::from_millis(100)));
        let t = text(&demo::render(&demo::state("synthwave"), 40, 10));
        assert!(t.contains(" CPU ") && t.contains(" telemetrix "), "{t}");
        assert!(!t.contains(TOO_SMALL));
        let t = text(&demo::render(&demo::state("synthwave"), 30, 8));
        assert!(t.contains("terminal too small"), "{t}");
    }
}
