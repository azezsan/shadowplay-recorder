//! Opens the NVIDIA overlay's debug port by editing `NVIDIA Overlay.json`.
//!
//! The overlay reads its Chromium switches from that file at launch. Adding
//! `nv-remote-debugging-port=<port>` makes it listen on loopback, which is how
//! `overlay` reaches it. The file sits under Program Files, so writing it
//! needs administrator rights, and an NVIDIA App update can reset it.
//!
//! `repair` is the Fix button: it relaunches the app with administrator
//! rights and `--patch-overlay`, which runs `patch_and_exit_code` and exits.
//!
//! The port is unauthenticated: any program on the same machine can use it to
//! run code in the overlay page. It is not reachable from other machines.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::overlay::DEBUG_PORT;
use crate::platform::ElevationError;

const SWITCH_NAME: &str = "nv-remote-debugging-port";

/// Where the NVIDIA App keeps the overlay's switches, if it is installed.
pub fn config_path() -> Option<PathBuf> {
    let program_files = std::env::var_os("ProgramFiles")?;
    let path = Path::new(&program_files)
        .join("NVIDIA Corporation")
        .join("NVIDIA App")
        .join("CEF")
        .join("Resources")
        .join("NVIDIA Overlay.json");
    path.is_file().then_some(path)
}

fn our_switch() -> String {
    format!("{SWITCH_NAME}={DEBUG_PORT}")
}

/// True if the file's text already carries our debug-port switch.
pub fn has_debug_switch(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|config| {
        config["switches"]
            .as_array()
            .is_some_and(|s| s.contains(&Value::String(our_switch())))
    })
}

/// The file's text with our switch added, replacing a debug-port switch for a
/// different port. `None` if nothing needs to change.
pub fn with_debug_switch(text: &str) -> Result<Option<String>, String> {
    let mut config: Value =
        serde_json::from_str(text).map_err(|e| format!("the file is not valid JSON: {e}"))?;
    let root = config
        .as_object_mut()
        .ok_or("the file is not a JSON object")?;
    let switches = root
        .entry("switches")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or("\"switches\" is not a list")?;

    let ours = Value::String(our_switch());
    if switches.contains(&ours) {
        return Ok(None);
    }
    switches.retain(|s| {
        !s.as_str()
            .is_some_and(|s| s.starts_with(&format!("{SWITCH_NAME}=")))
    });
    switches.push(ours);
    serde_json::to_string_pretty(&config)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Adds the switch to the file, keeping a `.bak` copy of the original.
/// Returns whether the file changed.
pub fn patch_file(path: &Path) -> io::Result<bool> {
    let text = fs::read_to_string(path)?;
    let Some(patched) = with_debug_switch(&text).map_err(io::Error::other)? else {
        return Ok(false);
    };
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bak");
    fs::write(backup, &text)?;
    fs::write(path, patched)?;
    Ok(true)
}

/// Why the overlay could not be repaired. Its text is shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairError {
    /// The user said no to the administrator prompt. Nothing changed.
    Declined,
    NvidiaAppMissing,
    Unreadable(String),
    /// The file is not the JSON the NVIDIA App writes. Nothing changed.
    NotUnderstood(String),
    /// The elevated copy could not write the file.
    WriteFailed,
    /// The elevated copy did not run, or ended in an unexpected way.
    Failed(String),
}

impl fmt::Display for RepairError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Declined => {
                f.write_str("The administrator prompt was declined, so nothing was changed.")
            }
            Self::NvidiaAppMissing => f.write_str("The NVIDIA App's overlay config was not found."),
            Self::Unreadable(e) => write!(f, "The overlay config could not be read: {e}."),
            Self::NotUnderstood(e) => write!(
                f,
                "The overlay config was left unchanged because {e}. Reinstalling the NVIDIA App rewrites it."
            ),
            Self::WriteFailed => f.write_str("The overlay config could not be written."),
            Self::Failed(e) => write!(f, "The overlay config could not be fixed: {e}."),
        }
    }
}

// Exit codes of the elevated `--patch-overlay` run. Only the code crosses
// back from the elevated copy, so each failure has its own.
const EXIT_PATCHED: u8 = 0;
const EXIT_NVIDIA_APP_MISSING: u8 = 10;
const EXIT_UNREADABLE: u8 = 11;
const EXIT_NOT_UNDERSTOOD: u8 = 12;
const EXIT_WRITE_FAILED: u8 = 13;

/// The patch the elevated copy runs: adds the switch to `config`, backing up
/// the original, and returns the process exit code for `repair` to read.
pub fn patch_and_exit_code(config: Option<&Path>) -> u8 {
    let Some(path) = config else {
        return EXIT_NVIDIA_APP_MISSING;
    };
    let Ok(text) = fs::read_to_string(path) else {
        return EXIT_UNREADABLE;
    };
    if with_debug_switch(&text).is_err() {
        return EXIT_NOT_UNDERSTOOD;
    }
    match patch_file(path) {
        Ok(_) => EXIT_PATCHED,
        Err(_) => EXIT_WRITE_FAILED,
    }
}

/// Adds the debug switch to `config` by running `elevate`, which must run
/// `patch_and_exit_code` with administrator rights and return its exit code.
///
/// The file is checked first without elevation, so a file that cannot be
/// patched is reported without an administrator prompt.
pub fn repair(
    config: Option<&Path>,
    elevate: impl FnOnce() -> Result<u32, ElevationError>,
) -> Result<(), RepairError> {
    let path = config.ok_or(RepairError::NvidiaAppMissing)?;
    let text = fs::read_to_string(path).map_err(|e| RepairError::Unreadable(e.to_string()))?;
    if with_debug_switch(&text)
        .map_err(RepairError::NotUnderstood)?
        .is_none()
    {
        return Ok(());
    }
    let code = elevate().map_err(|e| match e {
        ElevationError::Declined => RepairError::Declined,
        ElevationError::Failed(e) => RepairError::Failed(e),
    })?;
    match u8::try_from(code) {
        Ok(EXIT_PATCHED) => Ok(()),
        Ok(EXIT_NVIDIA_APP_MISSING) => Err(RepairError::NvidiaAppMissing),
        Ok(EXIT_UNREADABLE) => Err(RepairError::Unreadable(
            "the administrator copy could not open it".into(),
        )),
        Ok(EXIT_NOT_UNDERSTOOD) => Err(RepairError::NotUnderstood("it is not valid JSON".into())),
        Ok(EXIT_WRITE_FAILED) => Err(RepairError::WriteFailed),
        _ => Err(RepairError::Failed(format!(
            "the administrator copy exited with code {code}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: &str = r#"{"switches":["nv-combine-processes=false","nv-osc=true"],"other":1}"#;

    #[test]
    fn adds_the_switch_and_keeps_everything_else() {
        assert!(!has_debug_switch(ORIGINAL));
        let patched = with_debug_switch(ORIGINAL).unwrap().unwrap();
        assert!(has_debug_switch(&patched));
        let config: Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(
            config["switches"],
            serde_json::json!([
                "nv-combine-processes=false",
                "nv-osc=true",
                "nv-remote-debugging-port=49222"
            ])
        );
        assert_eq!(config["other"], 1);
        assert_eq!(
            with_debug_switch(&patched),
            Ok(None),
            "patching twice changes nothing"
        );
    }

    #[test]
    fn replaces_a_switch_for_another_port() {
        let text = r#"{"switches":["nv-remote-debugging-port=9000","nv-osc=true"]}"#;
        let patched = with_debug_switch(text).unwrap().unwrap();
        let config: Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(
            config["switches"],
            serde_json::json!(["nv-osc=true", "nv-remote-debugging-port=49222"])
        );
    }

    #[test]
    fn refuses_a_file_it_does_not_understand() {
        assert!(with_debug_switch("not json").is_err());
        assert!(with_debug_switch(r#"{"switches":"x"}"#).is_err());
        assert!(!has_debug_switch("not json"));
    }

    #[test]
    fn patches_the_file_and_backs_up_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("NVIDIA Overlay.json");
        fs::write(&path, ORIGINAL).unwrap();

        assert!(patch_file(&path).unwrap());
        assert!(has_debug_switch(&fs::read_to_string(&path).unwrap()));
        let backup = dir.path().join("NVIDIA Overlay.json.bak");
        assert_eq!(fs::read_to_string(backup).unwrap(), ORIGINAL);
        assert!(!patch_file(&path).unwrap());
    }

    fn overlay_config(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("NVIDIA Overlay.json");
        fs::write(&path, text).unwrap();
        (dir, path)
    }

    /// Stands in for the elevated copy of the app: it runs the patch the way
    /// `--patch-overlay` does and returns its exit code.
    fn elevated_patch(path: &Path) -> impl FnOnce() -> Result<u32, ElevationError> + '_ {
        move || Ok(patch_and_exit_code(Some(path)).into())
    }

    #[test]
    fn repair_patches_the_file_through_the_elevated_copy() {
        let (dir, path) = overlay_config(ORIGINAL);
        assert_eq!(repair(Some(&path), elevated_patch(&path)), Ok(()));
        assert!(has_debug_switch(&fs::read_to_string(&path).unwrap()));
        let backup = dir.path().join("NVIDIA Overlay.json.bak");
        assert_eq!(fs::read_to_string(backup).unwrap(), ORIGINAL);
    }

    #[test]
    fn declining_the_prompt_leaves_the_file_untouched() {
        let (dir, path) = overlay_config(ORIGINAL);
        let result = repair(Some(&path), || Err(ElevationError::Declined));
        assert_eq!(result, Err(RepairError::Declined));
        assert!(result.unwrap_err().to_string().contains("declined"));
        assert_eq!(fs::read_to_string(&path).unwrap(), ORIGINAL);
        assert!(!dir.path().join("NVIDIA Overlay.json.bak").exists());
    }

    #[test]
    fn a_file_that_is_not_json_is_reported_without_a_prompt() {
        let (_dir, path) = overlay_config("not json");
        let result = repair(Some(&path), || panic!("no administrator prompt expected"));
        assert!(matches!(result, Err(RepairError::NotUnderstood(_))));
        assert_eq!(fs::read_to_string(&path).unwrap(), "not json");
    }

    #[test]
    fn a_missing_or_unreadable_file_is_reported() {
        let never = || -> Result<u32, ElevationError> { panic!("no prompt expected") };
        assert_eq!(repair(None, never), Err(RepairError::NvidiaAppMissing));
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("NVIDIA Overlay.json");
        assert!(matches!(
            repair(Some(&gone), never),
            Err(RepairError::Unreadable(_))
        ));
    }

    #[test]
    fn an_already_patched_file_needs_no_prompt() {
        let (_dir, path) = overlay_config(ORIGINAL);
        patch_file(&path).unwrap();
        assert_eq!(repair(Some(&path), || panic!("no prompt expected")), Ok(()));
    }

    #[test]
    fn a_failure_in_the_elevated_copy_is_reported() {
        let (_dir, path) = overlay_config(ORIGINAL);
        // The file changed between the check and the elevated run.
        let elevated = || {
            fs::write(&path, "{broken").unwrap();
            Ok(patch_and_exit_code(Some(&path)).into())
        };
        assert!(matches!(
            repair(Some(&path), elevated),
            Err(RepairError::NotUnderstood(_))
        ));

        let (_dir, path) = overlay_config(ORIGINAL);
        assert_eq!(
            repair(Some(&path), || Ok(1)),
            Err(RepairError::Failed(
                "the administrator copy exited with code 1".into()
            ))
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), ORIGINAL);
    }
}
