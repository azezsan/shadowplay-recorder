//! The only code that calls the operating system directly: finding the game
//! process, moving its window to the foreground, locating Steam, and running
//! a command with administrator rights.
//!
//! `windows` is the real implementation. `other` answers "not available" so
//! the workspace builds and tests on macOS and Linux.

use std::path::{Path, PathBuf};

use crate::session::GameWindow;

#[cfg(windows)]
#[path = "windows.rs"]
mod imp;

#[cfg(not(windows))]
#[path = "other.rs"]
mod imp;

/// True if a process with this executable name exists (case-insensitive).
pub fn is_process_running(name: &str) -> bool {
    imp::is_process_running(name)
}

/// Where Steam is installed.
pub fn steam_root() -> Option<PathBuf> {
    imp::steam_root()
}

/// Why a program could not be run with administrator rights.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElevationError {
    /// The user said no to the Windows consent prompt.
    Declined,
    Failed(String),
}

/// Runs a program with administrator rights, showing the Windows consent
/// prompt, and waits for it. Returns its exit code.
pub fn run_elevated(program: &Path, arguments: &str) -> Result<u32, ElevationError> {
    imp::run_elevated(program, arguments)
}

/// The main window of a process, found by executable name.
pub struct ProcessWindow {
    pub process: &'static str,
}

impl GameWindow for ProcessWindow {
    fn focus(&mut self) -> bool {
        let focused = imp::focus_process(self.process);
        if !focused {
            log::warn!("could not bring {} to the foreground", self.process);
        }
        focused
    }

    fn is_foreground(&self) -> bool {
        imp::is_foreground(self.process)
    }
}
