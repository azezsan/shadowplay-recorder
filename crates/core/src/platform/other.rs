use std::path::{Path, PathBuf};

pub fn is_process_running(_name: &str) -> bool {
    false
}

pub fn steam_root() -> Option<PathBuf> {
    None
}

pub fn run_elevated(_program: &Path, _arguments: &str) -> Result<u32, String> {
    Err("administrator prompts are only available on Windows".into())
}

pub fn focus_process(_name: &str) -> bool {
    false
}

pub fn is_foreground(_name: &str) -> bool {
    false
}
