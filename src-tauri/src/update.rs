//! Self-update: checks GitHub Releases in the background and installs a newer
//! version while the game is not running.
//!
//! The endpoint and the public key that verifies an update's signature are
//! `plugins.updater` in `tauri.conf.json`. The attempt itself (the order of
//! the steps, when to hold back, marking the restart to start in the tray)
//! is `recorder_core::update::attempt`; this module supplies Tauri's updater.

use std::path::PathBuf;
use std::thread;

use recorder_core::update::{self, FIRST_CHECK_DELAY, Outcome, Updater, next_attempt_after};
use tauri::async_runtime::block_on;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::AppState;

/// Starts the update loop on its own thread. Development builds never
/// update: they would replace themselves with the latest release.
pub fn start(app: AppHandle) {
    if cfg!(debug_assertions) {
        log::info!("development build: not checking for updates");
        return;
    }
    thread::spawn(move || {
        thread::sleep(FIRST_CHECK_DELAY);
        let marker = relaunch_marker(&app);
        loop {
            let mut updater = TauriUpdater {
                app: &app,
                update: None,
                bytes: None,
            };
            let outcome = update::attempt(&mut updater, marker.as_deref(), || {
                update::may_install(&app.state::<AppState>().status.lock().unwrap())
            });
            if outcome == Outcome::Installed {
                // Only reached where installing does not end the process
                // itself; on Windows the installer restarts the app.
                app.restart();
            }
            thread::sleep(next_attempt_after(outcome));
        }
    });
}

/// Where the restart into an update is marked, so it starts in the tray.
pub fn relaunch_marker(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_local_data_dir()
        .ok()
        .map(|folder| folder.join("relaunched-after-update"))
}

struct TauriUpdater<'a> {
    app: &'a AppHandle,
    update: Option<Update>,
    bytes: Option<Vec<u8>>,
}

impl Updater for TauriUpdater<'_> {
    fn check(&mut self) -> Result<Option<String>, String> {
        let update =
            block_on(async { self.app.updater()?.check().await }).map_err(|e| e.to_string())?;
        let version = update.as_ref().map(|u| u.version.clone());
        self.update = update;
        Ok(version)
    }

    fn download(&mut self) -> Result<(), String> {
        let update = self.update.as_ref().ok_or("no update was found")?;
        let bytes = block_on(update.download(|_, _| {}, || {})).map_err(|e| e.to_string())?;
        self.bytes = Some(bytes);
        Ok(())
    }

    fn install(&mut self) -> Result<(), String> {
        let update = self.update.as_ref().ok_or("no update was found")?;
        let bytes = self.bytes.take().ok_or("the update was not downloaded")?;
        update.install(bytes).map_err(|e| e.to_string())
    }
}
