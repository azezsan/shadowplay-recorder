//! The desktop shell around `recorder-core`: it runs the engine on a
//! background thread, keeps the latest status for the window, and exposes
//! commands and events to the UI. It lives in the tray (`tray`), runs one
//! copy at a time, and can start at login.
//!
//! The UI never calls `invoke` by name. `specta_builder` is the single list of
//! commands and events, and `src/lib/bindings.ts` is generated from it by
//! running a debug build with `--export-bindings`.
//!
//! This crate has no tests: on Windows a test binary lacks the app manifest
//! and cannot load. Anything worth testing belongs in `recorder-core`.

mod activity;
mod commands;
mod tray;
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
use recorder_core::settings::Settings;
use recorder_core::setup::{self, default_recordings_folder};
use recorder_core::test_recording::TestRecordingError;
use recorder_core::tray::{LAUNCHED_AT_LOGIN_FLAG, starts_hidden};
use recorder_core::update::take_relaunch_mark;
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
    /// Asks the engine thread for something between its steps.
    pub engine_requests: Mutex<Sender<EngineRequest>>,
}

/// What the engine thread is asked to do between steps.
pub enum EngineRequest {
    /// Run a test recording and send its result on the enclosed sender.
    TestRecording(Sender<Result<String, TestRecordingError>>),
    /// Stop or save what is in progress, then exit the app.
    Quit,
}

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
            commands::test_recording,
            commands::get_start_with_windows,
            commands::set_start_with_windows
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

/// Steps the engine once a second for the life of the app, and handles
/// requests between steps.
fn run_engine(app: AppHandle, requests: Receiver<EngineRequest>) {
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
        publish(&app, &status);
        // The overlay's debug switch disappeared; the setup checklist in
        // the window shows the fix.
        if engine.take_window_request() {
            tray::show_window(&app);
        }
        for request in requests.try_iter() {
            match request {
                // The engine refuses the test during a match.
                EngineRequest::TestRecording(reply) => {
                    let _ = reply.send(engine.test_recording(|status| publish(&app, status)));
                }
                EngineRequest::Quit => {
                    engine.quit();
                    app.exit(0);
                    return;
                }
            }
        }
        thread::sleep(STEP_INTERVAL);
    }
}

/// Keeps `status` for the window, the tray and the update gate, and tells
/// them if it changed.
fn publish(app: &AppHandle, status: &Status) {
    let changed = {
        let state = app.state::<AppState>();
        let mut current = state.status.lock().unwrap();
        let changed = *current != *status;
        *current = status.clone();
        changed
    };
    if changed {
        tray::show_status(app, status);
        let _ = StatusChanged(status.clone()).emit(app);
    }
}

pub fn run() {
    let activity = activity::install();
    let builder = specta_builder();

    tauri::Builder::default()
        // First, so a second launch hands over before anything else starts.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_window(app);
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .arg(LAUNCHED_AT_LOGIN_FLAG)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(builder.invoke_handler())
        .on_window_event(tray::on_window_event)
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
            let (request_sender, request_receiver) = mpsc::channel();
            let status = Status::idle();
            tray::install(app.handle(), &status)?;
            app.manage(AppState {
                settings: RwLock::new(settings),
                settings_path,
                status: Mutex::new(status),
                engine_requests: Mutex::new(request_sender),
            });
            // The window starts hidden (`visible: false`), so a launch at
            // login or the restart into an update shows nothing but the tray
            // icon, unless an NVIDIA App update removed the overlay's debug
            // switch: then the window opens so the user can fix it before a
            // match.
            let switch_missing = setup::overlay_switch_missing();
            if switch_missing {
                log::warn!(
                    "the overlay's debug switch is missing; an NVIDIA App update may have removed it"
                );
            }
            let after_update = update::relaunch_marker(app.handle())
                .is_some_and(|marker| take_relaunch_mark(&marker, SystemTime::now()));
            if !starts_hidden(std::env::args(), switch_missing, after_update) {
                tray::show_window(app.handle());
            }

            let handle = app.handle().clone();
            thread::spawn(move || run_engine(handle, request_receiver));
            update::start(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start the app");
}
