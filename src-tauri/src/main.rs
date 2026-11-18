// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use recorder_core::overlay_setup;

fn main() -> ExitCode {
    // The app relaunches itself with administrator rights and this flag to
    // edit the NVIDIA overlay config, which a normal process cannot write.
    if std::env::args().any(|arg| arg == overlay_setup::PATCH_OVERLAY_FLAG) {
        let config = overlay_setup::config_path();
        return ExitCode::from(overlay_setup::patch_and_exit_code(config.as_deref()));
    }
    #[cfg(debug_assertions)]
    if std::env::args().any(|arg| arg == shadowplay_recorder_lib::EXPORT_BINDINGS_FLAG) {
        shadowplay_recorder_lib::export_bindings();
        return ExitCode::SUCCESS;
    }
    shadowplay_recorder_lib::run();
    ExitCode::SUCCESS
}
