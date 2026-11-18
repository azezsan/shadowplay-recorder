//! The app as a background tool: the tray icon and its menu, closing the
//! window to the tray, and bringing the window back.
//!
//! The tooltip text and the login-launch flag live in `recorder_core::tray`.

use recorder_core::engine::Status;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Window, WindowEvent};
use tauri_plugin_opener::OpenerExt;

use crate::{AppState, EngineRequest};

const TRAY_ID: &str = "main";
const MAIN_WINDOW: &str = "main";

const OPEN: &str = "open";
const OPEN_RECORDINGS: &str = "open-recordings";
const QUIT: &str = "quit";

/// Adds the tray icon with its menu. Clicking the icon opens the window.
pub fn install(app: &AppHandle, status: &Status) -> tauri::Result<()> {
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, OPEN, "Open", true, None::<&str>)?,
            &MenuItem::with_id(
                app,
                OPEN_RECORDINGS,
                "Open recordings folder",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(app, QUIT, "Quit", true, None::<&str>)?,
        ],
    )?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(recorder_core::tray::tooltip(status))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            OPEN => show_window(app),
            OPEN_RECORDINGS => open_recordings_folder(app),
            QUIT => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Makes the tooltip follow the status.
pub fn show_status(app: &AppHandle, status: &Status) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(recorder_core::tray::tooltip(status)));
    }
}

/// Shows, restores and focuses the main window.
pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Closing the window hides it; the app keeps running and recording.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
    }
}

/// Quits through the engine thread, which first stops or saves a recording
/// in progress and then exits the app.
fn quit(app: &AppHandle) {
    log::info!("quit from the tray");
    let sent = app
        .state::<AppState>()
        .engine_requests
        .lock()
        .unwrap()
        .send(EngineRequest::Quit);
    if sent.is_err() {
        // The engine thread is gone, so nothing can be recording.
        app.exit(0);
    }
}

fn open_recordings_folder(app: &AppHandle) {
    let folder = app
        .state::<AppState>()
        .settings
        .read()
        .unwrap()
        .recordings_folder
        .clone();
    let Some(folder) = folder else {
        log::warn!("no recordings folder is set; choose one in Settings");
        show_window(app);
        return;
    };
    if let Err(e) = app.opener().open_path(&folder, None::<&str>) {
        log::warn!("could not open the recordings folder {folder}: {e}");
    }
}
