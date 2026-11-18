//! The setup checklist: the three things that must be true before a match
//! can be recorded, each with a state the UI can explain and fix.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::game::GameProfile;
use crate::overlay::{Overlay, Probe};
use crate::settings::Settings;
use crate::{overlay_setup, platform, steam};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum OverlayState {
    Ready,
    /// The NVIDIA App's overlay config file was not found.
    NvidiaAppMissing,
    /// The debug-port switch is not in the overlay config. Fixable by the
    /// app, with an administrator prompt.
    SwitchMissing,
    /// The switch is there but nothing listens: the NVIDIA App needs a full
    /// restart, or is not running.
    NotReachable,
    /// The port answers but the overlay page is not loaded yet.
    NoRecordPage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ConsoleLogState {
    Ready,
    /// The game was not found in any Steam library.
    GameNotFound,
    /// The game's launch options lack the console-log option.
    OptionMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum FolderState {
    Ready,
    NotSet,
    /// A folder is set but does not exist.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SetupReport {
    pub overlay: OverlayState,
    pub console_log: ConsoleLogState,
    pub recordings_folder: FolderState,
}

impl SetupReport {
    pub fn is_ready(&self) -> bool {
        self.overlay == OverlayState::Ready
            && self.console_log == ConsoleLogState::Ready
            && self.recordings_folder == FolderState::Ready
    }
}

pub fn check(settings: &Settings, game: &GameProfile) -> SetupReport {
    SetupReport {
        overlay: overlay_state(),
        console_log: console_log_state(settings, game, platform::steam_root().as_deref()),
        recordings_folder: folder_state(settings),
    }
}

/// The overlay state that needs no network probe: is the switch in the file?
pub fn overlay_switch_missing() -> bool {
    overlay_setup::config_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|text| !overlay_setup::has_debug_switch(&text))
}

fn overlay_state() -> OverlayState {
    let probe = Overlay::default().probe();
    if !matches!(probe, Probe::Unreachable(_)) {
        return overlay_state_from(probe, None);
    }
    let config = overlay_setup::config_path().map(|path| {
        // An unreadable file cannot be shown to carry the switch.
        std::fs::read_to_string(path).unwrap_or_default()
    });
    overlay_state_from(probe, config.as_deref())
}

/// The overlay state from a probe of the debug port and the text of the
/// overlay config, `None` when the NVIDIA App is not installed.
fn overlay_state_from(probe: Probe, config: Option<&str>) -> OverlayState {
    // A reachable port is proof enough, whatever the file says.
    match probe {
        Probe::Ready => OverlayState::Ready,
        Probe::NoRecordPage => OverlayState::NoRecordPage,
        Probe::Unreachable(_) => match config {
            None => OverlayState::NvidiaAppMissing,
            Some(text) if !overlay_setup::has_debug_switch(text) => OverlayState::SwitchMissing,
            Some(_) => OverlayState::NotReachable,
        },
    }
}

/// The console log to follow: the manual override, else the Steam install.
pub fn console_log_path(
    settings: &Settings,
    game: &GameProfile,
    steam_root: Option<&Path>,
) -> Option<PathBuf> {
    settings
        .console_log
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| steam::console_log_path(steam_root?, game))
}

fn console_log_state(
    settings: &Settings,
    game: &GameProfile,
    steam_root: Option<&Path>,
) -> ConsoleLogState {
    let Some(log) = console_log_path(settings, game, steam_root) else {
        return ConsoleLogState::GameNotFound;
    };
    let has_option = steam_root
        .and_then(|root| steam::has_console_log_option(root, game))
        // Steam's config was unreadable: an existing log is the next best sign.
        .unwrap_or_else(|| log.is_file());
    if has_option {
        ConsoleLogState::Ready
    } else {
        ConsoleLogState::OptionMissing
    }
}

fn folder_state(settings: &Settings) -> FolderState {
    match &settings.recordings_folder {
        None => FolderState::NotSet,
        Some(folder) if Path::new(folder).is_dir() => FolderState::Ready,
        Some(_) => FolderState::Missing,
    }
}

/// ShadowPlay's usual folder for the game, if it exists.
pub fn default_recordings_folder(game: &GameProfile) -> Option<PathBuf> {
    let videos = Path::new(&std::env::var_os("USERPROFILE")?).join("Videos");
    [
        videos.join("NVIDIA").join(game.name),
        videos.join(game.name),
    ]
    .into_iter()
    .find(|folder| folder.is_dir())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::game::DEADLOCK;

    const UNPATCHED: &str = r#"{"switches":["nv-osc=true"]}"#;
    const PATCHED: &str = r#"{"switches":["nv-remote-debugging-port=49222"]}"#;

    fn unreachable() -> Probe {
        Probe::Unreachable("connection refused".into())
    }

    #[test]
    fn overlay_state_trusts_a_reachable_port_over_the_file() {
        assert_eq!(
            overlay_state_from(Probe::Ready, Some(UNPATCHED)),
            OverlayState::Ready
        );
        assert_eq!(
            overlay_state_from(Probe::NoRecordPage, None),
            OverlayState::NoRecordPage
        );
    }

    #[test]
    fn overlay_state_explains_an_unreachable_port_from_the_file() {
        assert_eq!(
            overlay_state_from(unreachable(), None),
            OverlayState::NvidiaAppMissing
        );
        assert_eq!(
            overlay_state_from(unreachable(), Some(UNPATCHED)),
            OverlayState::SwitchMissing
        );
        assert_eq!(
            overlay_state_from(unreachable(), Some(PATCHED)),
            OverlayState::NotReachable,
            "the switch is in place, so the NVIDIA App needs a restart"
        );
    }

    #[test]
    fn folder_state_follows_the_setting() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        assert_eq!(folder_state(&settings), FolderState::NotSet);
        settings.recordings_folder = Some(dir.path().join("gone").to_string_lossy().into());
        assert_eq!(folder_state(&settings), FolderState::Missing);
        settings.recordings_folder = Some(dir.path().to_string_lossy().into());
        assert_eq!(folder_state(&settings), FolderState::Ready);
    }

    #[test]
    fn console_log_state_without_steam_uses_the_override() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        assert_eq!(
            console_log_state(&settings, &DEADLOCK, None),
            ConsoleLogState::GameNotFound
        );

        let log = dir.path().join("console.log");
        settings.console_log = Some(log.to_string_lossy().into());
        assert_eq!(
            console_log_state(&settings, &DEADLOCK, None),
            ConsoleLogState::OptionMissing
        );
        fs::write(&log, "").unwrap();
        assert_eq!(
            console_log_state(&settings, &DEADLOCK, None),
            ConsoleLogState::Ready
        );
    }
}
