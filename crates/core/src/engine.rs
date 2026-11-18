//! The background loop: polls the game process and its console log, feeds the
//! match session, and applies the size cap after the game closes.
//!
//! `Engine::step` does one iteration and is called about once a second. It
//! owns no thread and no clock; the shell calls it and passes the time in.
//! OS access goes through [`Host`], so the whole loop is tested with fakes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::game::GameProfile;
use crate::overlay::RecordChannel;
use crate::recorder::ShadowPlayRecorder;
use crate::session::{GameWindow, Phase, Session, SessionConfig};
use crate::settings::Settings;
use crate::tail::Tailer;
use crate::test_recording::{self, TestRecordingError, TestRecordingTiming};
use crate::{platform, retention, setup};

/// How often `step` should be called.
pub const STEP_INTERVAL: Duration = Duration::from_secs(1);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Wait after the game closes before applying the size cap.
const CLEANUP_DELAY: Duration = Duration::from_secs(30);
const MISSING_LOG_WARNING_AFTER: Duration = Duration::from_secs(30);

/// What the engine needs to ask the operating system.
pub trait Host {
    fn is_game_running(&self, game: &GameProfile) -> bool;
    fn steam_root(&self) -> Option<PathBuf>;
}

/// The real operating system.
pub struct SystemHost;

impl Host for SystemHost {
    fn is_game_running(&self, game: &GameProfile) -> bool {
        platform::is_process_running(game.process)
    }

    fn steam_root(&self) -> Option<PathBuf> {
        platform::steam_root()
    }
}

/// What the app is doing right now, for the status line and the tray.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub phase: Phase,
    pub match_id: Option<String>,
    /// A test recording is running. The phase stays what it was meanwhile.
    pub test_recording_running: bool,
    /// A test recording may be started now.
    pub test_recording_allowed: bool,
}

impl Status {
    /// The status before the engine's first step.
    pub fn idle() -> Self {
        Self::of(Phase::Idle, None, false)
    }

    fn of(phase: Phase, match_id: Option<String>, test_recording_running: bool) -> Self {
        Self {
            phase,
            match_id,
            test_recording_running,
            test_recording_allowed: !test_recording_running && test_recording::allowed_in(phase),
        }
    }
}

pub struct Engine<C: RecordChannel, W: GameWindow, H: Host> {
    game: &'static GameProfile,
    host: H,
    session: Session<ShadowPlayRecorder<C>, W>,
    settings: Settings,
    tailer: Option<Tailer>,
    next_process_poll: Duration,
    cleanup_at: Option<Duration>,
    log_missing_since: Option<Duration>,
    warned_log_missing: bool,
    warned_no_folder: bool,
    test_recording_timing: TestRecordingTiming,
}

impl<C: RecordChannel, W: GameWindow, H: Host> Engine<C, W, H> {
    pub fn new(game: &'static GameProfile, channel: C, window: W, host: H) -> Self {
        let recorder = ShadowPlayRecorder::new(PathBuf::new(), channel);
        let mut engine = Self {
            game,
            host,
            session: Session::new(game, SessionConfig::default(), recorder, window),
            settings: Settings::default(),
            tailer: None,
            next_process_poll: Duration::ZERO,
            // Also apply the cap once at startup.
            cleanup_at: Some(Duration::ZERO),
            log_missing_since: None,
            warned_log_missing: false,
            warned_no_folder: false,
            test_recording_timing: TestRecordingTiming::default(),
        };
        engine.apply_settings(Settings::default());
        engine
    }

    /// Replaces how long a test recording runs and waits; tests shorten it.
    pub fn with_test_recording_timing(mut self, timing: TestRecordingTiming) -> Self {
        self.test_recording_timing = timing;
        self
    }

    pub fn session_mut(&mut self) -> &mut Session<ShadowPlayRecorder<C>, W> {
        &mut self.session
    }

    fn channel_mut(&mut self) -> &mut C {
        self.session.recorder_mut().channel_mut()
    }

    /// Takes new settings into use. Safe to call on every step.
    pub fn apply_settings(&mut self, settings: Settings) {
        if settings == self.settings {
            return;
        }
        self.session.config.focus_on_match_start = settings.focus_on_match_start;
        self.session.config.stop_delay = Duration::from_secs(settings.stop_delay_seconds.into());
        self.session
            .recorder_mut()
            .set_folder(settings.recordings_folder.clone().unwrap_or_default());
        self.warned_no_folder = false;
        if settings.console_log != self.settings.console_log {
            self.tailer = None;
        }
        self.settings = settings;
    }

    pub fn status(&self) -> Status {
        self.status_with(false)
    }

    fn status_with(&self, test_recording_running: bool) -> Status {
        Status::of(
            self.session.phase(),
            self.session.match_id().map(str::to_string),
            test_recording_running,
        )
    }

    /// Runs the setup checklist's test recording and returns the path of the
    /// file it recorded. Refused unless the phase is idle or game running.
    /// Blocks for the test's length and its wait for a late file; no step
    /// runs meanwhile, so a match cannot start during the test. `report` is
    /// handed the status as the test starts and again as it ends, so the rest
    /// of the app (the update gate among it) knows a test is running.
    pub fn test_recording(
        &mut self,
        mut report: impl FnMut(&Status),
    ) -> Result<String, TestRecordingError> {
        let phase = self.session.phase();
        if !test_recording::allowed_in(phase) {
            log::info!("test recording refused: the recorder is busy ({phase:?})");
            return Err(TestRecordingError::Busy { phase });
        }
        let Some(folder) = self.settings.recordings_folder.clone() else {
            return Err(TestRecordingError::NoRecordingsFolder);
        };
        let timing = self.test_recording_timing;
        report(&self.status_with(true));
        let result = test_recording::run(self.channel_mut(), Path::new(&folder), timing);
        report(&self.status());
        result
    }

    pub fn step(&mut self, now: Duration, wall: SystemTime) -> Status {
        if now >= self.next_process_poll {
            self.next_process_poll = now + PROCESS_POLL_INTERVAL;
            let exited = self
                .session
                .on_running(self.host.is_game_running(self.game));
            if exited {
                self.cleanup_at = Some(now + CLEANUP_DELAY);
                // The game may be reinstalled elsewhere before its next run.
                self.tailer = None;
            }
        }

        if self.session.is_running() {
            self.read_console_log(now, wall);
        }
        self.session.tick(now);

        if self.cleanup_at.is_some_and(|at| now >= at) {
            self.cleanup_at = None;
            if let Some(folder) = &self.settings.recordings_folder {
                let cap = retention::gib_to_bytes(self.settings.max_size_gb);
                retention::enforce_size_cap(Path::new(folder), cap);
            }
        }
        self.status()
    }

    fn read_console_log(&mut self, now: Duration, wall: SystemTime) {
        if self.tailer.is_none() {
            let steam_root = self.host.steam_root();
            self.tailer = setup::console_log_path(&self.settings, self.game, steam_root.as_deref())
                .inspect(|path| log::info!("following {path:?}"))
                .map(Tailer::new);
        }
        let Some(result) = self.tailer.as_mut().and_then(Tailer::poll) else {
            let since = *self.log_missing_since.get_or_insert(now);
            if !self.warned_log_missing && now - since >= MISSING_LOG_WARNING_AFTER {
                self.warned_log_missing = true;
                log::warn!(
                    "{} is running but writes no console log; add {} to its Steam launch options",
                    self.game.name,
                    self.game.console_log_launch_option
                );
            }
            return;
        };
        self.log_missing_since = None;
        self.warned_log_missing = false;
        if result.truncated {
            log::info!("the console log was restarted; reading it from the beginning");
        }

        for line in result.lines {
            let Some(event) = (self.game.parse_line)(&line) else {
                continue;
            };
            // Without a folder a recording could be started but never
            // confirmed or stopped, so nothing is recorded at all.
            if self.settings.recordings_folder.is_none() {
                if !self.warned_no_folder {
                    self.warned_no_folder = true;
                    log::warn!("no recordings folder is set; matches are not recorded");
                }
                continue;
            }
            self.session.on_event(event, now, wall);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::rc::Rc;

    use super::*;
    use crate::game::DEADLOCK;
    use crate::overlay::RecordCommand;
    use crate::recorder::RenameTiming;
    use crate::recorder::testing::{FakeChannel, Mode};
    use crate::update;

    struct FakeHost {
        running: Rc<Cell<bool>>,
    }

    impl Host for FakeHost {
        fn is_game_running(&self, _game: &GameProfile) -> bool {
            self.running.get()
        }

        fn steam_root(&self) -> Option<PathBuf> {
            None
        }
    }

    /// Always in the foreground; counts the times it was focused.
    struct FakeWindow {
        focuses: Rc<Cell<u32>>,
    }

    impl GameWindow for FakeWindow {
        fn focus(&mut self) -> bool {
            self.focuses.set(self.focuses.get() + 1);
            true
        }

        fn is_foreground(&self) -> bool {
            true
        }
    }

    struct Fixture {
        engine: Engine<FakeChannel, FakeWindow, FakeHost>,
        settings: Settings,
        running: Rc<Cell<bool>>,
        focuses: Rc<Cell<u32>>,
        log: PathBuf,
        recordings: PathBuf,
        now: Duration,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new(with_folder: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let recordings = dir.path().join("recordings");
            fs::create_dir(&recordings).unwrap();
            let log = dir.path().join("console.log");
            fs::write(&log, "old session line\n").unwrap();
            let running = Rc::new(Cell::new(false));

            let channel = FakeChannel::new(Some(&recordings), Mode::Ok);
            let host = FakeHost {
                running: running.clone(),
            };
            let focuses = Rc::new(Cell::new(0));
            let window = FakeWindow {
                focuses: focuses.clone(),
            };
            let mut engine = Engine::new(&DEADLOCK, channel, window, host)
                .with_test_recording_timing(TestRecordingTiming {
                    length: Duration::ZERO,
                    late_file_grace: Duration::from_millis(300),
                    poll: Duration::from_millis(10),
                });
            let settings = Settings {
                recordings_folder: with_folder.then(|| recordings.to_string_lossy().into()),
                console_log: Some(log.to_string_lossy().into()),
                stop_delay_seconds: 0,
                max_size_gb: 1.0,
                ..Settings::default()
            };
            engine.apply_settings(settings.clone());
            let session = engine.session_mut();
            session.config.start_delay = Duration::ZERO;
            session.recorder_mut().set_timing(RenameTiming {
                stable_for: Duration::from_millis(20),
                poll: Duration::from_millis(10),
                give_up_after: Duration::from_millis(500),
            });
            Self {
                engine,
                settings,
                running,
                focuses,
                log,
                recordings,
                now: Duration::ZERO,
                _dir: dir,
            }
        }

        /// Advances by whole seconds, one step per second.
        fn run(&mut self, seconds: u64) -> Status {
            let mut status = self.engine.status();
            for _ in 0..seconds {
                self.now += STEP_INTERVAL;
                status = self.engine.step(self.now, SystemTime::now());
            }
            status
        }

        /// Changes the settings the way the shell does: handing the engine
        /// the whole new value before its next step.
        fn change_settings(&mut self, change: impl FnOnce(&mut Settings)) {
            change(&mut self.settings);
            self.engine.apply_settings(self.settings.clone());
        }

        fn write_log(&self, text: &str) {
            let mut file = OpenOptions::new().append(true).open(&self.log).unwrap();
            file.write_all(text.as_bytes()).unwrap();
        }

        fn sent(&mut self) -> Vec<RecordCommand> {
            self.engine.channel_mut().sent.clone()
        }
    }

    #[test]
    fn records_a_match_read_from_the_console_log() {
        let mut f = Fixture::new(true);
        assert_eq!(f.run(1).phase, Phase::Idle);

        f.running.set(true);
        assert_eq!(f.run(3).phase, Phase::GameRunning);

        f.write_log("Lobby 5 for Match 81038409 created\n[Client] Map: \"start\"\n");
        let status = f.run(2);
        assert_eq!(status.phase, Phase::Recording);
        assert_eq!(status.match_id.as_deref(), Some("81038409"));

        f.write_log("Lobby 5 for Match 81038409 destroyed\n");
        assert_eq!(f.run(1).phase, Phase::GameRunning);
        assert_eq!(f.sent(), [RecordCommand::Start, RecordCommand::Stop]);
        assert!(f.recordings.join("81038409.mp4").exists());
    }

    #[test]
    fn uses_changed_settings_without_a_restart() {
        let mut f = Fixture::new(true);
        f.running.set(true);
        f.run(3);
        f.write_log("Lobby 5 for Match 1 created\n[Client] Map: \"start\"\n");
        f.run(2);
        f.write_log("Lobby 5 for Match 1 destroyed\n");
        f.run(1);
        assert_eq!(f.focuses.get(), 1);
        assert!(f.recordings.join("1.mp4").exists());

        // The player moves the recordings folder, lengthens the stop delay
        // and turns focusing off while the app keeps running.
        let moved = f.recordings.parent().unwrap().join("moved");
        fs::create_dir(&moved).unwrap();
        f.change_settings(|s| {
            s.recordings_folder = Some(moved.to_string_lossy().into());
            s.stop_delay_seconds = 5;
            s.focus_on_match_start = false;
        });
        f.engine.channel_mut().folder = Some(moved.clone());

        f.write_log("Lobby 6 for Match 2 created\n[Client] Map: \"start\"\n");
        assert_eq!(f.run(2).phase, Phase::Recording);
        assert_eq!(f.focuses.get(), 1, "focusing is off now");

        f.write_log("Lobby 6 for Match 2 destroyed\n");
        assert_eq!(f.run(5).phase, Phase::Finishing);
        assert_eq!(f.sent().last(), Some(&RecordCommand::Start));
        assert_eq!(f.run(1).phase, Phase::GameRunning);
        assert_eq!(f.sent().last(), Some(&RecordCommand::Stop));
        assert!(moved.join("2.mp4").exists());
        assert!(!f.recordings.join("2.mp4").exists());
    }

    #[test]
    fn records_nothing_without_a_recordings_folder() {
        let mut f = Fixture::new(false);
        f.running.set(true);
        f.run(3);
        f.write_log("Lobby 5 for Match 1 created\n[Client] Map: \"start\"\n");
        assert_eq!(f.run(3).phase, Phase::GameRunning);
        assert!(f.sent().is_empty());
    }

    #[test]
    fn applies_the_size_cap_after_the_game_closes() {
        let mut f = Fixture::new(true);
        f.run(1);
        let old = f.recordings.join("old.mp4");
        let new = f.recordings.join("new.mp4");
        for (path, age_hours) in [(&old, 2), (&new, 1)] {
            let file = File::create(path).unwrap();
            file.set_len(700 * 1024 * 1024).unwrap();
            file.set_modified(SystemTime::now() - Duration::from_secs(age_hours * 3600))
                .unwrap();
        }

        f.running.set(true);
        f.run(3);
        f.running.set(false);
        f.run(6);
        assert!(
            old.exists(),
            "the cap waits out the delay after the game closes"
        );
        f.run(CLEANUP_DELAY.as_secs());
        assert!(!old.exists());
        assert!(new.exists());
    }

    #[test]
    fn a_test_recording_names_the_file_it_recorded() {
        let mut f = Fixture::new(true);
        f.run(1);
        let recording = f.engine.test_recording(|_| {}).unwrap();
        assert_eq!(f.sent(), [RecordCommand::Start, RecordCommand::Stop]);
        assert_eq!(
            Path::new(&recording),
            f.recordings.join("DefaultShadowPlay.mp4")
        );
    }

    #[test]
    fn no_update_installs_while_a_test_recording_runs() {
        let mut f = Fixture::new(true);
        assert!(update::may_install(&f.run(1)));
        let mut reported = Vec::new();
        f.engine
            .test_recording(|status| reported.push(status.clone()))
            .unwrap();
        let [during, after] = reported.as_slice() else {
            panic!("expected a status as the test starts and one as it ends: {reported:?}");
        };
        assert_eq!(during.phase, Phase::Idle);
        assert!(during.test_recording_running);
        assert!(!update::may_install(during));
        assert!(!after.test_recording_running);
        assert!(update::may_install(after));
    }

    #[test]
    fn a_test_recording_is_refused_during_a_match() {
        let mut f = Fixture::new(true);
        f.running.set(true);
        f.run(3);
        f.write_log("Lobby 5 for Match 1 created\n[Client] Map: \"start\"\n");
        assert_eq!(f.run(2).phase, Phase::Recording);
        let sent = f.sent().len();

        assert_eq!(
            f.engine.test_recording(|_| {}),
            Err(TestRecordingError::Busy {
                phase: Phase::Recording
            })
        );
        assert_eq!(f.sent().len(), sent, "the test sent nothing");
    }

    #[test]
    fn a_test_recording_runs_while_the_game_is_out_of_a_match() {
        let mut f = Fixture::new(true);
        f.running.set(true);
        assert_eq!(f.run(3).phase, Phase::GameRunning);
        assert!(f.engine.test_recording(|_| {}).is_ok());
    }

    #[test]
    fn a_test_recording_needs_a_recordings_folder() {
        let mut f = Fixture::new(false);
        f.run(1);
        assert_eq!(
            f.engine.test_recording(|_| {}),
            Err(TestRecordingError::NoRecordingsFolder)
        );
        assert!(f.sent().is_empty());
    }

    #[test]
    fn a_test_recording_reports_an_unreachable_overlay() {
        let mut f = Fixture::new(true);
        f.engine.channel_mut().mode = Mode::Fail;
        assert!(matches!(
            f.engine.test_recording(|_| {}),
            Err(TestRecordingError::OverlayUnreachable { .. })
        ));
        assert_eq!(f.sent(), [RecordCommand::Start]);
    }

    #[test]
    fn a_test_recording_reports_a_rejected_command() {
        let mut f = Fixture::new(true);
        f.engine.channel_mut().mode = Mode::Reject;
        assert!(matches!(
            f.engine.test_recording(|_| {}),
            Err(TestRecordingError::StartRejected { detail }) if detail.contains("busy")
        ));
        assert_eq!(f.sent(), [RecordCommand::Start]);
    }

    #[test]
    fn a_test_recording_without_a_file_never_sends_the_stop() {
        let mut f = Fixture::new(true);
        // ShadowPlay saves somewhere other than the recordings folder.
        f.engine.channel_mut().folder = None;
        assert_eq!(
            f.engine.test_recording(|_| {}),
            Err(TestRecordingError::NoRecordingAppeared {
                folder: f.recordings.to_string_lossy().into_owned()
            })
        );
        assert_eq!(f.sent(), [RecordCommand::Start]);
    }

    #[test]
    fn a_test_recording_stops_a_recording_whose_file_appears_late() {
        let mut f = Fixture::new(true);
        // ShadowPlay creates the file only after the test's few seconds.
        f.engine.channel_mut().late_by = Some(Duration::from_millis(100));
        let recording = f.engine.test_recording(|_| {}).unwrap();
        assert_eq!(f.sent(), [RecordCommand::Start, RecordCommand::Stop]);
        assert_eq!(
            Path::new(&recording),
            f.recordings.join("DefaultShadowPlay.mp4")
        );
    }

    #[test]
    fn a_test_recording_counts_a_command_without_a_reply_as_sent() {
        let mut f = Fixture::new(true);
        f.engine.channel_mut().mode = Mode::NoAck;
        assert!(f.engine.test_recording(|_| {}).is_ok());
        assert_eq!(f.sent(), [RecordCommand::Start, RecordCommand::Stop]);
    }
}
