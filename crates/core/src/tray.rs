//! What the app shows while it runs in the background: the tray icon's
//! tooltip, and how a launch at login is told apart from one by the player.

use crate::engine::Status;
use crate::game::GameProfile;
use crate::session::Phase;

/// The argument the login entry launches the app with. Registered by "Start
/// with Windows".
pub const LAUNCHED_AT_LOGIN_FLAG: &str = "--launched-at-login";

/// True if the app should start with no window: it was launched at login.
pub fn starts_hidden<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> bool {
    args.into_iter()
        .any(|arg| arg.as_ref() == LAUNCHED_AT_LOGIN_FLAG)
}

/// The tray icon's tooltip for a status. The wording follows the window's
/// status line.
pub fn tooltip(game: &GameProfile, status: &Status) -> String {
    let name = game.name;
    let of_match = |id: &str| format!(" of match {id}");
    let phase = match (status.phase, status.match_id.as_deref()) {
        (Phase::Idle, _) => format!("waiting for {name}"),
        (Phase::GameRunning, _) => format!("{name} is running"),
        (Phase::InMatch, _) => "in a match, not recording".to_string(),
        (Phase::Starting, id) => {
            format!(
                "starting the recording{}",
                id.map(of_match).unwrap_or_default()
            )
        }
        (Phase::Recording, Some(id)) => format!("recording match {id}"),
        (Phase::Recording, None) => "recording".to_string(),
        (Phase::Finishing, id) => {
            format!(
                "saving the recording{}",
                id.map(of_match).unwrap_or_default()
            )
        }
    };
    format!("ShadowPlay Recorder: {phase}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::DEADLOCK;

    fn status(phase: Phase, match_id: Option<&str>) -> Status {
        Status {
            phase,
            match_id: match_id.map(str::to_string),
        }
    }

    #[test]
    fn tooltip_names_the_app_and_the_phase() {
        assert_eq!(
            tooltip(&DEADLOCK, &status(Phase::Idle, None)),
            "ShadowPlay Recorder: waiting for Deadlock"
        );
        assert_eq!(
            tooltip(&DEADLOCK, &status(Phase::GameRunning, None)),
            "ShadowPlay Recorder: Deadlock is running"
        );
    }

    #[test]
    fn tooltip_names_the_match_being_recorded() {
        assert_eq!(
            tooltip(&DEADLOCK, &status(Phase::Recording, Some("81038409"))),
            "ShadowPlay Recorder: recording match 81038409"
        );
        assert_eq!(
            tooltip(&DEADLOCK, &status(Phase::Finishing, Some("81038409"))),
            "ShadowPlay Recorder: saving the recording of match 81038409"
        );
    }

    #[test]
    fn a_launch_at_login_starts_with_no_window() {
        let exe = "shadowplay-recorder.exe";
        assert!(starts_hidden([exe, LAUNCHED_AT_LOGIN_FLAG]));
        assert!(!starts_hidden([exe]));
    }
}
