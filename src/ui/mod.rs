//! Everything drawn on top of the theme: banner, status bar, toast, overlays.

mod help_overlay;
mod log_overlay;
pub mod settings_overlay;
mod theme_picker;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};

use crate::app::{AppState, Overlay};
use crate::selfmem;
use crate::themes::Theme;
use crate::themes::common::{self, ACCENT, MUTED, fg};

pub const TOO_SMALL: &str = "terminal too small (need 40x10)";
const OVERLAY_BG: Color = Color::Rgb(22, 22, 22);
const OVERLAY_FG: Color = Color::Rgb(210, 210, 210);

pub fn draw(frame: &mut Frame, state: &AppState, theme: &mut dyn Theme) {
    let area = frame.area();
    if common::too_small(area) {
        let y = area.y + area.height.saturating_sub(1) / 2;
        let rect = Rect::new(area.x, y, area.width, 2).intersection(area);
        let notice = Paragraph::new(TOO_SMALL)
            .centered()
            .wrap(Wrap { trim: true });
        frame.render_widget(notice, rect);
        return;
    }
    theme.draw(frame, state);
    let banner_text = state.banner();
    let [banner, body, status] = Layout::vertical([
        Constraint::Length(u16::from(banner_text.is_some())),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    // Paragraph backgrounds do not erase rain glyphs, so these rows are cleared first.
    frame.render_widget(Clear, banner);
    frame.render_widget(Clear, status);
    if let Some(text) = banner_text {
        let style = Style::new()
            .bg(Color::Rgb(135, 20, 20))
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        frame.render_widget(Paragraph::new(format!(" {text}")).style(style), banner);
    }
    draw_status(frame, status, state);
    if let Some((text, _)) = &state.toast {
        draw_toast(frame, body, text);
    }
    match state.overlay {
        Overlay::None => {}
        Overlay::Settings => settings_overlay::draw(frame, area, state),
        Overlay::Themes => theme_picker::draw(frame, area, state),
        Overlay::Log => log_overlay::draw(frame, area, state),
        Overlay::Help => help_overlay::draw(frame, area, state),
    }
}

fn draw_status(frame: &mut Frame, area: Rect, state: &AppState) {
    let paused = if state.paused { " · PAUSED" } else { "" };
    let memory = state.self_memory.map_or(String::new(), |m| {
        let note = if state.memory_paused {
            " (speed test)"
        } else {
            ""
        };
        format!(" self {:.1} MB{note} ·", selfmem::mb(m.working_set))
    });
    let rest = format!(
        " {} · {} fps{paused} ",
        state.theme_name(),
        state.config.general.fps
    );
    let right_w = u16::try_from(memory.chars().count() + rest.chars().count()).unwrap_or(u16::MAX);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(area);
    let base = Style::new()
        .bg(Color::Rgb(38, 38, 38))
        .fg(Color::Rgb(190, 190, 190));
    let left = Line::from(vec![
        Span::styled(
            " telemetrix ",
            Style::new()
                .bg(ACCENT)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" · t themes · s settings · l log · ? help · q quit"),
    ]);
    frame.render_widget(Paragraph::new(left).style(base), left_area);
    let strong = base.fg(Color::White).add_modifier(Modifier::BOLD);
    let memory_style = if state.memory_alarm() {
        strong.fg(ACCENT)
    } else {
        strong
    };
    let right = Line::from(vec![
        Span::styled(memory, memory_style),
        Span::styled(rest, strong),
    ]);
    frame.render_widget(Paragraph::new(right).style(base), right_area);
}

fn draw_toast(frame: &mut Frame, body: Rect, text: &str) {
    let w = u16::try_from(text.chars().count() + 4)
        .unwrap_or(u16::MAX)
        .min(body.width);
    let y = body.bottom().saturating_sub(4).max(body.y);
    let rect = Rect::new(body.x + (body.width - w) / 2, y, w, 3).intersection(body);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(ACCENT))
        .style(Style::new().bg(OVERLAY_BG).fg(Color::White));
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(text).centered().block(block), rect);
}

pub(crate) fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Clears `rect`, draws the overlay frame, returns the inner area.
pub(crate) fn overlay_frame(frame: &mut Frame, rect: Rect, title: &str) -> Rect {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(MUTED))
        .title(Span::styled(
            format!(" {title} "),
            fg(Color::White).add_modifier(Modifier::BOLD),
        ))
        .style(Style::new().bg(OVERLAY_BG).fg(OVERLAY_FG));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ConfigStatus};
    use crate::metrics::{DiskMetric, SystemSnapshot};
    use crate::themes;
    use crate::themes::common::{RISE, WARN};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn state(theme: &str) -> AppState {
        let mut cfg = Config::default();
        cfg.general.theme = theme.into();
        let mut state = AppState::new(cfg, ConfigStatus::Ok);
        state.apply(crate::event::AppEvent::Metrics(SystemSnapshot {
            cpu_usage: 100.0,
            cpu_temp: Some(50.0),
            ram_used_bytes: 1 << 30,
            ram_total_bytes: 4 << 30,
            disks: vec![DiskMetric {
                mount: "C:\\".into(),
                label: None,
                used_bytes: 10,
                total_bytes: 100,
            }],
            ..SystemSnapshot::default()
        }));
        state
    }

    fn render(state: &AppState, w: u16, h: u16) -> Buffer {
        let mut theme = themes::all().remove(state.theme_idx);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| {
                theme.tick(f.area(), &state.config);
                draw(f, state, theme.as_mut());
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buf: &Buffer) -> String {
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn full_cpu_uses_the_warning_color_at_80x24() {
        for theme in ["minimalist", "matrix"] {
            let buf = render(&state(theme), 80, 24);
            let t = text(&buf);
            assert!(t.contains(" CPU "), "{theme}: CPU card title");
            assert!(t.contains(" 100%"), "{theme}: CPU percentage");
            assert!(
                buf.content()
                    .iter()
                    .any(|c| c.symbol() == "█" && c.fg == WARN),
                "{theme}: the CPU gauge is drawn in the warning color"
            );
        }
    }

    #[test]
    fn minimum_size_still_draws() {
        for theme in ["minimalist", "matrix"] {
            let t = text(&render(&state(theme), 40, 10));
            assert!(t.contains("CPU") && t.contains("telemetrix"), "{theme}");
            assert!(!t.contains(TOO_SMALL));
        }
    }

    #[test]
    fn network_card_shows_grouped_single_and_offline_rows() {
        use crate::metrics::network::NetDrive;
        let nas = |letter: &str, share: &str, online: bool, total: u64, free: u64| NetDrive {
            mount: letter.into(),
            server: Some("nas".into()),
            share: Some(share.into()),
            label: None,
            online,
            total_bytes: total,
            free_bytes: free,
        };
        let tb = 1u64 << 40;
        let mut s = state("minimalist");
        s.network = Some(vec![
            nas("M:", "music", true, 5 * tb, 2 * tb),
            nas("P:", "photos", true, 5 * tb, 2 * tb),
            nas("Y:", "Archive", true, 24 * tb, 8 * tb),
            nas("Z:", "old_share", false, 0, 0),
        ]);
        let buf = render(&s, 120, 40);
        let t = text(&buf);
        assert!(t.contains(" Network "), "card title");
        assert!(t.contains("nas  M: P:"), "grouped row");
        assert!(t.contains("Archive (Y:)"), "single row");
        assert!(t.contains("old_share (Z:)"), "offline row");
        let offline_red = buf
            .content()
            .iter()
            .zip(buf.content().iter().skip(1))
            .any(|(a, b)| a.symbol() == "o" && b.symbol() == "f" && a.fg == WARN);
        assert!(offline_red, "offline is drawn in the warning colour");

        s.config.disks.show_network = false;
        assert!(
            !text(&render(&s, 120, 40)).contains(" Network "),
            "hidden when turned off"
        );
        s.config.disks.show_network = true;
        s.network = None;
        assert!(text(&render(&s, 120, 40)).contains("checking…"));
        s.network = Some(Vec::new());
        assert!(
            !text(&render(&s, 120, 40)).contains(" Network "),
            "hidden without drives"
        );
    }

    #[test]
    fn plugin_trend_rows_render_in_both_themes() {
        use crate::plugins::{MetricItem, PluginCard, PluginData, PluginStatus};
        for theme in ["minimalist", "matrix"] {
            let mut s = state(theme);
            let mut up = MetricItem::text("7d", "+0.8%");
            up.trend = Some(vec![1.0, 1.5, 1.2, 2.0, 2.4]);
            let mut down = MetricItem::text("30d", "-1.2%");
            down.trend = Some(vec![3.0, 2.0, 1.0]);
            let mut bad = MetricItem::text("1d", "trend needs 2..400 numbers");
            bad.bad = true;
            let data = PluginData {
                id: "fx".into(),
                title: "FX".into(),
                metrics: vec![MetricItem::text("USD", "44.80"), up, down, bad],
                error: None,
                lua_bytes: None,
            };
            s.plugins.insert(
                "fx".into(),
                PluginCard {
                    data,
                    status: PluginStatus::Ok,
                },
            );
            // One column at this width: the plugin card comes after the system cards.
            let buf = render(&s, 45, 70);
            let t = text(&buf);
            assert!(
                t.contains("7d ") && t.contains("+0.8%") && t.contains("-1.2%"),
                "{theme}"
            );
            assert!(
                t.contains('▁') && t.contains('█'),
                "{theme}: a sparkline is drawn"
            );
            let color_of = |needle: &str| {
                let cells = buf.content();
                let chars: Vec<&str> = cells.iter().map(|c| c.symbol()).collect();
                let pos = chars
                    .windows(needle.len())
                    .position(|w| w.concat() == needle)
                    .unwrap();
                cells[pos].fg
            };
            assert_eq!(color_of("+0.8%"), RISE, "{theme}");
            assert_eq!(color_of("-1.2%"), WARN, "{theme}");
            assert_eq!(color_of("trend needs"), WARN, "{theme}");
        }
    }

    #[test]
    fn metric_styles_render_in_both_themes() {
        use crate::plugins::{MetricItem, MetricStyle, PluginCard, PluginData, PluginStatus};
        use crate::themes::common::{MUTED, RISE};
        use ratatui::style::Modifier;
        let styled = |label: &str, value: &str, style| {
            let mut m = MetricItem::text(label, value);
            m.style = style;
            m
        };
        for theme in ["minimalist", "matrix"] {
            let mut s = state(theme);
            let data = PluginData {
                id: "fx".into(),
                title: "FX".into(),
                metrics: vec![
                    styled("", "buyhead", Some(MetricStyle::Header)),
                    styled("plain", "valueplain", None),
                    styled("7d quiet", "30d quiet", Some(MetricStyle::Dim)),
                    styled("up", "risegood", Some(MetricStyle::Good)),
                    styled("stale", "sincebad", Some(MetricStyle::Bad)),
                ],
                error: None,
                lua_bytes: None,
            };
            s.plugins.insert(
                "fx".into(),
                PluginCard {
                    data,
                    status: PluginStatus::Ok,
                },
            );
            let buf = render(&s, 45, 70);
            let cell = |needle: &str| {
                let cells = buf.content();
                let chars: Vec<&str> = cells.iter().map(|c| c.symbol()).collect();
                let pos = chars
                    .windows(needle.len())
                    .position(|w| w.concat() == needle)
                    .unwrap_or_else(|| panic!("{theme}: {needle} not drawn"));
                cells[pos].clone()
            };
            let plain = cell("valueplain").fg;
            let header = cell("buyhead");
            assert!(header.modifier.contains(Modifier::BOLD), "{theme}");
            assert_ne!(header.fg, plain, "{theme}: the header is not a value");
            assert_eq!(cell("7d quiet").fg, MUTED, "{theme}");
            assert_eq!(cell("30d quiet").fg, MUTED, "{theme}");
            assert_eq!(cell("risegood").fg, RISE, "{theme}");
            assert_eq!(cell("sincebad").fg, WARN, "{theme}");
        }
        // Too narrow for both parts: a dim row keeps its label and drops the value.
        let pal = crate::themes::common::minimalist_palette();
        let m = styled(
            "7d ▁▂▃▅▇▆█ +0.69%",
            "30d ▂▃▄▃▅▆█ +0.96%",
            Some(MetricStyle::Dim),
        );
        let narrow: String = crate::themes::common::metric_line(&m, &pal, 30)
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(
            narrow.starts_with("7d ▁▂▃▅▇▆█ +0.69%") && !narrow.contains("30d"),
            "{narrow}"
        );
    }

    #[test]
    fn tiny_terminal_shows_the_notice() {
        assert!(text(&render(&state("minimalist"), 30, 5)).contains("terminal too small"));
        assert!(text(&render(&state("matrix"), 39, 20)).contains(TOO_SMALL));
    }

    #[test]
    fn settings_overlay_shows_schema_rows_and_the_text_input() {
        use crate::plugins::schema::{SchemaEntry, SchemaKind};
        let mut s = state("minimalist");
        s.plugin_ids = vec!["weather".into()];
        let city = SchemaEntry {
            key: "city".into(),
            label: "city".into(),
            kind: SchemaKind::Text,
            default: crate::config::Value::Str("Kyiv".into()),
        };
        s.plugin_schemas.insert("weather".into(), vec![city]);
        s.overlay = Overlay::Settings;
        let rows = settings_overlay::rows(&s.plugin_ids, &s.plugin_schemas);
        s.settings_cursor = settings_overlay::selectable(&rows).len() - 1;
        let t = text(&render(&s, 80, 60));
        assert!(t.contains("Kyiv  (Enter to edit)"), "the selected text row");
        s.text_input = Some(crate::app::TextInput::new("weather", "city", "Lviv"));
        for (w, h) in [(80, 60), (40, 10)] {
            let t = text(&render(&s, w, h));
            assert!(t.contains("Esc cancel"), "{w}x{h}: the input footer");
        }
        assert!(text(&render(&s, 80, 60)).contains("Lviv "));
    }

    #[test]
    fn help_lists_the_accepted_plugin_run_keys() {
        use crate::event::AppEvent;
        let mut s = state("minimalist");
        let meta = |id: &str, title: &str, key| AppEvent::PluginMeta {
            id: id.into(),
            title: title.into(),
            run_key: Some(key),
            schema: Vec::new(),
        };
        s.apply(meta("speedtest", "Speed test", 'g'));
        s.apply(meta("other", "Other", 'g'));
        s.apply(meta("bad", "Bad", 's'));
        s.overlay = Overlay::Help;
        let t = text(&render(&s, 80, 30));
        assert!(
            t.contains(&format!("  {:<20}Speed test: run now", "g")),
            "{t}"
        );
        assert!(!t.contains("Other: run now"), "a refused key is not listed");
        assert!(!t.contains("Bad: run now"));
        s.apply(AppEvent::PluginRemoved("speedtest".into()));
        assert!(!text(&render(&s, 80, 30)).contains("run now"));
    }

    #[test]
    fn status_bar_notes_a_speed_test_in_the_normal_colour() {
        use crate::selfmem::{MB, SelfMemory};
        use std::time::Duration;
        let mut s = state("minimalist");
        let over = SelfMemory {
            working_set: 14 * MB + MB / 5,
            peak_working_set: None,
            private: None,
        };
        s.record_memory_with(over, true, None);
        let buf = render(&s, 120, 30);
        let t = text(&buf);
        assert!(t.contains(" self 14.2 MB (speed test) ·"), "{t}");
        let color_of_self = |buf: &Buffer| {
            let cells = buf.content();
            let chars: Vec<&str> = cells.iter().map(|c| c.symbol()).collect();
            let pos = chars.windows(4).position(|w| w.concat() == "self").unwrap();
            cells[pos].fg
        };
        assert_ne!(color_of_self(&buf), ACCENT, "not amber during the test");
        s.record_memory_with(over, false, Some(Duration::from_secs(30)));
        let buf = render(&s, 120, 30);
        assert!(!text(&buf).contains("(speed test)"));
        assert_eq!(
            color_of_self(&buf),
            ACCENT,
            "amber once the grace period is over"
        );
    }

    #[test]
    fn empty_plugins_card_names_the_folder() {
        let mut s = state("minimalist");
        let long = "/home/someone-with-a-long-name/.config/telemetrix/plugins";
        s.plugins_dir = std::path::PathBuf::from(long);
        let t = text(&render(&s, 45, 70));
        assert!(t.contains("no plugins found in:"));
        assert!(t.contains("press l for the log"));
        let joined: String = t.split('│').map(str::trim).collect();
        assert!(joined.contains(long), "the whole path, wrapped: {t}");
        s.plugins_running = 2;
        assert!(text(&render(&s, 45, 70)).contains("starting plugins"));
    }

    #[test]
    fn overlays_banner_and_toast_render_at_all_sizes() {
        for theme in ["minimalist", "matrix"] {
            let mut s = state(theme);
            s.config_status = ConfigStatus::Syntax {
                line: Some(14),
                message: "expected `=`".into(),
            };
            s.toast = Some(("hello".into(), std::time::Instant::now()));
            s.plugin_ids = vec!["clock".into(), "weather".into()];
            for overlay in [
                Overlay::None,
                Overlay::Log,
                Overlay::Help,
                Overlay::Settings,
                Overlay::Themes,
            ] {
                s.overlay = overlay;
                for (w, h) in [(120, 40), (80, 24), (64, 16), (40, 10)] {
                    let buf = render(&s, w, h);
                    let status: String = (0..w).map(|x| buf[(x, h - 1)].symbol()).collect();
                    assert!(status.starts_with(" telemetrix "), "{status}");
                    let t = text(&buf);
                    assert!(
                        t.contains("config syntax error line 14"),
                        "{theme} {overlay:?} {w}x{h}"
                    );
                }
            }
        }
    }
}
