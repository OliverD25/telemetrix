use std::process::ExitCode;
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::cli::Flags;
use crate::config::{self, Config};
use crate::format;
use crate::metrics::{self, SystemSnapshot, worker};

fn round1(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}

fn used_total(used: u64, total: u64) -> Value {
    json!({ "used_bytes": used, "total_bytes": total })
}

pub fn to_json(
    s: &SystemSnapshot,
    host: &str,
    timestamp: &str,
    plugins: Option<Vec<Value>>,
) -> Value {
    let disks: Vec<Value> = s
        .disks
        .iter()
        .map(|d| json!({ "mount": d.mount, "used_bytes": d.used_bytes, "total_bytes": d.total_bytes }))
        .collect();
    let gpus: Vec<Value> = s
        .gpus
        .iter()
        .map(|g| json!({ "name": g.name, "usage_pct": g.usage_pct.map(round1) }))
        .collect();
    let mut doc = json!({
        "schema": 1,
        "timestamp": timestamp,
        "host": host,
        "system": {
            "cpu_usage_pct": round1(s.cpu_usage),
            "cpu_temp_c": s.cpu_temp.map(round1),
            "ram": used_total(s.ram_used_bytes, s.ram_total_bytes),
            "swap": used_total(s.swap_used_bytes, s.swap_total_bytes),
            "disks": disks,
            "gpus": gpus,
        },
    });
    if let Some(plugins) = plugins {
        doc["plugins"] = Value::Array(plugins);
    }
    doc
}

pub fn to_text(s: &SystemSnapshot, host: &str, timestamp: &str, cfg: &Config) -> String {
    let unit = cfg.units.bytes;
    let used_of = |used, total| {
        format!(
            "{} / {}  ({})",
            format::bytes(used, unit),
            format::bytes(total, unit),
            format::pct(metrics::pct(used, total))
        )
    };
    let mut rows = vec![
        ("host".to_string(), host.to_string()),
        ("time".to_string(), timestamp.to_string()),
        ("cpu".to_string(), format::pct(s.cpu_usage)),
        (
            "cpu temp".to_string(),
            s.cpu_temp.map_or("not available".to_string(), |t| {
                format::temperature(t, cfg.units.temperature)
            }),
        ),
        (
            "ram".to_string(),
            used_of(s.ram_used_bytes, s.ram_total_bytes),
        ),
        (
            "swap".to_string(),
            used_of(s.swap_used_bytes, s.swap_total_bytes),
        ),
    ];
    for d in &s.disks {
        rows.push((
            format!("disk {}", d.mount),
            used_of(d.used_bytes, d.total_bytes),
        ));
    }
    let width = rows
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    rows.iter()
        .map(|(k, v)| format!("{k:<width$}  {v}\n"))
        .collect()
}

pub fn host_name() -> String {
    sysinfo::System::host_name().unwrap_or_else(|| "unknown".into())
}

pub fn run(json: bool, with_plugins: bool, flags: &Flags) -> ExitCode {
    let (_, cfg, _) = config::load_effective(flags);
    let snapshot = worker::read_once();
    let host = host_name();
    let timestamp = format::utc_timestamp(SystemTime::now());
    if json {
        let plugins = with_plugins.then(|| super::plugin_cmd::run_all_once(&cfg));
        println!("{:#}", to_json(&snapshot, &host, &timestamp, plugins));
    } else {
        print!("{}", to_text(&snapshot, &host, &timestamp, &cfg));
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::DiskMetric;

    #[test]
    fn json_shape() {
        let s = SystemSnapshot {
            cpu_usage: 12.34,
            ram_total_bytes: 10,
            disks: vec![DiskMetric {
                mount: "C:\\".into(),
                used_bytes: 1,
                total_bytes: 2,
            }],
            ..SystemSnapshot::default()
        };
        let doc = to_json(&s, "PC", "2026-09-25T20:00:00Z", None);
        assert_eq!(doc["schema"], 1);
        assert_eq!(doc["system"]["cpu_usage_pct"], 12.3);
        assert!(doc["system"]["cpu_temp_c"].is_null());
        assert_eq!(doc["system"]["disks"][0]["mount"], "C:\\");
        assert_eq!(doc["system"]["gpus"], json!([]));
        assert!(doc.get("plugins").is_none());
        let doc = to_json(&s, "PC", "t", Some(Vec::new()));
        assert_eq!(doc["plugins"], json!([]));
    }

    #[test]
    fn text_is_aligned() {
        let s = SystemSnapshot::default();
        let text = to_text(&s, "PC", "t", &Config::default());
        assert!(text.contains("host      PC\n"));
        assert!(text.contains("cpu temp  not available\n"));
    }
}
