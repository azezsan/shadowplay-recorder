use std::path::{Path, PathBuf};

use super::{DiskSpace, ElevationError};

pub fn is_process_running(_name: &str) -> bool {
    false
}

pub fn steam_root() -> Option<PathBuf> {
    None
}

pub fn run_elevated(_program: &Path, _arguments: &str) -> Result<u32, ElevationError> {
    Err(ElevationError::Failed(
        "administrator prompts are only available on Windows".into(),
    ))
}

pub fn focus_process(_name: &str) -> bool {
    false
}

pub fn is_foreground(_name: &str) -> bool {
    false
}

pub fn disk_space(_path: &Path) -> Option<DiskSpace> {
    None
}
