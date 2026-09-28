pub mod ascii_dashboard;
pub mod common;
pub mod crt_amber;
pub mod cyberpunk;
pub mod kernel_log;
pub mod matrix;
pub mod minimalist;
pub mod nasa;
pub mod pip_boy;
pub mod synthwave;
pub mod tokyo_night;

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::AppState;
use crate::config::Config;

/// Above this many cells a frame costs enough that 10 FPS is the ceiling.
pub const LARGE_AREA: u32 = 20_000;
const LARGE_AREA_FPS: u32 = 10;
/// Larger gaps (a stall, a pause) would make an animation jump at once.
pub const MAX_STEP: Duration = Duration::from_millis(200);

/// The frame interval of an animated theme on a screen of `cells` cells.
pub fn animation_interval(cfg: &Config, cells: u32) -> Duration {
    let mut fps = cfg.general.fps.max(1);
    if cells > LARGE_AREA {
        fps = fps.min(LARGE_AREA_FPS);
    }
    Duration::from_secs_f64(1.0 / f64::from(fps))
}

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
        Box::new(nasa::Nasa),
        Box::new(pip_boy::PipBoy),
        Box::new(synthwave::Synthwave::new()),
        Box::new(kernel_log::KernelLog::new()),
        Box::new(ascii_dashboard::AsciiDashboard),
    ]
}

/// Whether a theme draws anything for one of its `[theme.<name>]` options:
/// kernel-log shows values as log records, so it has no bars or charts.
pub fn uses_option(theme: &str, key: &str) -> bool {
    !(theme == "kernel-log" && matches!(key, "cpu_view" | "ram_view" | "chart_height"))
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

    fn with(theme: &str, sets: &[(&str, crate::config::Value)]) -> crate::app::AppState {
        let mut s = demo::state(theme);
        for (key, value) in sets {
            s.config.assign(&format!("theme.{theme}.{key}"), value);
        }
        s
    }

    fn str_value(v: &str) -> crate::config::Value {
        crate::config::Value::Str(v.to_string().into())
    }

    fn list(ids: &[&str]) -> crate::config::Value {
        crate::config::Value::List(ids.iter().map(|s| s.to_string()).collect())
    }

    /// The rows of the first chart: from its `100┤` down to its `  0┤`.
    fn chart_rows_drawn(buf: &Buffer) -> usize {
        let area = buf.area;
        let at = |x: u16, y: u16| -> String { (x..x + 4).map(|x| buf[(x, y)].symbol()).collect() };
        for y in 0..area.height {
            for x in 0..area.width.saturating_sub(4) {
                if at(x, y) == "100┤" {
                    let end = (y..area.height).find(|y2| at(x, *y2) == "  0┤").unwrap();
                    return usize::from(end - y + 1);
                }
            }
        }
        0
    }

    #[test]
    fn a_chart_view_draws_the_tall_history_chart_in_any_theme() {
        for theme in [
            "minimalist",
            "matrix",
            "synthwave",
            "tokyo-night",
            "pip-boy",
        ] {
            let s = with(theme, &[("cpu_view", str_value("chart"))]);
            let t = text(&demo::render(&s, 100, 30));
            assert!(
                t.contains("100┤") && t.contains("  0┤"),
                "{theme}: the scale"
            );
            assert!(
                t.contains("CPU 23%"),
                "{theme}: the percentage moves to the title"
            );
            assert!(t.contains("avg ") && t.contains("peak 35%"), "{theme}");
            assert!(t.contains("52.0 °C"), "{theme}: the temperature stays");
            let both = with(theme, &[("cpu_view", str_value("both"))]);
            let t = text(&demo::render(&both, 100, 30));
            assert!(
                t.contains("100┤") && t.contains("   23%"),
                "{theme}: chart and bar"
            );
        }
        let s = with("minimalist", &[("ram_view", str_value("chart"))]);
        let t = text(&demo::render(&s, 100, 30));
        assert!(t.contains("RAM 29%") && t.contains("100┤"));
        assert!(!t.contains("CPU 23%"), "only RAM changed");
        // nasa keeps its table rows and adds the chart under them.
        let s = with("nasa", &[("cpu_view", str_value("chart"))]);
        let t = text(&demo::render(&s, 100, 30));
        assert!(t.contains("CPU-LD") && t.contains("100┤"));
    }

    #[test]
    fn chart_height_caps_the_rows() {
        let rows = |height: &str| {
            let s = with(
                "minimalist",
                &[
                    ("cpu_view", str_value("chart")),
                    ("chart_height", str_value(height)),
                ],
            );
            chart_rows_drawn(&demo::render(&s, 120, 70))
        };
        let (small, medium, tall) = (rows("small"), rows("medium"), rows("tall"));
        assert_eq!(small, 4);
        assert_eq!(medium, 10);
        assert!(tall > medium && tall <= 20, "{tall}");
        // A short screen gives the chart less, never less than two rows.
        let s = with("minimalist", &[("cpu_view", str_value("chart"))]);
        let short = chart_rows_drawn(&demo::render(&s, 120, 24));
        assert!((2..10).contains(&short), "{short}");
    }

    #[test]
    fn hide_and_order_change_the_cards() {
        let s = with(
            "matrix",
            &[
                ("hide", list(&["swap", "clock"])),
                ("order", list(&["crypto", "cpu", "weather"])),
            ],
        );
        let buf = demo::render(&s, 100, 30);
        let t = text(&buf);
        assert!(!t.contains(" Swap ") && !t.contains(" Clock "), "hidden");
        assert!(t.contains(" Crypto ") && t.contains(" CPU ") && t.contains(" Disks "));
        let head: String = (2..10).map(|x| buf[(x, 1)].symbol()).collect();
        assert_eq!(head, "┌ Crypto", "crypto comes first, top left");
        let pos = |needle: &str| t.find(needle).unwrap();
        assert!(pos(" Crypto ") < pos(" CPU "));
        // Hiding a system card works without an order too.
        let s = with("tokyo-night", &[("hide", list(&["cpu", "gpu"]))]);
        let t = text(&demo::render(&s, 100, 30));
        assert!(!t.contains(" CPU ") && !t.contains("NVIDIA") && t.contains(" RAM "));
        // nasa: swap rows go with swap, the MEMORY card stays.
        let s = with("nasa", &[("hide", list(&["swap"]))]);
        let t = text(&demo::render(&s, 100, 30));
        assert!(t.contains("MEM-US") && !t.contains("SWP-US"));
        // kernel-log: hidden cards leave the status block and the log.
        let s = with(
            "kernel-log",
            &[("hide", list(&["cpu"])), ("order", list(&["weather"]))],
        );
        let t = text(&demo::render(&s, 100, 30));
        assert!(!t.contains("cpu0:") && !t.contains(" cpu     load"), "{t}");
        assert!(t.find("▸ Weather").unwrap() < t.find(" ram ").unwrap());
    }

    #[test]
    fn accent_changes_the_main_colour() {
        use ratatui::style::Color;
        let find = |buf: &Buffer, needle: &str| {
            let t = text(buf);
            let pos = t.find(needle).map(|b| t[..b].chars().count()).unwrap();
            buf.content()[pos].fg
        };
        let orange = Color::Rgb(0xff, 0x88, 0x00);
        for theme in [
            "minimalist",
            "tokyo-night",
            "cyberpunk",
            "synthwave",
            "ascii-dashboard",
        ] {
            let s = with(theme, &[("accent", str_value("#ff8800"))]);
            let buf = demo::render(&s, 100, 30);
            assert_eq!(find(&buf, "Disks"), orange, "{theme}: title");
            let before = demo::render(&demo::state(theme), 100, 30);
            assert_ne!(find(&before, "Disks"), orange, "{theme}: not by default");
        }
        let s = with("nasa", &[("accent", str_value("cyan"))]);
        let buf = demo::render(&s, 100, 30);
        assert_eq!(find(&buf, "CPU-LD"), Color::Rgb(0, 215, 255));
        let s = with("pip-boy", &[("accent", str_value("amber"))]);
        let buf = demo::render(&s, 100, 30);
        assert_eq!(
            buf[(1, 0)].bg,
            mix_white_amber(),
            "the lit tab follows the hue"
        );
        let s = with("crt-amber", &[("accent", str_value("green"))]);
        let buf = demo::render(&s, 100, 30);
        assert_ne!(buf[(4, 2)].bg, crt_amber::palette().bg.unwrap());
        let s = with("kernel-log", &[("accent", str_value("magenta"))]);
        let buf = demo::render(&s, 100, 30);
        assert_eq!(find(&buf, "status"), Color::Rgb(255, 70, 200));
        // matrix keeps `color` as its accent.
        assert_eq!(crate::config::accent_key("matrix"), "theme.matrix.color");
    }

    fn mix_white_amber() -> ratatui::style::Color {
        common::mix_white((255, 176, 0), 0.2)
    }

    #[test]
    fn registry_matches_the_config_enum() {
        let names: Vec<&str> = all().iter().map(|t| t.name()).collect();
        assert_eq!(names, THEME_NAMES);
        assert_eq!(index_of("matrix"), Some(1));
        assert_eq!(index_of("neon"), None);
    }
}
