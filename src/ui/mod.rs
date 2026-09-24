//! Everything drawn on top of the theme: banner, status bar, toast, overlays.

mod help_overlay;
mod log_overlay;
pub mod settings_overlay;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};

use crate::app::{AppState, Overlay};
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
        Overlay::Log => log_overlay::draw(frame, area, &state.log),
        Overlay::Help => help_overlay::draw(frame, area),
    }
}

fn draw_status(frame: &mut Frame, area: Rect, state: &AppState) {
    let paused = if state.paused { " · PAUSED" } else { "" };
    let right = format!(
        " {} · {} fps{paused} ",
        state.theme_name(),
        state.config.general.fps
    );
    let right_w = u16::try_from(right.chars().count()).unwrap_or(u16::MAX);
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
        Span::raw(" · t theme · s settings · l log · ? help · q quit"),
    ]);
    frame.render_widget(Paragraph::new(left).style(base), left_area);
    frame.render_widget(
        Paragraph::new(right).style(base.fg(Color::White).add_modifier(Modifier::BOLD)),
        right_area,
    );
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
    use crate::themes::common::WARN;
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
    fn tiny_terminal_shows_the_notice() {
        assert!(text(&render(&state("minimalist"), 30, 5)).contains("terminal too small"));
        assert!(text(&render(&state("matrix"), 39, 20)).contains(TOO_SMALL));
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
