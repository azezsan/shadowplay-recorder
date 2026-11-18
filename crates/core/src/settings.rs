//! The few values a person can change, stored as one JSON file.
//!
//! Timings, the NVIDIA command names and the recordable-map list are
//! deliberately not here: they are constants, changed by shipping an update.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Where ShadowPlay saves recordings. Nothing is recorded while unset.
    pub recordings_folder: Option<String>,
    /// Size cap for the recordings folder, in GB.
    pub max_size_gb: f64,
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
            max_size_gb: 200.0,
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
    fn normalizes_out_of_range_values() {
        let settings = Settings {
            recordings_folder: Some("  ".into()),
            max_size_gb: -5.0,
            stop_delay_seconds: 100_000,
            ..Settings::default()
        }
        .normalized();
        assert_eq!(settings.recordings_folder, None);
        assert_eq!(settings.max_size_gb, 1.0);
        assert_eq!(settings.stop_delay_seconds, 600);
    }
}
