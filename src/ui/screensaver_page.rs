//! The Screensaver page of the `s` box (decision 57): the switch and the
//! idle limit that the watcher follows, and whether the watcher runs. The
//! box never installs or removes the task; it tells you the command.

use crate::app::AppState;
use crate::commands::screensaver;
use crate::config;
use crate::ui::settings_overlay::Row;

/// What the box knows about the watcher.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WatcherState {
    /// The look runs in a thread since the box opened.
    #[default]
    Checking,
    Running,
    /// The task is installed, but no watcher runs.
    Stopped,
    NotInstalled,
    /// Linux: an idle daemon starts the dashboard.
    WindowsOnly,
}

impl WatcherState {
    /// The watcher's mutex, then `schtasks` when no watcher runs. That
    /// starts a process, so the box runs this in a thread when it opens,
    /// never while it draws.
    pub fn probe() -> Self {
        if !cfg!(windows) {
            Self::WindowsOnly
        } else if screensaver::watcher_running(None) {
            Self::Running
        } else if screensaver::task_installed(None) {
            Self::Stopped
        } else {
            Self::NotInstalled
        }
    }

    /// The `watcher` row's value, and a second row that says what to run.
    fn text(self) -> (&'static str, Option<(&'static str, &'static str)>) {
        match self {
            Self::Checking => ("checking…", None),
            Self::Running => ("running", None),
            Self::Stopped => (
                "installed, not running",
                Some(("run in a terminal", "telemetrix screensaver update")),
            ),
            Self::NotInstalled => (
                "not installed",
                Some(("run in a terminal", "telemetrix screensaver install")),
            ),
            Self::WindowsOnly => (
                "Windows only",
                Some(("Linux recipe", "telemetrix screensaver install")),
            ),
        }
    }
}

pub fn rows(state: &AppState) -> Vec<Row> {
    let mut rows: Vec<Row> = ["screensaver.enabled", "screensaver.idle_minutes"]
        .iter()
        .filter_map(|p| config::find(p))
        .map(Row::Setting)
        .collect();
    let (value, hint) = state.watcher.text();
    rows.push(Row::Info {
        label: "watcher",
        value: value.into(),
    });
    if let Some((label, value)) = hint {
        rows.push(Row::Info {
            label,
            value: value.into(),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ConfigStatus};
    use crate::ui::settings_overlay::{key_label, value_text};

    fn shown(watcher: WatcherState) -> Vec<String> {
        let mut state = AppState::new(Config::default(), ConfigStatus::Ok);
        state.watcher = watcher;
        rows(&state)
            .iter()
            .map(|r| format!("{} = {}", key_label(r), value_text(r, &state.config)))
            .collect()
    }

    #[test]
    fn the_page_says_what_the_watcher_does_and_what_to_run() {
        let head = ["enabled = on", "idle_minutes = 10"];
        let page = |w| shown(w)[2..].join(" | ");
        assert_eq!(shown(WatcherState::Running)[..2], head);
        assert_eq!(page(WatcherState::Running), "watcher = running");
        assert_eq!(page(WatcherState::Checking), "watcher = checking…");
        assert_eq!(
            page(WatcherState::Stopped),
            "watcher = installed, not running | \
             run in a terminal = telemetrix screensaver update"
        );
        assert_eq!(
            page(WatcherState::NotInstalled),
            "watcher = not installed | run in a terminal = telemetrix screensaver install"
        );
        assert_eq!(
            page(WatcherState::WindowsOnly),
            "watcher = Windows only | Linux recipe = telemetrix screensaver install"
        );
    }
}
