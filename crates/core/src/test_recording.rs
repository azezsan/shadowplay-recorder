//! The setup checklist's test recording: proves the whole chain by sending
//! the start command, waiting a few seconds, sending the stop and checking
//! that a recording appeared in the recordings folder.
//!
//! As in a match, the file is the proof and a command sent without a reply
//! counts as sent. The stop is only sent once the file exists, because a stop
//! with nothing recording would start a recording instead.
//! `Engine::test_recording` refuses the test during a match, so it can never
//! interfere with a real recording.

use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::overlay::{OverlayError, RecordChannel, RecordCommand};
use crate::recorder::newest_recording_since;
use crate::session::Phase;

/// How long the test records before sending the stop.
pub const TEST_RECORDING_LENGTH: Duration = Duration::from_secs(5);

/// Why a test recording failed or was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TestRecordingError {
    /// The recorder is in a match or recording one; the test would get in
    /// its way.
    Busy { phase: Phase },
    /// No recordings folder is set, so the result could not be checked.
    NoRecordingsFolder,
    /// The overlay's debug port could not be reached; nothing was sent.
    OverlayUnreachable { detail: String },
    /// The overlay answered and refused the start command.
    StartRejected { detail: String },
    /// The start was sent but no file appeared in the recordings folder,
    /// which usually means ShadowPlay saves somewhere else.
    NoRecordingAppeared { folder: String },
    /// A recording started but the stop failed, so it may still be running.
    StopFailed { recording: String, detail: String },
}

/// The phases in which a test recording may run.
pub fn allowed_in(phase: Phase) -> bool {
    matches!(phase, Phase::Idle | Phase::GameRunning)
}

/// Runs the test against `folder` and returns the recording's path.
pub(crate) fn run(
    channel: &mut impl RecordChannel,
    folder: &Path,
    length: Duration,
) -> Result<String, TestRecordingError> {
    log::info!("test recording: starting");
    let since = SystemTime::now();
    send(channel, RecordCommand::Start).map_err(|e| match e {
        OverlayError::Rejected(detail) => TestRecordingError::StartRejected { detail },
        e => TestRecordingError::OverlayUnreachable {
            detail: e.to_string(),
        },
    })?;
    thread::sleep(length);

    let Some(recording) = newest_recording_since(folder, since) else {
        log::warn!("test recording: no recording appeared in {folder:?}");
        return Err(TestRecordingError::NoRecordingAppeared {
            folder: folder.to_string_lossy().into_owned(),
        });
    };
    let recording = recording.to_string_lossy().into_owned();
    send(channel, RecordCommand::Stop).map_err(|e| TestRecordingError::StopFailed {
        recording: recording.clone(),
        detail: e.to_string(),
    })?;
    log::info!("test recording: passed, recorded {recording}");
    Ok(recording)
}

/// Sends a command, treating a missing reply as sent.
fn send(channel: &mut impl RecordChannel, command: RecordCommand) -> Result<(), OverlayError> {
    match channel.send(command) {
        Err(OverlayError::NoAck(e)) => {
            log::info!("test recording: {command:?} command sent, no reply ({e})");
            Ok(())
        }
        Err(e) => {
            log::warn!("test recording: {command:?} command failed: {e}");
            Err(e)
        }
        Ok(()) => Ok(()),
    }
}
