//! When and how the app checks for and installs an update of itself.
//!
//! `attempt` is one try: check, download, and install. The shell supplies
//! the operations, through [`Updater`], with Tauri's updater; this module
//! decides the order and when to hold back. Installing restarts the app, so it
//! happens only while the phase is idle and no test recording runs: an
//! update can never cut off a recording. The background loop and the
//! window's "Check for updates" button both run `attempt`.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::engine::Status;
use crate::session::Phase;

/// Wait after the app starts before the first check, so startup stays quick.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);

/// Wait between checks while the app is up to date.
const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

/// Wait before trying again after an update was deferred or failed.
const RETRY_DELAY: Duration = Duration::from_secs(10 * 60);

/// How long a relaunch mark stays good. Installing takes seconds; an older
/// mark is left over from an install that never restarted the app.
const RELAUNCH_MARK_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// What one update attempt came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Outcome {
    /// No newer release exists.
    UpToDate,
    /// A newer release exists but the game is running, so it was not
    /// installed.
    Deferred { version: String },
    /// The check, download or install failed, or no attempt could be made.
    /// The reason is a sentence fragment for the user.
    Failed { reason: String },
    /// The update was installed; the app must restart into it.
    Installed { version: String },
}

/// What an update attempt needs from the updater.
pub trait Updater {
    /// Checks for a newer release, returning its version if there is one.
    fn check(&mut self) -> Result<Option<String>, String>;
    /// Downloads and verifies the release `check` found.
    fn download(&mut self) -> Result<(), String>;
    /// Installs the downloaded release.
    fn install(&mut self) -> Result<(), String>;
}

/// Checks once and, if a newer release exists and `may_install_now` allows
/// it, downloads and installs it. `may_install_now` is asked again after the
/// download, which can take a while: the game may have started meanwhile.
///
/// The installer restarts the app with the arguments it was started with,
/// which opens the window unless it was a launch at login. Before
/// installing, the attempt writes `relaunch_marker`, which the restarted
/// app reads with [`take_relaunch_mark`] to start in the tray instead.
pub fn attempt(
    updater: &mut impl Updater,
    relaunch_marker: Option<&Path>,
    may_install_now: impl Fn() -> bool,
) -> Outcome {
    let version = match updater.check() {
        Ok(Some(version)) => version,
        Ok(None) => return Outcome::UpToDate,
        Err(e) => {
            log::warn!("could not check for updates: {e}");
            return Outcome::Failed {
                reason: format!("could not check for updates: {e}"),
            };
        }
    };
    if !may_install_now() {
        log::info!("version {version} is available; it will install once Deadlock closes");
        return Outcome::Deferred { version };
    }
    log::info!("downloading version {version}");
    if let Err(e) = updater.download() {
        log::warn!("could not download version {version}: {e}");
        return Outcome::Failed {
            reason: format!("could not download version {version}: {e}"),
        };
    }
    if !may_install_now() {
        log::info!("version {version} is downloaded; it will install once Deadlock closes");
        return Outcome::Deferred { version };
    }
    log::info!("installing version {version} and restarting");
    if let Some(marker) = relaunch_marker
        && let Err(e) = mark_relaunch(marker)
    {
        log::warn!("could not mark the restart to start in the tray: {e}");
    }
    if let Err(e) = updater.install() {
        log::warn!("could not install version {version}: {e}");
        if let Some(marker) = relaunch_marker {
            let _ = fs::remove_file(marker);
        }
        return Outcome::Failed {
            reason: format!("could not install version {version}: {e}"),
        };
    }
    Outcome::Installed { version }
}

/// Marks the next launch as the restart into an update.
pub fn mark_relaunch(marker: &Path) -> io::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    if let Some(folder) = marker.parent() {
        fs::create_dir_all(folder)?;
    }
    fs::write(marker, now.as_secs().to_string())
}

/// True if this launch is the restart into an update, marked by `attempt`
/// within the last few minutes. Removes the mark, so it counts once.
pub fn take_relaunch_mark(marker: &Path, now: SystemTime) -> bool {
    let Ok(text) = fs::read_to_string(marker) else {
        return false;
    };
    let _ = fs::remove_file(marker);
    let Ok(secs) = text.trim().parse::<u64>() else {
        return false;
    };
    let marked = UNIX_EPOCH + Duration::from_secs(secs);
    now.duration_since(marked)
        .is_ok_and(|age| age <= RELAUNCH_MARK_LIFETIME)
}

/// How long to wait before the next attempt after `outcome`.
pub fn next_attempt_after(outcome: &Outcome) -> Duration {
    match outcome {
        Outcome::UpToDate => CHECK_INTERVAL,
        Outcome::Deferred { .. } | Outcome::Failed { .. } | Outcome::Installed { .. } => {
            RETRY_DELAY
        }
    }
}

/// Whether an update may be installed now, given the status.
pub fn may_install(status: &Status) -> bool {
    status.phase == Phase::Idle && !status.test_recording_running
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::SystemTime;

    use super::*;
    use crate::game::DEADLOCK;

    fn status(phase: Phase, test_recording_running: bool) -> Status {
        Status::new(&DEADLOCK, phase, None, test_recording_running)
    }

    #[test]
    fn installs_only_while_the_game_is_not_running() {
        assert!(may_install(&status(Phase::Idle, false)));
        for phase in [
            Phase::GameRunning,
            Phase::InMatch,
            Phase::Starting,
            Phase::Recording,
            Phase::Finishing,
        ] {
            assert!(
                !may_install(&status(phase, false)),
                "installed during {phase:?}"
            );
        }
    }

    #[test]
    fn never_installs_during_a_test_recording() {
        assert!(!may_install(&status(Phase::Idle, true)));
    }

    /// Records which updater operations ran; `fails` names the one that
    /// fails.
    struct FakeUpdater {
        available: Option<&'static str>,
        fails: Option<&'static str>,
        ran: Vec<&'static str>,
        /// The game starts while the update downloads.
        game_starts_during_download: Option<Rc<Cell<bool>>>,
    }

    impl FakeUpdater {
        fn new(available: Option<&'static str>) -> Self {
            Self {
                available,
                fails: None,
                ran: Vec::new(),
                game_starts_during_download: None,
            }
        }

        fn run(&mut self, op: &'static str) -> Result<(), String> {
            self.ran.push(op);
            match self.fails {
                Some(failing) if failing == op => Err(format!("{op} failed (test)")),
                _ => Ok(()),
            }
        }
    }

    impl Updater for FakeUpdater {
        fn check(&mut self) -> Result<Option<String>, String> {
            self.run("check")?;
            Ok(self.available.map(str::to_string))
        }

        fn download(&mut self) -> Result<(), String> {
            if let Some(running) = &self.game_starts_during_download {
                running.set(true);
            }
            self.run("download")
        }

        fn install(&mut self) -> Result<(), String> {
            self.run("install")
        }
    }

    fn installed() -> Outcome {
        Outcome::Installed {
            version: "0.2.0".into(),
        }
    }

    fn deferred() -> Outcome {
        Outcome::Deferred {
            version: "0.2.0".into(),
        }
    }

    #[test]
    fn an_attempt_installs_a_newer_release() {
        let mut updater = FakeUpdater::new(Some("0.2.0"));
        assert_eq!(attempt(&mut updater, None, || true), installed());
        assert_eq!(updater.ran, ["check", "download", "install"]);
    }

    #[test]
    fn the_restart_into_an_installed_update_is_marked_to_start_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("relaunch");
        let mut updater = FakeUpdater::new(Some("0.2.0"));
        assert_eq!(attempt(&mut updater, Some(&marker), || true), installed());
        let now = SystemTime::now();
        assert!(take_relaunch_mark(&marker, now));
        assert!(!take_relaunch_mark(&marker, now), "the mark is used once");
    }

    #[test]
    fn a_failed_install_leaves_no_relaunch_mark() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("relaunch");
        let mut updater = FakeUpdater::new(Some("0.2.0"));
        updater.fails = Some("install");
        assert!(matches!(
            attempt(&mut updater, Some(&marker), || true),
            Outcome::Failed { .. }
        ));
        assert!(!take_relaunch_mark(&marker, SystemTime::now()));
    }

    #[test]
    fn a_stale_relaunch_mark_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("relaunch");
        mark_relaunch(&marker).unwrap();
        let much_later = SystemTime::now() + Duration::from_secs(60 * 60);
        assert!(!take_relaunch_mark(&marker, much_later));
        assert!(!marker.exists(), "a stale mark is removed");
    }

    #[test]
    fn an_attempt_does_nothing_when_up_to_date() {
        let mut updater = FakeUpdater::new(None);
        assert_eq!(attempt(&mut updater, None, || true), Outcome::UpToDate);
        assert_eq!(updater.ran, ["check"]);
    }

    #[test]
    fn an_attempt_waits_for_the_game_to_close_before_downloading() {
        let mut updater = FakeUpdater::new(Some("0.2.0"));
        assert_eq!(attempt(&mut updater, None, || false), deferred());
        assert_eq!(updater.ran, ["check"]);
    }

    #[test]
    fn an_attempt_does_not_install_if_the_game_started_during_the_download() {
        let running = Rc::new(Cell::new(false));
        let mut updater = FakeUpdater::new(Some("0.2.0"));
        updater.game_starts_during_download = Some(running.clone());
        assert_eq!(attempt(&mut updater, None, || !running.get()), deferred());
        assert_eq!(updater.ran, ["check", "download"]);
    }

    #[test]
    fn an_attempt_reports_a_failed_step() {
        for (failing, ran) in [
            ("check", &["check"][..]),
            ("download", &["check", "download"]),
            ("install", &["check", "download", "install"]),
        ] {
            let mut updater = FakeUpdater::new(Some("0.2.0"));
            updater.fails = Some(failing);
            let outcome = attempt(&mut updater, None, || true);
            let Outcome::Failed { reason } = outcome else {
                panic!("{failing}: {outcome:?}");
            };
            assert!(reason.contains(&format!("{failing} failed")), "{reason}");
            assert_eq!(updater.ran, ran);
        }
    }

    #[test]
    fn checks_every_few_hours_when_up_to_date() {
        assert_eq!(
            next_attempt_after(&Outcome::UpToDate),
            Duration::from_secs(4 * 60 * 60)
        );
    }

    #[test]
    fn retries_a_deferred_or_failed_update_within_minutes() {
        for outcome in [
            deferred(),
            Outcome::Failed {
                reason: "offline".into(),
            },
        ] {
            assert_eq!(next_attempt_after(&outcome), Duration::from_secs(10 * 60));
        }
    }
}
