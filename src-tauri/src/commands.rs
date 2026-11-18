//! Commands the UI can call. Each one must also be listed in
//! `specta_builder`, which is what generates its TypeScript binding.

use recorder_core::engine::Status;
use tauri::State;

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
