//! What the app shows while it runs in the background: the tray icon's
//! tooltip, and how a launch at login is told apart from one by the player.

use crate::engine::Status;

/// The argument the login entry launches the app with. Registered by "Start
/// with Windows".
pub const LAUNCHED_AT_LOGIN_FLAG: &str = "--launched-at-login";

/// True if the app should start with no window: it was launched at login,
/// or it is restarting into an update it installed by itself (see
/// `update::take_relaunch_mark`). A missing overlay debug switch wins over
/// both, so the window opens and the user can fix the overlay before a match.
pub fn starts_hidden<I: IntoIterator<Item = S>, S: AsRef<str>>(
    args: I,
    overlay_switch_missing: bool,
    relaunched_after_update: bool,
) -> bool {
    !overlay_switch_missing
        && (relaunched_after_update
            || args
                .into_iter()
                .any(|arg| arg.as_ref() == LAUNCHED_AT_LOGIN_FLAG))
}

/// The tray icon's tooltip for a status. The wording is the status's own
/// label, which the window's status line shows too.
pub fn tooltip(status: &Status) -> String {
    format!("ShadowPlay Recorder: {}", status.label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::DEADLOCK;
    use crate::session::Phase;

    fn status(phase: Phase, match_id: Option<&str>) -> Status {
        Status::new(&DEADLOCK, phase, match_id.map(str::to_string), false)
    }

    #[test]
    fn tooltip_names_the_app_and_the_phase() {
        assert_eq!(
            tooltip(&status(Phase::Idle, None)),
            "ShadowPlay Recorder: Waiting for Deadlock"
        );
        assert_eq!(
            tooltip(&status(Phase::GameRunning, None)),
            "ShadowPlay Recorder: Deadlock is running"
        );
    }

    #[test]
    fn tooltip_names_the_match_being_recorded() {
        assert_eq!(
            tooltip(&status(Phase::Recording, Some("81038409"))),
            "ShadowPlay Recorder: Recording match 81038409"
        );
        assert_eq!(
            tooltip(&status(Phase::Finishing, Some("81038409"))),
            "ShadowPlay Recorder: Saving the recording of match 81038409"
        );
    }

    #[test]
    fn a_launch_at_login_starts_with_no_window() {
        let exe = "shadowplay-recorder.exe";
        assert!(starts_hidden([exe, LAUNCHED_AT_LOGIN_FLAG], false, false));
        assert!(!starts_hidden([exe], false, false));
    }

    #[test]
    fn the_restart_into_an_update_starts_with_no_window() {
        let exe = "shadowplay-recorder.exe";
        assert!(starts_hidden([exe], false, true));
    }

    #[test]
    fn a_missing_overlay_switch_opens_the_window_even_at_login() {
        let exe = "shadowplay-recorder.exe";
        assert!(!starts_hidden([exe, LAUNCHED_AT_LOGIN_FLAG], true, false));
        assert!(!starts_hidden([exe], true, true));
    }
}
