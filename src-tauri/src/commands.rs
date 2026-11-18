//! Commands the UI can call. Each one must also be listed in
//! `specta_builder`, which is what generates its TypeScript binding.

use std::path::Path;
use std::sync::mpsc;

use recorder_core::engine::Status;
use recorder_core::game::DEADLOCK;
use recorder_core::overlay_setup;
use recorder_core::settings::Settings;
use recorder_core::setup::{self, SetupReport};
use recorder_core::test_recording::TestRecordingError;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;

use crate::activity::{self, ActivityEntry};
use crate::{AppState, EngineRequest};

/// The current status, for a window that has just opened. Later changes
/// arrive as `StatusChanged` events.
#[tauri::command]
#[specta::specta]
pub fn get_status(state: State<'_, AppState>) -> Status {
    state.status.lock().unwrap().clone()
}

/// The activity entries kept in memory, oldest first. Later entries arrive
/// as `ActivityAdded` events.
#[tauri::command]
#[specta::specta]
pub fn get_recent_activity() -> Vec<ActivityEntry> {
    activity::install().recent()
}

/// The setup checklist, checked now. It probes the debug port and reads
/// Steam's files, so it runs off the main thread; the window calls it on a
/// timer while it is visible.
#[tauri::command]
#[specta::specta]
pub async fn get_setup_report(app: AppHandle) -> SetupReport {
    let settings = app.state::<AppState>().settings.read().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || setup::check(&settings, &DEADLOCK))
        .await
        .expect("the setup check panicked")
}

/// The settings in use.
#[tauri::command]
#[specta::specta]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.read().unwrap().clone()
}

/// Clamps the settings, writes them to the settings file and hands them to
/// the engine before its next step. Returns what was saved.
#[tauri::command]
#[specta::specta]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<Settings, String> {
    let settings = settings.save_from_window(state.settings_path.as_deref())?;
    *state.settings.write().unwrap() = settings.clone();
    Ok(settings)
}

/// Asks for a recordings folder with the native folder picker, starting at
/// the current one. `None` when the picker was cancelled.
#[tauri::command]
#[specta::specta]
pub async fn pick_recordings_folder(app: AppHandle, current: Option<String>) -> Option<String> {
    let mut picker = app
        .dialog()
        .file()
        .set_title("Choose the recordings folder");
    if let Some(folder) = current.filter(|f| Path::new(f).is_dir()) {
        picker = picker.set_directory(folder);
    }
    // Blocking is fine here: async commands run off the main thread.
    let folder = picker.blocking_pick_folder()?.into_path().ok()?;
    Some(folder.to_string_lossy().into_owned())
}

/// Asks for the console log file with the native file picker, starting next
/// to the current one. `None` when the picker was cancelled.
#[tauri::command]
#[specta::specta]
pub async fn pick_console_log(app: AppHandle, current: Option<String>) -> Option<String> {
    let mut picker = app
        .dialog()
        .file()
        .set_title("Choose Deadlock's console log")
        .add_filter("Console log", &["log", "txt"]);
    if let Some(folder) = current.as_deref().and_then(|f| Path::new(f).parent())
        && folder.is_dir()
    {
        picker = picker.set_directory(folder);
    }
    // Blocking is fine here: async commands run off the main thread.
    let file = picker.blocking_pick_file()?.into_path().ok()?;
    Some(file.to_string_lossy().into_owned())
}

/// The version of the running app, shown in the window.
#[tauri::command]
#[specta::specta]
pub fn get_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Adds the debug switch to the NVIDIA overlay config: the app relaunches
/// itself with administrator rights and `--patch-overlay`, behind one
/// Windows prompt. The error is a sentence for the user.
#[tauri::command]
#[specta::specta]
pub async fn repair_overlay() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let exe = std::env::current_exe().map_err(|e| format!("could not find the app: {e}"))?;
        overlay_setup::repair_installed(&exe).map_err(|e| e.to_string())
    })
    .await
    .expect("the overlay repair panicked")
}

/// Records a few seconds to prove the overlay, the record command and the
/// recordings folder work together, and returns the recorded file's path.
/// The engine thread runs it between steps and refuses it during a match.
#[tauri::command]
#[specta::specta]
pub async fn test_recording(app: AppHandle) -> Result<String, TestRecordingError> {
    let (reply, result) = mpsc::channel();
    app.state::<AppState>()
        .engine_requests
        .lock()
        .unwrap()
        .send(EngineRequest::TestRecording(reply))
        .expect("the engine thread has stopped");
    tauri::async_runtime::spawn_blocking(move || result.recv())
        .await
        .expect("the test recording panicked")
        .expect("the engine thread has stopped")
}

/// Whether the app launches at login. The OS holds this, not the settings
/// file, so the answer is always the real state.
#[tauri::command]
#[specta::specta]
pub fn get_start_with_windows(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| format!("could not read the startup entry: {e}"))
}

/// Registers or removes the login launch, which starts the app in the tray
/// with no window. Returns the state the OS reports afterwards.
#[tauri::command]
#[specta::specta]
pub fn set_start_with_windows(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let autolaunch = app.autolaunch();
    let changed = if enabled {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };
    changed.map_err(|e| format!("could not change the startup entry: {e}"))?;
    log::info!(
        "start with Windows {}",
        if enabled { "turned on" } else { "turned off" }
    );
    get_start_with_windows(app)
}
