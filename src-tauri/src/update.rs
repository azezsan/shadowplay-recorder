//! Self-update: checks GitHub Releases in the background and installs a newer
//! version while the game is not running.
//!
//! The endpoint and the public key that verifies an update's signature are
//! `plugins.updater` in `tauri.conf.json`. When to check and whether the
//! phase allows installing are decided by `recorder_core::update`.

use std::thread;

use recorder_core::update::{FIRST_CHECK_DELAY, Outcome, may_install, next_attempt_after};
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::UpdaterExt;

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
        loop {
            let outcome = tauri::async_runtime::block_on(attempt(&app));
            thread::sleep(next_attempt_after(outcome));
        }
    });
}

fn game_allows_install(app: &AppHandle) -> bool {
    may_install(app.state::<AppState>().status.lock().unwrap().phase)
}

/// Checks once and, if a newer release exists and the phase allows it,
/// downloads, verifies and installs it, then restarts the app.
async fn attempt(app: &AppHandle) -> Outcome {
    let check = async { app.updater()?.check().await };
    let update = match check.await {
        Ok(Some(update)) => update,
        Ok(None) => return Outcome::UpToDate,
        Err(e) => {
            log::warn!("could not check for updates: {e}");
            return Outcome::Failed;
        }
    };
    if !game_allows_install(app) {
        log::info!(
            "version {} is available; it will install once Deadlock closes",
            update.version
        );
        return Outcome::Deferred;
    }
    log::info!("downloading version {}", update.version);
    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => bytes,
        Err(e) => {
            log::warn!("could not download version {}: {e}", update.version);
            return Outcome::Failed;
        }
    };
    // The download can take a while; the game may have started meanwhile.
    if !game_allows_install(app) {
        log::info!(
            "version {} is downloaded; it will install once Deadlock closes",
            update.version
        );
        return Outcome::Deferred;
    }
    log::info!("installing version {} and restarting", update.version);
    if let Err(e) = update.install(bytes) {
        log::warn!("could not install version {}: {e}", update.version);
        return Outcome::Failed;
    }
    app.restart()
}
