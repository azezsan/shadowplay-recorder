//! When the app may check for and install an update of itself.
//!
//! The shell does the checking, downloading and installing with Tauri's
//! updater; this module only decides. Installing restarts the app, so it
//! happens only while the phase is idle: an update can never cut off a
//! recording.

use std::time::Duration;

use crate::session::Phase;

/// Wait after the app starts before the first check, so startup stays quick.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);

/// Wait between checks while the app is up to date.
const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

/// Wait before trying again after an update was deferred or failed.
const RETRY_DELAY: Duration = Duration::from_secs(10 * 60);

/// What one update attempt came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// No newer release exists.
    UpToDate,
    /// A newer release exists but the game is running, so it was not
    /// installed.
    Deferred,
    /// The check, download or install failed.
    Failed,
}

/// How long to wait before the next attempt after `outcome`.
pub fn next_attempt_after(outcome: Outcome) -> Duration {
    match outcome {
        Outcome::UpToDate => CHECK_INTERVAL,
        Outcome::Deferred | Outcome::Failed => RETRY_DELAY,
    }
}

/// Whether an update may be installed now, given the phase.
pub fn may_install(phase: Phase) -> bool {
    phase == Phase::Idle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_only_while_the_game_is_not_running() {
        assert!(may_install(Phase::Idle));
        for phase in [
            Phase::GameRunning,
            Phase::InMatch,
            Phase::Starting,
            Phase::Recording,
            Phase::Finishing,
        ] {
            assert!(!may_install(phase), "installed during {phase:?}");
        }
    }

    #[test]
    fn checks_every_few_hours_when_up_to_date() {
        assert_eq!(
            next_attempt_after(Outcome::UpToDate),
            Duration::from_secs(4 * 60 * 60)
        );
    }

    #[test]
    fn retries_a_deferred_or_failed_update_within_minutes() {
        for outcome in [Outcome::Deferred, Outcome::Failed] {
            assert_eq!(next_attempt_after(outcome), Duration::from_secs(10 * 60));
        }
    }
}
