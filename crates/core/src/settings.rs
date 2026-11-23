//! The few values a person can change, stored as one JSON file.
//!
//! Timings, the NVIDIA command names and the recordable-map list are
//! deliberately not here: they are constants, changed by shipping an update.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::game::GameProfile;
use crate::retention::{GIB, gib_to_bytes};

/// How the app decides when to delete the oldest recordings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum RetentionRule {
    /// Keep the recordings folder under `max_size_gb`.
    #[default]
    SizeCap,
    /// Let recordings use the whole disk except the free-space reserve.
    FillDisk,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Where ShadowPlay saves recordings. Nothing is recorded while unset.
    pub recordings_folder: Option<String>,
    pub retention_rule: RetentionRule,
    /// Size cap for the recordings folder, in GB. Kept while fill the disk
    /// is in use, for switching back.
    pub max_size_gb: f64,
    /// Under fill the disk, use `reserve_gb` instead of the automatic
    /// free-space reserve.
    pub manual_reserve: bool,
    /// The manual free-space reserve, in GB. Kept while unused.
    pub reserve_gb: f64,
    /// How long to keep recording after a match ends.
    pub stop_delay_seconds: u32,
    pub focus_on_match_start: bool,
    /// Console log location, set only when it could not be found through
    /// Steam.
    pub console_log: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            recordings_folder: None,
            retention_rule: RetentionRule::SizeCap,
            max_size_gb: 200.0,
            manual_reserve: false,
            reserve_gb: 50.0,
            stop_delay_seconds: 10,
            focus_on_match_start: true,
            console_log: None,
        }
    }
}

impl Settings {
    /// Clamps values to what the app can honour and drops blank paths.
    pub fn normalized(mut self) -> Self {
        if !self.max_size_gb.is_finite() || self.max_size_gb < 1.0 {
            self.max_size_gb = 1.0;
        }
        if !self.reserve_gb.is_finite() || self.reserve_gb < 1.0 {
            self.reserve_gb = 1.0;
        }
        self.stop_delay_seconds = self.stop_delay_seconds.min(600);
        let blank = |p: &Option<String>| p.as_deref().is_some_and(|p| p.trim().is_empty());
        if blank(&self.recordings_folder) {
            self.recordings_folder = None;
        }
        if blank(&self.console_log) {
            self.console_log = None;
        }
        self
    }

    /// ShadowPlay saves each game's recordings in a subfolder named after
    /// the game, so a recordings folder that holds one (such as
    /// `Videos\NVIDIA` instead of `Videos\NVIDIA\Deadlock`) is moved down
    /// into it. The app only looks at the folder itself, so the parent would
    /// never show a recording. Returns whether the folder changed.
    pub fn use_game_subfolder(&mut self, game: &GameProfile) -> bool {
        let Some(folder) = &self.recordings_folder else {
            return false;
        };
        let subfolder = Path::new(folder).join(game.name);
        if !subfolder.is_dir() {
            return false;
        }
        let subfolder = subfolder.to_string_lossy().into_owned();
        log::info!(
            "recordings folder {folder} holds a {} folder; using {subfolder}",
            game.name
        );
        self.recordings_folder = Some(subfolder);
        true
    }

    /// Reads the file, falling back to defaults when it is missing or broken.
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str::<Settings>(&text)
                .inspect_err(|e| log::warn!("settings file is invalid, using defaults: {e}"))
                .unwrap_or_default()
                .normalized(),
            Err(_) => Self::default(),
        }
    }

    /// Like `load`, but on first launch (no file yet) starts from defaults
    /// with the suggested recordings folder, and saves that.
    pub fn load_or_first_launch(
        path: &Path,
        suggested_folder: impl FnOnce() -> Option<PathBuf>,
    ) -> Self {
        if path.exists() {
            return Self::load(path);
        }
        let settings = Self {
            recordings_folder: suggested_folder().map(|f| f.to_string_lossy().into_owned()),
            ..Self::default()
        };
        if let Some(folder) = &settings.recordings_folder {
            log::info!("first launch: using ShadowPlay's usual folder {folder}");
        }
        if let Err(e) = settings.save(path) {
            log::warn!("could not save the settings file: {e}");
        }
        settings
    }

    /// The manual free-space reserve in use, if any. Never more than half
    /// of a disk of `disk_total` bytes, even from a hand-edited file.
    pub fn manual_reserve_bytes(&self, disk_total: u64) -> Option<u64> {
        (self.retention_rule == RetentionRule::FillDisk && self.manual_reserve)
            .then(|| gib_to_bytes(self.reserve_gb).min(disk_total / 2))
    }

    /// Saves settings edited in the window: clamps them, writes them to
    /// `path` (`None` when the OS gave the app no config folder) and returns
    /// what was saved. `disk_total` gives the size of the disk holding a
    /// folder, so a manual reserve over half the disk is refused. The error
    /// is a sentence for the user.
    pub fn save_from_window(
        self,
        path: Option<&Path>,
        disk_total: impl FnOnce(&Path) -> Option<u64>,
    ) -> Result<Self, String> {
        let settings = self.normalized();
        if settings.retention_rule == RetentionRule::FillDisk
            && settings.manual_reserve
            && let Some(total) = settings
                .recordings_folder
                .as_deref()
                .and_then(|folder| disk_total(Path::new(folder)))
            && gib_to_bytes(settings.reserve_gb) > total / 2
        {
            return Err(format!(
                "The reserve can be at most half the disk, {} GB.",
                total / 2 / GIB
            ));
        }
        let path = path.ok_or("the app has no folder to keep its settings in")?;
        settings
            .save(path)
            .map_err(|e| format!("could not save the settings: {e}"))?;
        log::info!("settings saved");
        Ok(settings)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::DEADLOCK;

    #[test]
    fn round_trips_and_tolerates_missing_or_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());

        let settings = Settings {
            recordings_folder: Some(r"C:\Videos".into()),
            max_size_gb: 50.0,
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);

        fs::write(&path, r#"{"maxSizeGb": 20}"#).unwrap();
        assert_eq!(Settings::load(&path).max_size_gb, 20.0);
        assert_eq!(Settings::load(&path).stop_delay_seconds, 10);

        fs::write(&path, "not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn first_launch_fills_in_the_suggested_folder_and_saves_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let suggested = dir.path().join("Videos").join("NVIDIA").join("Deadlock");

        let settings = Settings::load_or_first_launch(&path, || Some(suggested.clone()));
        let expected = Some(suggested.to_string_lossy().into_owned());
        assert_eq!(settings.recordings_folder, expected);
        assert_eq!(Settings::load(&path).recordings_folder, expected);
    }

    #[test]
    fn a_later_launch_keeps_the_saved_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        Settings::default().save(&path).unwrap();

        let settings = Settings::load_or_first_launch(&path, || Some(dir.path().to_path_buf()));
        assert_eq!(settings.recordings_folder, None);
    }

    #[test]
    fn saving_from_the_window_stores_what_the_app_will_use() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let edited = Settings {
            console_log: Some(" ".into()),
            max_size_gb: 0.0,
            ..Settings::default()
        };
        let saved = edited.save_from_window(Some(&path), |_| None).unwrap();
        assert_eq!(saved.console_log, None);
        assert_eq!(saved.max_size_gb, 1.0);
        assert_eq!(Settings::load(&path), saved);
    }

    #[test]
    fn a_manual_reserve_over_half_the_disk_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            recordings_folder: Some(r"D:\Videos".into()),
            retention_rule: RetentionRule::FillDisk,
            manual_reserve: true,
            reserve_gb: 501.0,
            ..Settings::default()
        };
        let disk = |_: &Path| Some(1000 * GIB);
        assert_eq!(
            settings.clone().save_from_window(Some(&path), disk),
            Err("The reserve can be at most half the disk, 500 GB.".to_string())
        );
        assert!(!path.exists());

        let half = Settings {
            reserve_gb: 500.0,
            ..settings.clone()
        };
        assert!(half.save_from_window(Some(&path), disk).is_ok());

        // Unused, the value is only remembered and not checked.
        let automatic = Settings {
            manual_reserve: false,
            ..settings
        };
        assert!(automatic.save_from_window(Some(&path), disk).is_ok());
    }

    #[test]
    fn a_hand_edited_manual_reserve_is_held_to_half_the_disk() {
        let settings = Settings {
            retention_rule: RetentionRule::FillDisk,
            manual_reserve: true,
            reserve_gb: 900.0,
            ..Settings::default()
        };
        assert_eq!(settings.manual_reserve_bytes(1000 * GIB), Some(500 * GIB));
        let size_cap = Settings {
            retention_rule: RetentionRule::SizeCap,
            ..settings
        };
        assert_eq!(size_cap.manual_reserve_bytes(1000 * GIB), None);
    }

    #[test]
    fn saving_without_a_settings_folder_is_refused() {
        assert_eq!(
            Settings::default().save_from_window(None, |_| None),
            Err("the app has no folder to keep its settings in".to_string())
        );
    }

    #[test]
    fn a_folder_holding_the_game_subfolder_moves_down_into_it() {
        let dir = tempfile::tempdir().unwrap();
        let nvidia = dir.path().join("NVIDIA");
        let deadlock = nvidia.join("Deadlock");
        fs::create_dir_all(&deadlock).unwrap();
        let folder = |path: &Path| Some(path.to_string_lossy().into_owned());

        let mut settings = Settings {
            recordings_folder: folder(&nvidia),
            ..Settings::default()
        };
        assert!(settings.use_game_subfolder(&DEADLOCK));
        assert_eq!(settings.recordings_folder, folder(&deadlock));

        // Already the game's folder, or no folder: nothing changes.
        assert!(!settings.use_game_subfolder(&DEADLOCK));
        assert_eq!(settings.recordings_folder, folder(&deadlock));
        assert!(!Settings::default().use_game_subfolder(&DEADLOCK));
    }

    #[test]
    fn normalizes_out_of_range_values() {
        let settings = Settings {
            recordings_folder: Some("  ".into()),
            max_size_gb: -5.0,
            reserve_gb: f64::NAN,
            stop_delay_seconds: 100_000,
            ..Settings::default()
        }
        .normalized();
        assert_eq!(settings.recordings_folder, None);
        assert_eq!(settings.max_size_gb, 1.0);
        assert_eq!(settings.reserve_gb, 1.0);
        assert_eq!(settings.stop_delay_seconds, 600);
    }
}
