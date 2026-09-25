//! A small JSON file per plugin (decision 41): last values, caches and
//! history that should survive a restart.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const MAX_BYTES: usize = 64 * 1024;

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Sets the data folder from `--data-dir`; call once at start.
pub fn set_data_dir(dir: Option<PathBuf>) {
    if let Some(dir) = dir {
        let _ = DATA_DIR.set(dir);
    }
}

/// `--data-dir`, else `%LOCALAPPDATA%\telemetrix` on Windows and
/// `$XDG_DATA_HOME/telemetrix` or `~/.local/share/telemetrix` elsewhere.
pub fn data_dir() -> PathBuf {
    DATA_DIR
        .get_or_init(|| default_dir(|k| std::env::var_os(k)))
        .clone()
}

fn default_dir(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> PathBuf {
    let base = if cfg!(windows) {
        env("LOCALAPPDATA").map(PathBuf::from)
    } else {
        env("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("telemetrix")
}

/// `^[a-z0-9_]{1,40}$`: the id becomes a file name, so nothing else is allowed.
pub fn valid_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn file(dir: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err(format!(
            "the store needs a plugin id of a-z, 0-9 and _ (1..40 characters), not {id:?}"
        ));
    }
    Ok(dir.join("plugins").join(format!("{id}.json")))
}

/// The stored value, or `None` when nothing was stored yet.
pub fn load(dir: &Path, id: &str) -> Result<Option<serde_json::Value>, String> {
    let path = file(dir, id)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("the stored data is not valid JSON: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

pub fn save(dir: &Path, id: &str, value: &serde_json::Value) -> Result<(), String> {
    let path = file(dir, id)?;
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    if text.len() > MAX_BYTES {
        return Err(format!(
            "the store is limited to {} KB, this is {} KB",
            MAX_BYTES / 1024,
            text.len().div_ceil(1024)
        ));
    }
    crate::config::write_atomic(&path, &text)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("telemetrix-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trip_limit_and_ids() {
        let dir = temp("rt");
        assert_eq!(load(&dir, "clock").unwrap(), None);
        save(&dir, "clock", &json!({ "last": [1, 2, 3] })).unwrap();
        assert_eq!(
            load(&dir, "clock").unwrap(),
            Some(json!({ "last": [1, 2, 3] }))
        );
        assert!(dir.join("plugins").join("clock.json").is_file());
        let big = json!({ "x": "a".repeat(MAX_BYTES) });
        assert!(save(&dir, "clock", &big).unwrap_err().contains("64 KB"));
        for bad in ["", "Clock", "../x", "a-b", "a.b", &"x".repeat(41)] {
            assert!(!valid_id(bad), "{bad:?}");
            assert!(save(&dir, bad, &json!(1)).is_err());
        }
        assert!(valid_id("network_ping_2"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn default_folder_per_platform() {
        let env = |k: &str| match k {
            "LOCALAPPDATA" => Some("L".into()),
            "HOME" => Some("/h".into()),
            _ => None,
        };
        let expected = if cfg!(windows) {
            PathBuf::from("L").join("telemetrix")
        } else {
            PathBuf::from("/h/.local/share/telemetrix")
        };
        assert_eq!(default_dir(env), expected);
    }
}
