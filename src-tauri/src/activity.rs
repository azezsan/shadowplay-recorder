//! The activity log: what the recorder did, kept three ways. Every entry is
//! appended to a log file, held in memory for the window to show when it
//! opens, and emitted as an event so an open window updates live.
//!
//! It is installed as the `log` crate's logger, so `recorder-core` only ever
//! calls `log::info!` and friends.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use recorder_core::activity;
use serde::Serialize;
use tauri::AppHandle;
use tauri_specta::Event;

/// How many entries are kept in memory. The window keeps as many; it reads
/// this from the bindings.
pub const KEPT_IN_MEMORY: usize = 200;
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    /// The level as written to the log file.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warning => "warning",
            Level::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    /// Milliseconds since the Unix epoch.
    pub time: f64,
    pub level: Level,
    pub message: String,
}

/// Emitted for every new entry.
#[derive(Debug, Clone, Serialize, specta::Type, Event)]
pub struct ActivityAdded(pub ActivityEntry);

#[derive(Default)]
pub struct Activity {
    recent: Mutex<VecDeque<ActivityEntry>>,
    file: Mutex<Option<File>>,
    app: OnceLock<AppHandle>,
}

static ACTIVITY: OnceLock<Activity> = OnceLock::new();

/// Installs the logger. Entries are kept in memory from this point on.
pub fn install() -> &'static Activity {
    let activity = ACTIVITY.get_or_init(Activity::default);
    if log::set_logger(activity).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    activity
}

impl Activity {
    /// Starts writing to `folder/recorder.log`, keeping one previous file.
    pub fn write_to(&self, folder: &Path) {
        let path = folder.join("recorder.log");
        if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
            let _ = fs::rename(&path, folder.join("recorder.old.log"));
        }
        let file = fs::create_dir_all(folder)
            .and_then(|()| OpenOptions::new().create(true).append(true).open(&path));
        match file {
            Ok(file) => *self.file.lock().unwrap() = Some(file),
            Err(e) => log::warn!("could not open the log file {path:?}: {e}"),
        }
    }

    /// Starts emitting an event per entry.
    pub fn emit_to(&self, app: AppHandle) {
        let _ = self.app.set(app);
    }

    pub fn recent(&self) -> Vec<ActivityEntry> {
        self.recent.lock().unwrap().iter().cloned().collect()
    }
}

impl log::Log for Activity {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // Only this app's own messages, not its dependencies'.
        metadata.level() <= log::Level::Info
            && ["recorder_core", "shadowplay_recorder_lib"]
                .iter()
                .any(|prefix| metadata.target().starts_with(prefix))
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let now = SystemTime::now();
        let entry = ActivityEntry {
            time: now
                .duration_since(UNIX_EPOCH)
                .map_or(0.0, |d| d.as_millis() as f64),
            level: match record.level() {
                log::Level::Error => Level::Error,
                log::Level::Warn => Level::Warning,
                _ => Level::Info,
            },
            message: record.args().to_string(),
        };

        if let Some(file) = self.file.lock().unwrap().as_mut() {
            let line = activity::log_line(now, entry.level.as_str(), &entry.message);
            let _ = writeln!(file, "{line}");
        }
        {
            let mut recent = self.recent.lock().unwrap();
            if recent.len() == KEPT_IN_MEMORY {
                recent.pop_front();
            }
            recent.push_back(entry.clone());
        }
        if let Some(app) = self.app.get() {
            let _ = ActivityAdded(entry).emit(app);
        }
    }

    fn flush(&self) {}
}
