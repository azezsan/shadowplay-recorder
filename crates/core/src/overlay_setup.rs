//! Opens the NVIDIA overlay's debug port by editing `NVIDIA Overlay.json`.
//!
//! The overlay reads its Chromium switches from that file at launch. Adding
//! `nv-remote-debugging-port=<port>` makes it listen on loopback, which is how
//! `overlay` reaches it. The file sits under Program Files, so writing it
//! needs administrator rights, and an NVIDIA App update can reset it.
//!
//! The port is unauthenticated: any program on the same machine can use it to
//! run code in the overlay page. It is not reachable from other machines.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::overlay::DEBUG_PORT;

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
}
