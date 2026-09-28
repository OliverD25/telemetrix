//! The `update` group of the `s` box: this version, the last release check,
//! and `check now` / `install now`, which run in a background thread and
//! report back with `Msg`.

use std::time::SystemTime;

use crate::format;
use crate::update::{CheckError, Release, Step, Version};

/// What the background thread of a check or an install sends back.
#[derive(Debug)]
pub enum Msg {
    Checked(Result<Release, CheckError>),
    Progress(Step),
    Installed(Version),
    InstallFailed(String),
}

/// What runs in the background; only one thing at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Job {
    #[default]
    Idle,
    Checking,
    /// Percent, when the server said the size.
    Downloading(Option<u8>),
    Verifying,
    Installing,
}

impl Job {
    pub fn of(step: Step) -> Self {
        match step {
            Step::Downloading { done, total } => Self::Downloading(
                total
                    .filter(|t| *t > 0)
                    .map(|t| (done.min(t) * 100 / t) as u8),
            ),
            Step::Verifying => Self::Verifying,
            Step::Installing => Self::Installing,
        }
    }
}

#[derive(Debug, Default)]
pub struct UpdateGroup {
    /// The last finished check, when it ended: the latest release, or a
    /// short reason why the check failed.
    pub checked: Option<(Result<Release, String>, SystemTime)>,
    pub job: Job,
    /// A short reason why the last install failed.
    pub install_error: Option<String>,
    /// Installed from this box; `u` restarts into it.
    pub installed: Option<Version>,
}

/// What a message changed that the dashboard must show.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub toast: Option<String>,
    pub log: Vec<String>,
    pub installed: Option<Version>,
}

impl UpdateGroup {
    /// The release of the last check when it is newer than `own`.
    pub fn newer(&self, own: &Version) -> Option<&Release> {
        match &self.checked {
            Some((Ok(r), _)) if r.version > *own => Some(r),
            _ => None,
        }
    }

    /// Starts a check; `false` when a check or an install already runs.
    pub fn start_check(&mut self) -> bool {
        if self.job != Job::Idle {
            return false;
        }
        self.job = Job::Checking;
        true
    }

    /// The release to install, when one is known and nothing runs.
    pub fn start_install(&mut self, own: &Version) -> Result<Release, &'static str> {
        if self.job != Job::Idle {
            return Err("already running");
        }
        if self.installed.is_some() {
            return Err("update ready · u restart");
        }
        let release = self.newer(own).cloned().ok_or("no newer version known")?;
        self.install_error = None;
        self.job = Job::Downloading(None);
        Ok(release)
    }

    pub fn apply(&mut self, msg: Msg, own: &Version, now: SystemTime) -> Outcome {
        let mut out = Outcome::default();
        match msg {
            Msg::Checked(Ok(release)) => {
                self.job = Job::Idle;
                let toast = if release.version > *own {
                    format!("{} available", release.tag)
                } else if release.version == *own {
                    "up to date".to_string()
                } else {
                    format!("this build is newer than {}", release.tag)
                };
                out.log.push(format!(
                    "update: {} is the latest release; this is {own}",
                    release.tag
                ));
                out.toast = Some(toast);
                self.checked = Some((Ok(release), now));
            }
            Msg::Checked(Err(e)) => {
                self.job = Job::Idle;
                let reason = short_check_error(&e);
                out.log.push(format!("warning: update: {e}"));
                out.toast = Some(format!("check failed: {reason}"));
                self.checked = Some((Err(reason), now));
            }
            Msg::Progress(step) => self.job = Job::of(step),
            Msg::Installed(v) => {
                self.job = Job::Idle;
                out.log.push(format!(
                    "update: installed telemetrix {v}; press u to restart"
                ));
                out.toast = Some("update ready · u restart".into());
                out.installed = Some(v.clone());
                self.installed = Some(v);
            }
            Msg::InstallFailed(e) => {
                self.job = Job::Idle;
                let reason = short_install_error(&e);
                out.log.push(format!("warning: update: {e}"));
                out.toast = Some(format!("install failed: {reason}"));
                self.install_error = Some(reason);
            }
        }
        out
    }

    /// `latest`: the last check, or why there is none.
    pub fn latest_text(&self) -> String {
        match &self.checked {
            None => "not checked yet".into(),
            Some((Ok(r), at)) => format!("{} · checked {} UTC", r.tag, hm(*at)),
            Some((Err(reason), _)) => format!("check failed: {reason}"),
        }
    }

    /// `check now`: empty unless a check runs.
    pub fn check_text(&self) -> String {
        match self.job {
            Job::Checking => "checking…".into(),
            _ => String::new(),
        }
    }

    /// `install now`, or `None` while no newer version is known.
    pub fn install_text(&self, own: &Version) -> Option<String> {
        let text = match self.job {
            Job::Downloading(Some(pct)) => format!("downloading {pct}%"),
            Job::Downloading(None) => "downloading…".into(),
            Job::Verifying => "verifying".into(),
            Job::Installing => "installing".into(),
            Job::Idle | Job::Checking => {
                if let Some(v) = &self.installed {
                    format!("v{v} ready · u restart")
                } else if let Some(e) = &self.install_error {
                    format!("failed: {e}")
                } else {
                    self.newer(own)?.tag.clone()
                }
            }
        };
        Some(text)
    }
}

fn hm(t: SystemTime) -> String {
    format::utc_hms(t).chars().take(5).collect()
}

/// A few words for the settings row; the log gets the whole message.
fn short_check_error(e: &CheckError) -> String {
    match e {
        CheckError::RateLimited(_) => "GitHub rate limit".into(),
        CheckError::Failed(m) if m.starts_with("cannot reach GitHub") => "no network".into(),
        CheckError::Failed(m) => match m.strip_prefix("GitHub answered ") {
            Some(status) => status.to_string(),
            None => cut(m, 32),
        },
    }
}

fn short_install_error(e: &str) -> String {
    if e.starts_with("checksum mismatch") {
        "checksum mismatch".into()
    } else if e.starts_with("cannot download") || e.contains(" answered HTTP ") {
        "download failed".into()
    } else if e.starts_with("the download broke off") {
        "download broke off".into()
    } else {
        cut(e, 32)
    }
}

fn cut(text: &str, max: usize) -> String {
    crate::themes::common::fit(text, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn release(tag: &str) -> Release {
        Release {
            tag: tag.into(),
            version: Version::parse(tag).unwrap(),
            assets: Vec::new(),
        }
    }

    fn at_14_20() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(14 * 3600 + 20 * 60)
    }

    #[test]
    fn check_results_and_their_rows() {
        let own = Version::parse("0.3.1").unwrap();
        let mut g = UpdateGroup::default();
        assert_eq!(g.latest_text(), "not checked yet");
        assert_eq!(g.install_text(&own), None, "nothing newer known");
        assert!(g.start_check());
        assert!(!g.start_check(), "one at a time");
        assert_eq!(g.check_text(), "checking…");
        let out = g.apply(Msg::Checked(Ok(release("v0.3.1"))), &own, at_14_20());
        assert_eq!(out.toast.as_deref(), Some("up to date"));
        assert_eq!(g.latest_text(), "v0.3.1 · checked 14:20 UTC");
        assert_eq!(g.check_text(), "");
        assert_eq!(g.install_text(&own), None);

        g.start_check();
        let out = g.apply(Msg::Checked(Ok(release("v0.3.0"))), &own, at_14_20());
        assert_eq!(
            out.toast.as_deref(),
            Some("this build is newer than v0.3.0")
        );
        assert_eq!(g.install_text(&own), None);

        g.start_check();
        let e = CheckError::Failed("cannot reach GitHub: io: connection refused".into());
        let out = g.apply(Msg::Checked(Err(e)), &own, at_14_20());
        assert_eq!(out.toast.as_deref(), Some("check failed: no network"));
        assert_eq!(g.latest_text(), "check failed: no network");
        assert!(out.log[0].starts_with("warning: update: cannot reach GitHub"));
        let limit = CheckError::RateLimited("GitHub refused the request (HTTP 403)".into());
        assert_eq!(short_check_error(&limit), "GitHub rate limit");
        let status = CheckError::Failed("GitHub answered HTTP 502".into());
        assert_eq!(short_check_error(&status), "HTTP 502");
    }

    #[test]
    fn a_newer_release_downloads_verifies_and_waits_for_u() {
        let own = Version::parse("0.3.1").unwrap();
        let mut g = UpdateGroup::default();
        assert_eq!(g.start_install(&own), Err("no newer version known"));
        g.start_check();
        let out = g.apply(Msg::Checked(Ok(release("v0.3.2"))), &own, at_14_20());
        assert_eq!(out.toast.as_deref(), Some("v0.3.2 available"));
        assert_eq!(g.install_text(&own).as_deref(), Some("v0.3.2"));
        assert_eq!(g.start_install(&own).unwrap().tag, "v0.3.2");
        assert_eq!(g.start_install(&own), Err("already running"));
        assert!(!g.start_check(), "no check during an install");
        assert_eq!(g.install_text(&own).as_deref(), Some("downloading…"));
        let step = Step::Downloading {
            done: 42,
            total: Some(100),
        };
        g.apply(Msg::Progress(step), &own, at_14_20());
        assert_eq!(g.install_text(&own).as_deref(), Some("downloading 42%"));
        g.apply(Msg::Progress(Step::Verifying), &own, at_14_20());
        assert_eq!(g.install_text(&own).as_deref(), Some("verifying"));
        let v = Version::parse("0.3.2").unwrap();
        let out = g.apply(Msg::Installed(v.clone()), &own, at_14_20());
        assert_eq!(out.installed, Some(v));
        assert_eq!(out.toast.as_deref(), Some("update ready · u restart"));
        assert_eq!(
            g.install_text(&own).as_deref(),
            Some("v0.3.2 ready · u restart")
        );
        assert_eq!(g.start_install(&own), Err("update ready · u restart"));
    }

    #[test]
    fn a_failed_install_shows_why_and_can_be_tried_again() {
        let own = Version::parse("0.3.1").unwrap();
        let mut g = UpdateGroup::default();
        g.start_check();
        g.apply(Msg::Checked(Ok(release("v0.3.2"))), &own, at_14_20());
        g.start_install(&own).unwrap();
        let e = "checksum mismatch for x: SHA256SUMS says aa, the download is bb; \
                 it was deleted and nothing was changed";
        let out = g.apply(Msg::InstallFailed(e.into()), &own, at_14_20());
        assert_eq!(
            out.toast.as_deref(),
            Some("install failed: checksum mismatch")
        );
        assert_eq!(out.installed, None);
        assert_eq!(
            g.install_text(&own).as_deref(),
            Some("failed: checksum mismatch")
        );
        assert_eq!(g.job, Job::Idle);
        assert!(g.start_install(&own).is_ok(), "the next try");
        assert_eq!(g.install_error, None);
        assert_eq!(
            short_install_error("https://x/y answered HTTP 404"),
            "download failed"
        );
    }

    #[test]
    fn progress_is_a_percentage_of_the_known_size() {
        let job = |done, total| Job::of(Step::Downloading { done, total });
        assert_eq!(job(0, Some(200)), Job::Downloading(Some(0)));
        assert_eq!(job(199, Some(200)), Job::Downloading(Some(99)));
        assert_eq!(job(300, Some(200)), Job::Downloading(Some(100)));
        assert_eq!(job(5, None), Job::Downloading(None));
        assert_eq!(job(5, Some(0)), Job::Downloading(None));
        assert_eq!(Job::of(Step::Installing), Job::Installing);
    }
}
