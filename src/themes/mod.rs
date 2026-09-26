pub mod common;
pub mod crt_amber;
pub mod cyberpunk;
pub mod matrix;
pub mod minimalist;
pub mod tokyo_night;

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::AppState;
use crate::config::Config;

pub trait Theme {
    fn name(&self) -> &'static str;
    /// `None` = static: redraw only when data changes. `Some(d)` = animated.
    fn frame_interval(&self, cfg: &Config) -> Option<Duration>;
    fn tick(&mut self, _area: Rect, _cfg: &Config) {}
    fn draw(&mut self, frame: &mut Frame, state: &AppState);
}

/// Same order as `config::THEME_NAMES`, so a theme index means the same in both.
pub fn all() -> Vec<Box<dyn Theme>> {
    vec![
        Box::new(minimalist::Minimalist),
        Box::new(matrix::Matrix::new()),
        Box::new(tokyo_night::TokyoNight),
        Box::new(crt_amber::CrtAmber),
        Box::new(cyberpunk::Cyberpunk),
    ]
}

pub fn index_of(name: &str) -> Option<usize> {
    all().iter().position(|t| t.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::THEME_NAMES;
    use crate::ui::demo;
    use ratatui::buffer::Buffer;

    fn text(buf: &Buffer) -> String {
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    fn render(theme: &str, w: u16, h: u16) -> Buffer {
        demo::render(&demo::state(theme), w, h)
    }

    #[test]
    fn new_themes_draw_the_common_cards_at_100x30_and_40x10() {
        for theme in ["tokyo-night", "crt-amber", "cyberpunk"] {
            let t = text(&render(theme, 100, 30));
            for card in [
                "CPU", "RAM", "Swap", "Disks", "GPU", "Clock", "Crypto", "Weather",
            ] {
                assert!(t.contains(card), "{theme}: {card}");
            }
            assert!(t.contains("84,853 USDT") && t.contains("+2.1%"), "{theme}");
            assert!(
                t.contains(&format!("{theme} · 15 fps")),
                "{theme}: status bar"
            );
            let small = text(&render(theme, 40, 10));
            assert!(
                small.contains("CPU") && small.contains(" telemetrix "),
                "{theme}"
            );
            assert!(!small.contains(crate::ui::TOO_SMALL), "{theme}");
            let interval = all()[index_of(theme).unwrap()].frame_interval(&Config::default());
            assert_eq!(interval, None, "{theme}: static");
        }
    }

    #[test]
    fn each_new_theme_has_its_own_look() {
        let tokyo = render("tokyo-night", 100, 30);
        assert_eq!(tokyo[(0, 0)].bg, tokyo_night::SCREEN, "slate background");
        assert!(text(&tokyo).contains("╭ CPU "), "rounded cards");

        let crt = render("crt-amber", 100, 30);
        assert!(text(&crt).contains("┏ CPU "), "heavy frames");
        assert_eq!(crt[(0, 0)].bg, crt_amber::SCREEN);
        assert_ne!(crt[(0, 1)].bg, crt_amber::SCREEN, "a darker scanline");
        assert_eq!(crt[(0, 2)].bg, crt_amber::SCREEN);
        let card_bg = crt_amber::palette().bg.unwrap();
        let rows: Vec<u16> = (0..28).filter(|y| crt[(4, *y)].bg == card_bg).collect();
        assert!(
            rows.len() > 3 && rows.iter().all(|y| y % 2 == 0),
            "scanlines run through the cards too: {rows:?}"
        );

        let neon = render("cyberpunk", 100, 30);
        let t = text(&neon);
        assert!(t.contains("╒[ CPU ]═"), "HUD frame with a bracketed title");
        assert_eq!(neon[(0, 0)].bg, cyberpunk::SCREEN);
        let pos = t.find("[ CPU ]").map(|b| t[..b].chars().count()).unwrap();
        assert_eq!(
            neon.content()[pos + 2].fg,
            cyberpunk::MAGENTA,
            "magenta title"
        );
    }

    #[test]
    fn warnings_use_each_palette() {
        for (theme, warn) in [
            ("tokyo-night", tokyo_night::palette().warn),
            ("crt-amber", crt_amber::palette().warn),
            ("cyberpunk", cyberpunk::palette().warn),
        ] {
            let mut s = demo::state(theme);
            let mut snap = demo::snapshot();
            snap.cpu_usage = 100.0;
            s.snapshot = Some(snap);
            let buf = demo::render(&s, 100, 30);
            assert!(
                buf.content()
                    .iter()
                    .any(|c| c.symbol() == "█" && c.fg == warn),
                "{theme}: a full CPU gauge in the palette's warning colour"
            );
        }
    }

    #[test]
    fn registry_matches_the_config_enum() {
        let names: Vec<&str> = all().iter().map(|t| t.name()).collect();
        assert_eq!(names, THEME_NAMES);
        assert_eq!(index_of("matrix"), Some(1));
        assert_eq!(index_of("neon"), None);
    }
}
