//! Commands the UI can call. Each one must also be listed in
//! `specta_builder`, which is what generates its TypeScript binding.

use std::path::Path;

use recorder_core::engine::Status;
use recorder_core::game::DEADLOCK;
use recorder_core::settings::Settings;
use recorder_core::setup::{self, SetupReport};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;

use crate::AppState;
use crate::activity::{self, ActivityEntry};

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
    let settings = settings.normalized();
    let path = state
        .settings_path
        .as_deref()
        .ok_or("the app has no folder to keep its settings in")?;
    settings
        .save(path)
        .map_err(|e| format!("could not save the settings: {e}"))?;
    log::info!("settings saved");
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
