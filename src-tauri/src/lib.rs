//! The desktop shell around `recorder-core`: it runs the engine on a
//! background thread, keeps the latest status for the window, and exposes
//! commands and events to the UI.
//!
//! The UI never calls `invoke` by name. `specta_builder` is the single list of
//! commands and events, and `src/lib/bindings.ts` is generated from it by
//! running a debug build with `--export-bindings`.
//!
//! This crate has no tests: on Windows a test binary lacks the app manifest
//! and cannot load. Anything worth testing belongs in `recorder-core`.

mod activity;
mod commands;
mod update;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, RwLock};
use std::thread;
use std::time::{Instant, SystemTime};

use recorder_core::engine::{Engine, STEP_INTERVAL, Status, SystemHost};
use recorder_core::game::DEADLOCK;
use recorder_core::overlay::Overlay;
use recorder_core::platform::ProcessWindow;
use recorder_core::session::Phase;
use recorder_core::settings::Settings;
use recorder_core::setup::{self, default_recordings_folder};
use recorder_core::test_recording::TestRecordingError;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_specta::{Builder, Event, collect_commands, collect_events};

/// Makes the app patch the NVIDIA overlay config and exit. See `main`.
pub const PATCH_OVERLAY_FLAG: &str = "--patch-overlay";

/// Emitted whenever the status changes.
#[derive(Debug, Clone, Serialize, specta::Type, Event)]
pub struct StatusChanged(pub Status);

pub struct AppState {
    pub settings: RwLock<Settings>,
    /// `None` when the OS gave no config folder; settings then last only
    /// until the app quits.
    pub settings_path: Option<PathBuf>,
    pub status: Mutex<Status>,
    /// Asks the engine thread for a test recording; it answers on the
    /// enclosed sender once the test is done.
    pub test_requests: Mutex<Sender<TestRequest>>,
}

/// Where the engine thread sends a test recording's result.
pub type TestRequest = Sender<Result<String, TestRecordingError>>;

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .disable_serde_phases()
        .commands(collect_commands![
            commands::get_status,
            commands::get_recent_activity,
            commands::get_setup_report,
            commands::get_settings,
            commands::save_settings,
            commands::pick_recordings_folder,
            commands::get_version,
            commands::repair_overlay,
            commands::test_recording
        ])
        .events(collect_events![StatusChanged, activity::ActivityAdded])
}

/// Makes a debug build write the TypeScript bindings and exit. See `main`.
pub const EXPORT_BINDINGS_FLAG: &str = "--export-bindings";

/// Regenerates `src/lib/bindings.ts`. `pnpm check` runs this and fails if the
/// committed file changed.
#[cfg(debug_assertions)]
pub fn export_bindings() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts");
    specta_builder()
        .export(specta_typescript::Typescript::default(), path)
        .expect("failed to export the TypeScript bindings");
}

/// Steps the engine once a second for the life of the app, and runs test
/// recordings between steps.
fn run_engine(app: AppHandle, test_requests: Receiver<TestRequest>) {
    let overlay = Overlay::default();
    let window = ProcessWindow {
        process: DEADLOCK.process,
    };
    let mut engine = Engine::new(&DEADLOCK, overlay, window, SystemHost);
    let started = Instant::now();
    loop {
        let state = app.state::<AppState>();
        engine.apply_settings(state.settings.read().unwrap().clone());
        let status = engine.step(started.elapsed(), SystemTime::now());
        let changed = {
            let mut current = state.status.lock().unwrap();
            let changed = *current != status;
            *current = status.clone();
            changed
        };
        if changed {
            let _ = StatusChanged(status).emit(&app);
        }
        // The engine refuses the test during a match.
        for reply in test_requests.try_iter() {
            let _ = reply.send(engine.test_recording());
        }
        thread::sleep(STEP_INTERVAL);
    }
}

/// Opens the window if an NVIDIA App update removed the debug switch, even
/// when the app started minimized, so the user can fix it before a match.
fn show_window_if_switch_missing(app: &AppHandle) {
    if !setup::overlay_switch_missing() {
        return;
    }
    log::warn!("the overlay's debug switch is missing; an NVIDIA App update may have removed it");
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn run() {
    let activity = activity::install();
    let builder = specta_builder();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            if let Ok(folder) = app.path().app_log_dir() {
                activity.write_to(&folder);
            }
            activity.emit_to(app.handle().clone());
            log::info!("ShadowPlay Recorder {} started", app.package_info().version);

            let settings_path = app
                .path()
                .app_config_dir()
                .ok()
                .map(|folder| folder.join("settings.json"));
            let settings = settings_path
                .as_deref()
                .map(|path| {
                    Settings::load_or_first_launch(path, || default_recordings_folder(&DEADLOCK))
                })
                .unwrap_or_default();
            let (test_sender, test_receiver) = mpsc::channel();
            app.manage(AppState {
                settings: RwLock::new(settings),
                settings_path,
                status: Mutex::new(Status {
                    phase: Phase::Idle,
                    match_id: None,
                }),
                test_requests: Mutex::new(test_sender),
            });

            show_window_if_switch_missing(app.handle());

            let handle = app.handle().clone();
            thread::spawn(move || run_engine(handle, test_receiver));
            update::start(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start the app");
}
