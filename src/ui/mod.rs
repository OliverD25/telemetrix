//! Everything drawn on top of the theme: banner, status bar, toast, overlays.

#[cfg(test)]
pub(crate) mod demo;
mod help_overlay;
mod log_overlay;
pub mod settings_overlay;
pub mod theme_options;
mod theme_picker;
pub mod update_group;

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
const STATUS_NAME: &str = " telemetrix ";
/// After the program name when a newer version waits.
pub const UPDATE_NOTICE: &str = " · update ready · u restart";
const UPDATE_NOTICE_SHORT: &str = " · u update";
/// The key hints after the program name, in the order they are dropped from.
const HINTS: [(&str, &str); 6] = [
    ("t", "themes"),
    ("s", "settings"),
    ("v", "view"),
    ("l", "log"),
    ("?", "help"),
    ("q", "quit"),
];

/// The longest key hint text that fits in `room` cells: all hints with their
/// words, then fewer from the end, then the keys alone, then fewer keys.
pub fn hint_text(room: usize) -> String {
    let join = |n: usize, words: bool| -> String {
        HINTS[..n]
            .iter()
            .map(|(key, word)| {
                if words {
                    format!(" · {key} {word}")
                } else {
                    format!(" · {key}")
                }
            })
            .collect()
    };
    // With fewer than two worded hints, the bare keys say more in less room.
    let worded = (2..=HINTS.len()).rev().map(|n| join(n, true));
    let bare = (1..=HINTS.len()).rev().map(|n| join(n, false));
    worded
        .chain(bare)
        .find(|t| t.chars().count() <= room)
        .unwrap_or_default()
}

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
        Overlay::ThemeOptions => {
            theme_picker::draw(frame, area, state);
            theme_options::draw(frame, area, state);
        }
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
    // On a narrow screen the memory figure goes first, so the program name
    // on the left stays readable.
    let memory = if memory.chars().count() + rest.chars().count() + STATUS_NAME.len()
        > usize::from(area.width)
    {
        String::new()
    } else {
        memory
    };
    let right_w = u16::try_from(memory.chars().count() + rest.chars().count()).unwrap_or(u16::MAX);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(area);
    let free = usize::from(left_area.width).saturating_sub(STATUS_NAME.len());
    let notice = match state.update {
        None => "",
        Some(_) if UPDATE_NOTICE.chars().count() <= free => UPDATE_NOTICE,
        Some(_) => UPDATE_NOTICE_SHORT,
    };
    let room = free.saturating_sub(notice.chars().count());
    let base = Style::new()
        .bg(Color::Rgb(38, 38, 38))
        .fg(Color::Rgb(190, 190, 190));
    let left = Line::from(vec![
        Span::styled(
            STATUS_NAME,
            Style::new()
                .bg(ACCENT)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(notice, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(hint_text(room)),
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
            nas("Y:", "archive", true, 24 * tb, 8 * tb),
            nas("Z:", "old_share", false, 0, 0),
        ]);
        let buf = render(&s, 120, 40);
        let t = text(&buf);
        assert!(t.contains(" Network "), "card title");
        assert!(t.contains("nas  M: P:"), "grouped row");
        assert!(t.contains("archive (Y:)"), "single row");
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
                    styled("footerbad", "", Some(MetricStyle::Bad)),
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
            assert_ne!(
                cell("stale").fg,
                WARN,
                "{theme}: the label of a row with a value"
            );
            assert_eq!(
                cell("footerbad").fg,
                WARN,
                "{theme}: no value, the label is red"
            );
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
    fn styled_spans_and_min_width_render_in_every_theme() {
        use crate::plugins::{
            MetricItem, MetricStyle, PluginCard, PluginData, PluginStatus, TextSpan,
        };
        let span = |text: &str, style| TextSpan {
            text: text.into(),
            style,
        };
        let row = |label: Vec<TextSpan>, value: Vec<TextSpan>, min_width| {
            let join = |s: &[TextSpan]| s.iter().map(|s| s.text.as_str()).collect::<String>();
            let mut m = MetricItem::text(join(&label), join(&value));
            m.style = Some(MetricStyle::Dim);
            m.label_spans = Some(label);
            m.value_spans = Some(value);
            m.min_width = min_width;
            m
        };
        for theme in themes::all().iter().map(|t| t.name()) {
            let mut s = state(theme);
            let metrics = vec![
                row(
                    vec![
                        span("CODE", None),
                        span(" brightrate", Some(MetricStyle::Bright)),
                    ],
                    vec![
                        span("quietgraph ", Some(MetricStyle::Dim)),
                        span("+rise%", Some(MetricStyle::Good)),
                    ],
                    None,
                ),
                row(
                    vec![],
                    vec![span("monthrow", Some(MetricStyle::Dim))],
                    Some(20),
                ),
                row(vec![span("hiddenrow", None)], vec![], Some(500)),
            ];
            let data = PluginData {
                id: "fx".into(),
                title: "FX".into(),
                metrics,
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
            let buf = render(&s, 45, 120);
            let color = |needle: &str| {
                cell_color(&buf, needle).unwrap_or_else(|| {
                    panic!(
                        "{theme}: {needle} not drawn:
{}",
                        text(&buf)
                    )
                })
            };
            let (code, rate, graph, rise) = (
                color("CODE"),
                color("brightrate"),
                color("quietgraph"),
                color("+rise%"),
            );
            assert_ne!(
                rate, graph,
                "{theme}: the rates are brighter than the graph"
            );
            assert_ne!(rise, graph, "{theme}: the change has its own colour");
            assert_ne!(
                code, rate,
                "{theme}: an unstyled span keeps the label colour"
            );
            color("monthrow");
            assert!(!text(&buf).contains("hiddenrow"), "{theme}: min_width 500");
        }
    }

    #[test]
    fn the_update_group_renders_in_the_s_box() {
        use crate::ui::update_group::Msg;
        use crate::update::{Release, Version};
        let release = |tag: &str| Release {
            tag: tag.into(),
            version: Version::parse(tag).unwrap(),
            assets: Vec::new(),
        };
        let own = format!("v{}", env!("CARGO_PKG_VERSION"));
        let own_version = Version::current();
        let newer = Version {
            patch: own_version.patch + 1,
            ..own_version.clone()
        };
        let newer_tag = format!("v{newer}");
        for theme in ["minimalist", "matrix"] {
            for tag in [own.as_str(), newer_tag.as_str()] {
                let mut s = state(theme);
                s.overlay = Overlay::Settings;
                s.apply(crate::event::AppEvent::Update(Msg::Checked(Ok(release(
                    tag,
                )))));
                let rows = settings_overlay::rows_for(&s);
                let sel = settings_overlay::selectable(&rows);
                s.settings_cursor = sel
                    .iter()
                    .position(|&i| {
                        matches!(&rows[i], settings_overlay::Row::Setting(s) if s.path == "update.check_interval_h")
                    })
                    .unwrap();
                let buf = render(&s, 100, 40);
                let lines: Vec<String> = (0..40)
                    .map(|y| (0..100).map(|x| buf[(x, y)].symbol()).collect::<String>())
                    .collect();
                let start = lines.iter().position(|l| l.contains("[update]")).unwrap();
                let group: Vec<&str> = lines[start..]
                    .iter()
                    .take_while(|l| !l.contains("Up/Down move"))
                    .map(|l| l.trim_end())
                    .collect();
                let text = group.join(
                    "
",
                );
                println!(
                    "{theme}, latest {tag}:
{text}
"
                );
                assert!(
                    text.contains(&format!("version               {}", own_version)),
                    "{text}"
                );
                assert!(
                    text.contains(&format!("latest                {tag} · checked ")),
                    "{text}"
                );
                assert!(text.contains("│  check now      "), "{text}");
                assert!(
                    text.contains("auto") && text.contains("check_interval_h"),
                    "{text}"
                );
                if tag == own {
                    assert!(!text.contains("install now"), "{text}");
                    assert_eq!(s.toast.as_ref().unwrap().0, "up to date");
                } else {
                    assert!(
                        text.contains(&format!("install now           {newer_tag}")),
                        "{text}"
                    );
                    assert_eq!(
                        s.toast.as_ref().unwrap().0,
                        format!("{newer_tag} available")
                    );
                }
            }
        }
    }

    fn cell_color(buf: &Buffer, needle: &str) -> Option<Color> {
        let cells = buf.content();
        let chars: Vec<&str> = cells.iter().map(|c| c.symbol()).collect();
        let n = needle.chars().count();
        let pos = chars.windows(n).position(|w| w.concat() == needle)?;
        Some(cells[pos].fg)
    }

    #[test]
    fn gpu_card_renders_at_three_two_and_one_columns() {
        for theme in crate::config::THEME_NAMES {
            for (w, h) in [(120, 40), (80, 40), (50, 60)] {
                let s = demo::state(theme);
                let t = text(&demo::render(&s, w, h));
                let at = format!("{theme} {w}x{h}");
                // Themes name the cards in their own style: `Disks`, `DISKS`, `disks`.
                let lower = t.to_lowercase();
                assert!(lower.contains(" gpu "), "{at}: card title");
                assert!(t.contains("NVIDIA GeForce"), "{at}: the name");
                let (vram, load): (&[&str], &str) = match *theme {
                    "nasa" => (&["GPU-VRU", "9.0 GiB", "24.0 GiB"], "37.0 %"),
                    _ => (&["vram", "9.0 GiB / 24.0 GiB"], " 37%"),
                };
                for needle in vram {
                    assert!(t.contains(needle), "{at}: {needle}");
                }
                assert!(t.contains("45.0 °C") && t.contains("112.4 W"), "{at}");
                assert!(t.contains(load), "{at}: the load");
                assert!(lower.contains(" disk") && lower.contains(" cpu "), "{at}");
            }
        }
    }

    #[test]
    fn gpu_card_hides_follows_units_and_warns_when_hot() {
        let mut s = demo::state("minimalist");
        s.config.gpu.enabled = false;
        assert!(!text(&demo::render(&s, 120, 40)).contains(" GPU "), "off");
        s.config.gpu.enabled = true;
        let mut snap = demo::snapshot();
        snap.gpus.clear();
        s.snapshot = Some(snap.clone());
        assert!(
            !text(&demo::render(&s, 120, 40)).contains(" GPU "),
            "no GPU"
        );

        let hot = crate::metrics::GpuMetric {
            name: "Hot One".into(),
            temp_c: Some(90.0),
            ..Default::default()
        };
        snap.gpus = vec![hot, demo::snapshot().gpus.remove(0)];
        s.snapshot = Some(snap);
        let buf = demo::render(&s, 120, 40);
        let t = text(&buf);
        assert!(t.contains(" GPU 1 ") && t.contains(" GPU 2 "), "{t}");
        assert_eq!(cell_color(&buf, "90.0 °C"), Some(WARN));
        assert_ne!(cell_color(&buf, "45.0 °C"), Some(WARN));
        s.config.units.temperature = crate::config::TempUnit::Fahrenheit;
        let t = text(&demo::render(&s, 120, 40));
        assert!(t.contains("194.0 °F") && t.contains("113.0 °F"), "{t}");
    }

    #[test]
    fn long_theme_names_shorten_the_hints_not_the_numbers() {
        assert_eq!(
            hint_text(60),
            " · t themes · s settings · v view · l log · ? help · q quit"
        );
        assert_eq!(hint_text(45), " · t themes · s settings · v view · l log");
        assert_eq!(hint_text(25), " · t themes · s settings");
        assert_eq!(hint_text(21), " · t · s · v · l · ?");
        assert_eq!(hint_text(9), " · t · s");
        assert_eq!(hint_text(2), "");
        let mut s = state("ascii-dashboard");
        s.record_memory_with(
            crate::selfmem::SelfMemory {
                working_set: 11 * crate::selfmem::MB,
                peak_working_set: None,
                private: None,
            },
            false,
            None,
        );
        for w in [100, 80, 64] {
            let buf = render(&s, w, 30);
            let status: String = (0..w).map(|x| buf[(x, 29)].symbol()).collect();
            let at = format!("{w}: {status}");
            assert!(status.starts_with(" telemetrix  · t"), "{at}");
            assert!(
                status.ends_with(" self 11.0 MB · ascii-dashboard · 15 fps "),
                "{at}"
            );
            assert!(!status.contains("q q"), "no word cut in half: {at}");
        }
        let buf = render(&s, 100, 30);
        let status: String = (0..100).map(|x| buf[(x, 29)].symbol()).collect();
        assert!(status.contains("s settings · v view · l log"), "{status}");
    }

    #[test]
    fn a_ready_update_shows_in_the_status_bar() {
        let mut s = state("ascii-dashboard");
        s.update = Some(crate::update::Pending::Ready(
            crate::update::Version::parse("0.4.0").unwrap(),
        ));
        for w in [120, 100] {
            let buf = render(&s, w, 30);
            let status: String = (0..w).map(|x| buf[(x, 29)].symbol()).collect();
            assert!(
                status.starts_with(" telemetrix  · update ready · u restart"),
                "{w}: {status}"
            );
            assert!(
                status.ends_with(" ascii-dashboard · 15 fps "),
                "{w}: {status}"
            );
            assert_eq!(cell_color(&buf, "update ready"), Some(ACCENT));
        }
        let buf = render(&s, 64, 30);
        let status: String = (0..64).map(|x| buf[(x, 29)].symbol()).collect();
        assert!(status.starts_with(" telemetrix  · u update"), "{status}");
        assert!(status.ends_with(" ascii-dashboard · 15 fps "), "{status}");
    }

    #[test]
    fn the_options_panel_draws_in_every_theme_and_size() {
        for theme in crate::config::THEME_NAMES {
            let mut s = state(theme);
            s.overlay = Overlay::ThemeOptions;
            s.theme_panel = Some(theme_options::ThemePanel {
                theme: theme.to_string(),
                cursor: 0,
                before: s.config.clone(),
                changed: Vec::new(),
            });
            for (w, h) in [(120, 40), (80, 24), (40, 10)] {
                let buf = render(&s, w, h);
                let t = text(&buf);
                let at = format!("{theme} {w}x{h}");
                assert!(t.contains(&format!("{theme} options")) || w < 60, "{at}");
                assert!(t.contains("CPU view"), "{at}");
                let status: String = (0..w).map(|x| buf[(x, h - 1)].symbol()).collect();
                assert!(status.starts_with(" telemetrix "), "{at}");
            }
            let t = text(&render(&s, 120, 40));
            assert!(
                t.contains("Save options for") && t.contains("[x] cpu"),
                "{theme}"
            );
            assert!(t.contains("Esc back"), "{theme}: the hint");
        }
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
        let rows = settings_overlay::rows_for(&s);
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
        for theme in crate::config::THEME_NAMES {
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
