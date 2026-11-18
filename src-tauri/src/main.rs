// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use recorder_core::overlay_setup;
use shadowplay_recorder_lib::PATCH_OVERLAY_FLAG;

fn main() -> ExitCode {
    // The app relaunches itself with administrator rights and this flag to
    // edit the NVIDIA overlay config, which a normal process cannot write.
    if std::env::args().any(|arg| arg == PATCH_OVERLAY_FLAG) {
        let patched = overlay_setup::config_path().map(|path| overlay_setup::patch_file(&path));
        return match patched {
            Some(Ok(_)) => ExitCode::SUCCESS,
            _ => ExitCode::FAILURE,
        };
    }
    shadowplay_recorder_lib::run();
    ExitCode::SUCCESS
}
