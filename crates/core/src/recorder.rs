//! Starts and stops a ShadowPlay recording and names the resulting file.
//!
//! A recording counts as started only once a new video file appears in the
//! recordings folder: the overlay's reply is not trusted as proof. A command
//! that was sent but not acknowledged is treated as sent for the same reason.
//! After the stop, the file is renamed from ShadowPlay's default name to the
//! match id once it has stopped growing.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::overlay::{OverlayError, RecordChannel, RecordCommand};

const RECORDING_EXTENSIONS: [&str; 3] = ["mp4", "mkv", "mov"];

/// What the match session needs from a recorder.
pub trait Recorder {
    /// Sends the start command once. Confirmation comes from
    /// [`Recorder::has_recording_since`], never from this call.
    fn start(&mut self);
    /// True if a recording file newer than `since` exists.
    fn has_recording_since(&self, since: SystemTime) -> bool;
    /// Sends stop-and-save if a recording exists. `confirmed` is the caller's
    /// belief; when false the folder is checked for a manually started one, so
    /// a stop is never sent blind (which would start a recording instead).
    /// Returns whether the stop was sent.
    fn stop(&mut self, confirmed: bool, since: SystemTime) -> bool;
    /// Waits for the newest recording to stop growing, then names it after
    /// the match. Returns the new path.
    fn rename(&mut self, match_id: &str, since: SystemTime) -> Option<PathBuf>;
}

/// How long `rename` waits for the file to settle.
#[derive(Debug, Clone, Copy)]
pub struct RenameTiming {
    pub stable_for: Duration,
    pub poll: Duration,
    pub give_up_after: Duration,
}

impl Default for RenameTiming {
    fn default() -> Self {
        Self {
            stable_for: Duration::from_secs(1),
            poll: Duration::from_millis(500),
            give_up_after: Duration::from_secs(30),
        }
    }
}

pub struct ShadowPlayRecorder<C: RecordChannel> {
    folder: PathBuf,
    channel: C,
    timing: RenameTiming,
}

impl<C: RecordChannel> ShadowPlayRecorder<C> {
    pub fn new(folder: impl Into<PathBuf>, channel: C) -> Self {
        Self {
            folder: folder.into(),
            channel,
            timing: RenameTiming::default(),
        }
    }

    pub fn with_timing(mut self, timing: RenameTiming) -> Self {
        self.timing = timing;
        self
    }

    pub fn set_folder(&mut self, folder: impl Into<PathBuf>) {
        self.folder = folder.into();
    }

    pub fn set_timing(&mut self, timing: RenameTiming) {
        self.timing = timing;
    }

    pub fn channel(&self) -> &C {
        &self.channel
    }

    pub fn channel_mut(&mut self) -> &mut C {
        &mut self.channel
    }

    /// True if the command was accepted or sent without a reply.
    fn send(&mut self, command: RecordCommand) -> bool {
        match self.channel.send(command) {
            Ok(()) => {
                log::info!("{command:?} command accepted by the overlay");
                true
            }
            Err(OverlayError::NoAck(e)) => {
                log::info!("{command:?} command sent, no reply ({e})");
                true
            }
            Err(e) => {
                log::warn!("{command:?} command failed: {e}");
                false
            }
        }
    }

    fn wait_until_stable(&self, since: SystemTime) -> Option<PathBuf> {
        let deadline = Instant::now() + self.timing.give_up_after;
        let mut last: Option<(PathBuf, u64, Instant)> = None;
        while Instant::now() < deadline {
            if let Some(path) = newest_recording_since(&self.folder, since) {
                let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(u64::MAX);
                match &last {
                    Some((p, s, unchanged_since)) if *p == path && *s == size => {
                        if unchanged_since.elapsed() >= self.timing.stable_for {
                            return Some(path);
                        }
                    }
                    _ => last = Some((path, size, Instant::now())),
                }
            }
            thread::sleep(self.timing.poll);
        }
        None
    }
}

impl<C: RecordChannel> Recorder for ShadowPlayRecorder<C> {
    fn start(&mut self) {
        self.send(RecordCommand::Start);
    }

    fn has_recording_since(&self, since: SystemTime) -> bool {
        newest_recording_since(&self.folder, since).is_some()
    }

    fn stop(&mut self, confirmed: bool, since: SystemTime) -> bool {
        if !confirmed {
            match newest_recording_since(&self.folder, since) {
                Some(path) => log::info!("found a manually started recording ({path:?})"),
                None => {
                    log::info!("no recording in progress; nothing to stop");
                    return false;
                }
            }
        }
        self.send(RecordCommand::Stop);
        true
    }

    fn rename(&mut self, match_id: &str, since: SystemTime) -> Option<PathBuf> {
        let Some(recording) = self.wait_until_stable(since) else {
            log::warn!(
                "no finished recording appeared in {:?} within {:?}",
                self.folder,
                self.timing.give_up_after
            );
            return None;
        };
        let name = sanitize(match_id);
        if name.is_empty() {
            log::warn!("match id {match_id:?} gives an empty file name; leaving the file as is");
            return None;
        }
        let target = match recording.extension() {
            Some(ext) => recording.with_file_name(format!("{name}.{}", ext.to_string_lossy())),
            None => recording.with_file_name(name),
        };
        if target == recording {
            return Some(target);
        }
        match fs::rename(&recording, &target) {
            Ok(()) => Some(target),
            Err(e) => {
                log::warn!("could not rename {recording:?}: {e}");
                None
            }
        }
    }
}

/// Newest recording in `folder` modified at or after `since`.
pub fn newest_recording_since(folder: &Path, since: SystemTime) -> Option<PathBuf> {
    fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter(|e| has_extension(&e.path(), &RECORDING_EXTENSIONS))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?;
            (meta.is_file() && modified >= since).then(|| (modified, e.path()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

pub(crate) fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.contains(&e.to_lowercase().as_str()))
}

/// Keeps letters, digits, space, `_`, `-` and `.`; everything else becomes `_`.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " _-.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
pub(crate) mod testing {
    use std::fs::File;

    use super::*;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Mode {
        Ok,
        Fail,
        NoAck,
        /// The overlay answers and refuses every command.
        Reject,
    }

    /// Stands in for the overlay. A start that takes effect creates a
    /// recording file the way ShadowPlay would, so file confirmation works.
    pub struct FakeChannel {
        pub sent: Vec<RecordCommand>,
        pub folder: Option<PathBuf>,
        pub mode: Mode,
        /// The first N start commands fail, to exercise the resend loop.
        pub fail_first: usize,
    }

    impl FakeChannel {
        pub fn new(folder: Option<&Path>, mode: Mode) -> Self {
            Self {
                sent: Vec::new(),
                folder: folder.map(Path::to_path_buf),
                mode,
                fail_first: 0,
            }
        }

        pub fn stops(&self) -> usize {
            self.sent
                .iter()
                .filter(|c| **c == RecordCommand::Stop)
                .count()
        }

        fn create_recording(&self) {
            let Some(folder) = &self.folder else { return };
            let path = folder.join("DefaultShadowPlay.mp4");
            if !path.exists() {
                fs::write(&path, vec![b'x'; 1024]).unwrap();
                let file = File::options().write(true).open(&path).unwrap();
                file.set_modified(SystemTime::now() + Duration::from_secs(5))
                    .unwrap();
            }
        }
    }

    impl RecordChannel for FakeChannel {
        fn send(&mut self, command: RecordCommand) -> Result<(), OverlayError> {
            self.sent.push(command);
            let is_start = command == RecordCommand::Start;
            let starts = self.sent.len() - self.stops();
            if is_start && starts <= self.fail_first {
                return Err(OverlayError::Unavailable("overlay busy (test)".into()));
            }
            match self.mode {
                Mode::Fail => Err(OverlayError::Unavailable("unreachable (test)".into())),
                Mode::Reject => Err(OverlayError::Rejected("busy (test)".into())),
                Mode::NoAck => {
                    if is_start {
                        self.create_recording();
                    }
                    Err(OverlayError::NoAck("no ack (test)".into()))
                }
                Mode::Ok => {
                    if is_start {
                        self.create_recording();
                    }
                    Ok(())
                }
            }
        }
    }

    pub fn fast_recorder(folder: &Path, channel: FakeChannel) -> ShadowPlayRecorder<FakeChannel> {
        ShadowPlayRecorder::new(folder, channel).with_timing(RenameTiming {
            stable_for: Duration::from_millis(20),
            poll: Duration::from_millis(10),
            give_up_after: Duration::from_millis(500),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn finds_only_recordings_newer_than_the_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let before = SystemTime::now() - Duration::from_secs(1);
        fs::write(dir.path().join("rec.MP4"), b"x").unwrap();
        fs::write(dir.path().join("notes.txt"), b"x").unwrap();

        let found = newest_recording_since(dir.path(), before).unwrap();
        assert_eq!(found.file_name().unwrap(), "rec.MP4");
        let later = SystemTime::now() + Duration::from_secs(100);
        assert_eq!(newest_recording_since(dir.path(), later), None);
        assert_eq!(
            newest_recording_since(&dir.path().join("gone"), before),
            None
        );
    }

    #[test]
    fn never_stops_when_nothing_is_recording() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = fast_recorder(dir.path(), FakeChannel::new(None, Mode::Ok));
        assert!(!recorder.stop(false, SystemTime::now()));
        assert_eq!(recorder.channel().stops(), 0);
    }

    #[test]
    fn stops_a_manually_started_recording() {
        let dir = tempfile::tempdir().unwrap();
        let since = SystemTime::now() - Duration::from_secs(1);
        fs::write(dir.path().join("manual.mp4"), b"x").unwrap();
        let mut recorder = fast_recorder(dir.path(), FakeChannel::new(None, Mode::Ok));
        assert!(recorder.stop(false, since));
        assert_eq!(recorder.channel().stops(), 1);
    }

    #[test]
    fn renames_the_recording_to_the_match_id() {
        let dir = tempfile::tempdir().unwrap();
        let since = SystemTime::now() - Duration::from_secs(1);
        fs::write(dir.path().join("Deadlock 2026.11.18.mp4"), b"x").unwrap();
        let mut recorder = fast_recorder(dir.path(), FakeChannel::new(None, Mode::Ok));

        let renamed = recorder.rename("81038409", since).unwrap();
        assert_eq!(renamed, dir.path().join("81038409.mp4"));
        assert!(renamed.exists());
    }

    #[test]
    fn rename_gives_up_when_no_recording_appears() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = fast_recorder(dir.path(), FakeChannel::new(None, Mode::Ok));
        assert_eq!(recorder.rename("1", SystemTime::now()), None);
    }

    #[test]
    fn sanitize_replaces_unsafe_characters() {
        assert_eq!(sanitize("81038409"), "81038409");
        assert_eq!(sanitize(" a/b:c "), "a_b_c");
    }
}
