//! A dashboard with made-up data, for render tests and the README screenshot.
//! `cargo test readme_screenshot -- --ignored --nocapture` prints it.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use crate::app::AppState;
use crate::config::{Config, ConfigStatus};
use crate::event::AppEvent;
use crate::metrics::{DiskMetric, GpuMetric, SystemSnapshot};
use crate::plugins::{MetricItem, MetricStyle, PluginCard, PluginData, PluginStatus};
use crate::selfmem::{MB, SelfMemory};
use crate::themes;

const GIB: u64 = 1 << 30;

fn trend(label: &str, value: &str, points: &[f32]) -> MetricItem {
    let mut m = MetricItem::text(label, value);
    m.trend = Some(points.to_vec());
    m
}

fn dim(label: &str, value: &str) -> MetricItem {
    let mut m = MetricItem::text(label, value);
    m.style = Some(MetricStyle::Dim);
    m
}

fn card(id: &str, title: &str, metrics: Vec<MetricItem>) -> (String, PluginCard) {
    let data = PluginData {
        id: id.into(),
        title: title.into(),
        metrics,
        error: None,
        lua_bytes: None,
    };
    let card = PluginCard {
        data,
        status: PluginStatus::Ok,
    };
    (id.into(), card)
}

pub fn snapshot() -> SystemSnapshot {
    let disk = |mount: &str, label: &str, used: u64, total: u64| DiskMetric {
        mount: mount.into(),
        label: Some(label.into()),
        used_bytes: used * GIB,
        total_bytes: total * GIB,
    };
    SystemSnapshot {
        cpu_usage: 23.0,
        cpu_temp: Some(52.0),
        ram_used_bytes: 18 * GIB + GIB / 3,
        ram_total_bytes: 64 * GIB,
        swap_used_bytes: GIB / 2,
        swap_total_bytes: 8 * GIB,
        disks: vec![
            disk("C:\\", "System", 412, 931),
            disk("D:\\", "Games", 1620, 1863),
        ],
        gpus: vec![GpuMetric {
            name: "NVIDIA GeForce RTX 4090".into(),
            usage_pct: Some(37.0),
            mem_used_bytes: Some(9 * GIB),
            mem_total_bytes: Some(24 * GIB),
            temp_c: Some(45.0),
            power_w: Some(112.4),
        }],
        ..SystemSnapshot::default()
    }
}

pub fn state(theme: &str) -> AppState {
    let mut cfg = Config::default();
    cfg.general.theme = theme.into();
    cfg.disks.show_network = false;
    cfg.gpu.enabled = true;
    let mut state = AppState::new(cfg, ConfigStatus::Ok);
    for i in 0..60 {
        let mut s = snapshot();
        s.cpu_usage = 20.0 + 15.0 * ((i as f32) / 5.0).sin().abs();
        state.apply(AppEvent::Metrics(s));
    }
    state.apply(AppEvent::Metrics(snapshot()));
    state.plugins.extend([
        card(
            "clock",
            "Clock",
            vec![
                MetricItem::text("time", "14:32:07"),
                MetricItem::text("date", "Sat 26 Sep 2026"),
            ],
        ),
        card(
            "crypto",
            "Crypto",
            vec![
                MetricItem::text("BTC", "84,853 USDT"),
                trend("7d", "+2.1%", &[81.2, 82.0, 80.9, 83.5, 84.1, 83.9, 84.8]),
                MetricItem::text("ETH", "2,725 USDT"),
                trend("7d", "-1.4%", &[2.8, 2.79, 2.75, 2.77, 2.7, 2.74, 2.72]),
            ],
        ),
        card(
            "weather",
            "Weather",
            vec![
                MetricItem::text("now", "17°C, wind 3 m/s"),
                MetricItem::text("Sun", "9..18°C cloudy 20%"),
                MetricItem::text("Mon", "8..15°C rain 70%"),
                dim("data: Open-Meteo.com", ""),
            ],
        ),
    ]);
    state.record_memory_with(
        SelfMemory {
            working_set: 11 * MB + MB / 2,
            peak_working_set: None,
            private: None,
        },
        false,
        None,
    );
    state.dirty = false;
    state
}

pub fn render(state: &AppState, w: u16, h: u16) -> Buffer {
    let mut theme = themes::all().remove(state.theme_idx);
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| {
            theme.tick(f.area(), &state.config);
            super::draw(f, state, theme.as_mut());
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

/// The buffer as text lines, trailing spaces removed.
pub fn lines(buf: &Buffer) -> String {
    let area = buf.area;
    (0..area.height)
        .map(|y| {
            let line: String = (0..area.width).map(|x| buf[(x, y)].symbol()).collect();
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
#[ignore = "prints the README screenshot"]
fn readme_screenshot() {
    // Colour is most of what tells the other themes apart, and text has none.
    for name in ["minimalist", "matrix"] {
        println!("--- {name}\n{}", lines(&render(&state(name), 100, 30)));
    }
}

#[test]
#[ignore = "prints a small render of every theme"]
fn theme_samples() {
    for name in crate::config::THEME_NAMES {
        println!("--- {name}\n{}", lines(&render(&state(name), 72, 18)));
    }
}
