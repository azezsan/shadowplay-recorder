//! Commands the UI can call. Each one must also be listed in
//! `specta_builder`, which is what generates its TypeScript binding.

use recorder_core::engine::Status;
use recorder_core::game::DEADLOCK;
use recorder_core::setup::{self, SetupReport};
use tauri::{AppHandle, Manager, State};

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
