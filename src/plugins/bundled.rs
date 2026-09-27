//! The default plugins, built into the program and copied into the plugin
//! home (`<settings folder>/plugins`) on every start.
//!
//! `<home>/.bundled.json` records the hash of each file as telemetrix wrote
//! it. That tells a file the user edited (keep it) from one nobody touched
//! (update it), and a file the user deleted (leave it deleted) from one that
//! was never installed (install it).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config;

pub const RECORD: &str = ".bundled.json";

/// File name and content of every built-in plugin.
pub const FILES: [(&str, &str); 8] = [
    ("clock.lua", include_str!("../../plugins/clock.lua")),
    ("crypto.lua", include_str!("../../plugins/crypto.lua")),
    ("currency.lua", include_str!("../../plugins/currency.lua")),
    ("hosts.lua", include_str!("../../plugins/hosts.lua")),
    (
        "network_ping.lua",
        include_str!("../../plugins/network_ping.lua"),
    ),
    ("speedtest.lua", include_str!("../../plugins/speedtest.lua")),
    ("uptime.lua", include_str!("../../plugins/uptime.lua")),
    ("weather.lua", include_str!("../../plugins/weather.lua")),
];

/// FNV-1a, 64 bit: stable across Rust versions, unlike the std hasher.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn hash_hex(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a64(bytes))
}

/// `<folder of the settings file>/plugins`.
pub fn home(settings_path: &Path) -> PathBuf {
    settings_folder(settings_path).join("plugins")
}

fn settings_folder(settings_path: &Path) -> PathBuf {
    let parent = settings_path.parent().unwrap_or(Path::new(""));
    std::path::absolute(parent).unwrap_or_else(|_| parent.to_path_buf())
}

/// The folder plugins are read from: `general.plugins_dir` (which
/// `--plugins-dir` has already replaced with an absolute path), relative to
/// the settings file's folder; empty means the home.
pub fn plugins_dir(configured: &Path, settings_path: &Path) -> PathBuf {
    if configured.as_os_str().is_empty() {
        home(settings_path)
    } else if configured.is_absolute() {
        configured.to_path_buf()
    } else {
        settings_folder(settings_path).join(configured)
    }
}

/// A built-in name from `weather` or `weather.lua`.
pub fn file_name(name: &str) -> Option<&'static str> {
    let wanted = name.strip_suffix(".lua").unwrap_or(name);
    FILES
        .iter()
        .map(|(f, _)| *f)
        .find(|f| f.strip_suffix(".lua") == Some(wanted))
}

fn read_record(home: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(home.join(RECORD))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_record(home: &Path, record: &BTreeMap<String, String>) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(record).unwrap_or_default();
    config::write_atomic(&home.join(RECORD), &text)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed,
    Updated,
    UpToDate,
    /// Recorded as installed but missing: the user deleted it.
    KeptDeleted,
    /// Its content differs from what telemetrix wrote: the user edited it.
    KeptEdited,
    /// `--force` wrote it over an edited or deleted file.
    Replaced,
}

impl Outcome {
    pub fn describe(self, file: &str) -> String {
        let stem = file.strip_suffix(".lua").unwrap_or(file);
        match self {
            Outcome::Installed => format!("{file}: installed"),
            Outcome::Updated => format!("{file}: updated to the new built-in version"),
            Outcome::UpToDate => format!("{file}: up to date"),
            Outcome::KeptDeleted => format!(
                "{file}: deleted by you, not restored. \
                 telemetrix plugin install --force {stem} restores it."
            ),
            Outcome::KeptEdited => format!(
                "{file} was changed by you; the new built-in version is not installed. \
                 telemetrix plugin install --force {stem} replaces it."
            ),
            Outcome::Replaced => format!("{file}: replaced with the built-in version"),
        }
    }

    /// Whether the start-up sync logs it (up-to-date files stay quiet).
    pub fn worth_logging(self) -> bool {
        self != Outcome::UpToDate
    }
}

/// What to do for one file. `force` writes it whatever the user did.
fn decide(
    present: Option<&str>,
    recorded: Option<&str>,
    bundled: &str,
    force: bool,
) -> (Outcome, bool) {
    let want = hash_hex(bundled.as_bytes());
    match present {
        None if recorded.is_none() => (Outcome::Installed, true),
        None if force => (Outcome::Replaced, true),
        None => (Outcome::KeptDeleted, false),
        Some(text) => {
            let have = hash_hex(text.as_bytes());
            let untouched = recorded == Some(have.as_str()) || have == want;
            match (untouched, have == want) {
                (true, true) => (Outcome::UpToDate, false),
                (true, false) => (Outcome::Updated, true),
                (false, _) if force => (Outcome::Replaced, true),
                (false, _) => (Outcome::KeptEdited, false),
            }
        }
    }
}

/// Brings the built-in plugins in `home` up to date. `only` limits it to
/// some file names; `force` overwrites edited files and restores deleted
/// ones. Files that are not built in are never touched.
pub fn sync(home: &Path, only: &[&str], force: bool) -> std::io::Result<Vec<(String, Outcome)>> {
    std::fs::create_dir_all(home)?;
    let mut record = read_record(home);
    let mut report = Vec::new();
    for (file, content) in FILES {
        if !only.is_empty() && !only.contains(&file) {
            continue;
        }
        let path = home.join(file);
        let present = std::fs::read_to_string(&path).ok();
        let (outcome, write) = decide(
            present.as_deref(),
            record.get(file).map(String::as_str),
            content,
            force,
        );
        if write {
            config::write_atomic(&path, content)?;
        }
        if write || outcome == Outcome::UpToDate {
            record.insert(file.to_string(), hash_hex(content.as_bytes()));
        }
        report.push((file.to_string(), outcome));
    }
    write_record(home, &record)?;
    Ok(report)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    BuiltIn,
    BuiltInEdited,
    Yours,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::BuiltIn => "built-in",
            Origin::BuiltInEdited => "built-in-edited",
            Origin::Yours => "yours",
        }
    }
}

/// Where a plugin file came from, judged by its name and content.
pub fn origin(path: &Path) -> Origin {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let Some((file, content)) = FILES.iter().find(|(f, _)| *f == name) else {
        return Origin::Yours;
    };
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let have = hash_hex(text.as_bytes());
    let dir = path.parent().unwrap_or(Path::new(""));
    let recorded = read_record(dir).get(*file).cloned();
    if have == hash_hex(content.as_bytes()) || recorded.as_deref() == Some(have.as_str()) {
        Origin::BuiltIn
    } else {
        Origin::BuiltInEdited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "telemetrix-bundled-sync-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn outcome(report: &[(String, Outcome)], file: &str) -> Outcome {
        report.iter().find(|(f, _)| f == file).unwrap().1
    }

    #[test]
    fn fnv1a_matches_the_published_test_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn missing_and_never_installed_is_written() {
        let home = temp("fresh");
        let report = sync(&home, &[], false).unwrap();
        assert!(report.iter().all(|(_, o)| *o == Outcome::Installed));
        for (file, content) in FILES {
            assert_eq!(std::fs::read_to_string(home.join(file)).unwrap(), content);
        }
        let record = read_record(&home);
        assert_eq!(record.len(), FILES.len());
        let again = sync(&home, &[], false).unwrap();
        assert!(again.iter().all(|(_, o)| *o == Outcome::UpToDate));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn missing_but_recorded_stays_deleted() {
        let home = temp("deleted");
        sync(&home, &[], false).unwrap();
        std::fs::remove_file(home.join("speedtest.lua")).unwrap();
        let report = sync(&home, &[], false).unwrap();
        assert_eq!(outcome(&report, "speedtest.lua"), Outcome::KeptDeleted);
        assert!(!home.join("speedtest.lua").exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn untouched_old_version_is_updated() {
        let home = temp("update");
        sync(&home, &[], false).unwrap();
        // As if an older telemetrix had installed an older clock.lua.
        let old = "-- old clock\nreturn { update = function() return { metrics = {} } end }\n";
        std::fs::write(home.join("clock.lua"), old).unwrap();
        let mut record = read_record(&home);
        record.insert("clock.lua".into(), hash_hex(old.as_bytes()));
        write_record(&home, &record).unwrap();
        let report = sync(&home, &[], false).unwrap();
        assert_eq!(outcome(&report, "clock.lua"), Outcome::Updated);
        assert_eq!(
            std::fs::read_to_string(home.join("clock.lua")).unwrap(),
            FILES[0].1
        );
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn edited_file_is_never_touched_and_other_files_are_left_alone() {
        let home = temp("edited");
        sync(&home, &[], false).unwrap();
        let mine = "-- my weather\nreturn { update = function() return { metrics = {} } end }\n";
        std::fs::write(home.join("weather.lua"), mine).unwrap();
        std::fs::write(home.join("own.lua"), "-- not built in").unwrap();
        let report = sync(&home, &[], false).unwrap();
        assert_eq!(outcome(&report, "weather.lua"), Outcome::KeptEdited);
        assert_eq!(
            std::fs::read_to_string(home.join("weather.lua")).unwrap(),
            mine
        );
        assert_eq!(
            std::fs::read_to_string(home.join("own.lua")).unwrap(),
            "-- not built in"
        );
        assert_eq!(
            Outcome::KeptEdited.describe("weather.lua"),
            "weather.lua was changed by you; the new built-in version is not installed. \
             telemetrix plugin install --force weather replaces it."
        );
        assert_eq!(origin(&home.join("weather.lua")), Origin::BuiltInEdited);
        assert_eq!(origin(&home.join("clock.lua")), Origin::BuiltIn);
        assert_eq!(origin(&home.join("own.lua")), Origin::Yours);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_file_already_there_that_matches_is_adopted() {
        let home = temp("adopt");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("clock.lua"), FILES[0].1).unwrap();
        std::fs::write(home.join("uptime.lua"), "-- someone else's uptime").unwrap();
        let report = sync(&home, &[], false).unwrap();
        assert_eq!(outcome(&report, "clock.lua"), Outcome::UpToDate);
        assert_eq!(outcome(&report, "uptime.lua"), Outcome::KeptEdited);
        assert!(read_record(&home).contains_key("clock.lua"));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn force_replaces_edited_and_restores_deleted_in_scope_only() {
        let home = temp("force");
        sync(&home, &[], false).unwrap();
        std::fs::write(home.join("weather.lua"), "-- mine").unwrap();
        std::fs::remove_file(home.join("speedtest.lua")).unwrap();
        let report = sync(&home, &["weather.lua"], true).unwrap();
        assert_eq!(report.len(), 1, "only the named file");
        assert_eq!(outcome(&report, "weather.lua"), Outcome::Replaced);
        assert!(
            !home.join("speedtest.lua").exists(),
            "not named: stays deleted"
        );
        let report = sync(&home, &[], true).unwrap();
        assert_eq!(outcome(&report, "speedtest.lua"), Outcome::Replaced);
        assert_eq!(outcome(&report, "weather.lua"), Outcome::UpToDate);
        assert!(home.join("speedtest.lua").is_file());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn plugin_folder_rules() {
        let settings = Path::new("/cfg/telemetrix/telemetrix.toml");
        let home = home(settings);
        assert!(home.ends_with("telemetrix/plugins"));
        assert_eq!(plugins_dir(Path::new(""), settings), home);
        assert_eq!(
            plugins_dir(Path::new("plugins"), settings),
            home,
            "the v0.1 template value means the home"
        );
        assert!(plugins_dir(Path::new("more"), settings).ends_with("telemetrix/more"));
        let abs = std::env::temp_dir().join("elsewhere");
        assert_eq!(plugins_dir(&abs, settings), abs);
        assert_eq!(file_name("weather"), Some("weather.lua"));
        assert_eq!(file_name("weather.lua"), Some("weather.lua"));
        assert_eq!(file_name("nope"), None);
    }
}
