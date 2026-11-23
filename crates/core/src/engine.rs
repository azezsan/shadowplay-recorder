//! The background loop: polls the game process and its console log, feeds the
//! match session, and applies the retention rule: the size cap after the
//! game closes, or fill the disk's free-space reserve every minute and as a
//! recordable map loads.
//!
//! `Engine::step` does one iteration and is called about once a second. It
//! owns no thread and no clock; the shell calls it and passes the time in.
//! OS access goes through [`Host`], so the whole loop is tested with fakes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::game::GameProfile;
use crate::overlay::RecordChannel;
use crate::platform::DiskSpace;
use crate::recorder::ShadowPlayRecorder;
use crate::retention::{GIB, ReserveOutcome};
use crate::session::{GameWindow, Phase, Session, SessionConfig};
use crate::settings::{RetentionRule, Settings};
use crate::tail::Tailer;
use crate::test_recording::{self, TestRecordingError, TestRecordingTiming};
use crate::{platform, retention, setup};

/// How often `step` should be called.
pub const STEP_INTERVAL: Duration = Duration::from_secs(1);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Wait after the game closes before applying the size cap.
const CLEANUP_DELAY: Duration = Duration::from_secs(30);
const MISSING_LOG_WARNING_AFTER: Duration = Duration::from_secs(30);
/// How often the overlay config is re-read for the debug switch, which an
/// NVIDIA App update can remove while the app runs.
const SWITCH_CHECK_INTERVAL: Duration = Duration::from_secs(60);
/// How often fill the disk checks the free-space reserve.
const RESERVE_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// What the engine needs to ask the operating system.
pub trait Host {
    fn is_game_running(&self, game: &GameProfile) -> bool;
    fn steam_root(&self) -> Option<PathBuf>;
    /// True if the overlay config exists and lacks the debug switch.
    fn overlay_switch_missing(&self) -> bool;
    /// The disk holding `folder`; `None` if it cannot be read.
    fn disk_space(&self, folder: &Path) -> Option<DiskSpace>;
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

    fn overlay_switch_missing(&self) -> bool {
        setup::overlay_switch_missing()
    }

    fn disk_space(&self, folder: &Path) -> Option<DiskSpace> {
        platform::disk_space(folder)
    }
}

/// What the app is doing right now, for the status line and the tray.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub phase: Phase,
    pub match_id: Option<String>,
    /// The phase in words, for the window's status line and the tray.
    pub label: String,
    /// A test recording is running. The phase stays what it was meanwhile.
    pub test_recording_running: bool,
    /// A test recording may be started now.
    pub test_recording_allowed: bool,
    /// The recordings folder's disk, while fill the disk is in use and its
    /// free space could be read.
    pub storage: Option<Storage>,
}

/// The disk fill the disk works on, as of its last check. Sizes are whole GB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Storage {
    /// The disk's name, such as `D:`.
    pub disk: String,
    pub free_gb: u32,
    pub total_gb: u32,
    pub reserve_gb: u32,
    /// The reserve is the automatic one, not the user's.
    pub automatic: bool,
    /// Deleting every recording would not free the reserve, so the next
    /// recording may not fit.
    pub short: bool,
}

/// The disk part of a Windows path, such as `D:` or `\\server\share`.
fn disk_name(folder: &str) -> String {
    let bytes = folder.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        return folder[..2].to_uppercase();
    }
    if let Some(rest) = folder.strip_prefix(r"\\") {
        let share: Vec<_> = rest.splitn(3, '\\').take(2).collect();
        return format!(r"\\{}", share.join(r"\"));
    }
    folder.to_string()
}

fn whole_gb(bytes: u64) -> u32 {
    u32::try_from(bytes / GIB).unwrap_or(u32::MAX)
}

impl Status {
    /// The status before the engine's first step.
    pub fn idle(game: &GameProfile) -> Self {
        Self::new(game, Phase::Idle, None, false)
    }

    pub fn new(
        game: &GameProfile,
        phase: Phase,
        match_id: Option<String>,
        test_recording_running: bool,
    ) -> Self {
        Self {
            label: label(game, phase, match_id.as_deref()),
            phase,
            match_id,
            test_recording_running,
            test_recording_allowed: !test_recording_running && test_recording::allowed_in(phase),
            storage: None,
        }
    }
}

/// The phase in words.
fn label(game: &GameProfile, phase: Phase, match_id: Option<&str>) -> String {
    let name = game.name;
    let of_match = |id: &str| format!(" of match {id}");
    match (phase, match_id) {
        (Phase::Idle, _) => format!("Waiting for {name}"),
        (Phase::GameRunning, _) => format!("{name} is running"),
        (Phase::InMatch, Some(id)) => format!("In match {id}, not recording"),
        (Phase::InMatch, None) => "In a match, not recording".to_string(),
        (Phase::Starting, id) => format!(
            "Starting the recording{}",
            id.map(of_match).unwrap_or_default()
        ),
        (Phase::Recording, Some(id)) => format!("Recording match {id}"),
        (Phase::Recording, None) => "Recording".to_string(),
        (Phase::Finishing, id) => format!(
            "Saving the recording{}",
            id.map(of_match).unwrap_or_default()
        ),
    }
}

pub struct Engine<C: RecordChannel, W: GameWindow, H: Host> {
    game: &'static GameProfile,
    host: H,
    session: Session<ShadowPlayRecorder<C>, W>,
    settings: Settings,
    /// The recordings folder in use. A changed folder is taken into use only
    /// out of a match, so a recording is always stopped and named in the
    /// folder it was started in.
    folder: Option<String>,
    tailer: Option<Tailer>,
    next_process_poll: Duration,
    cleanup_at: Option<Duration>,
    log_missing_since: Option<Duration>,
    warned_log_missing: bool,
    warned_no_folder: bool,
    next_switch_check: Duration,
    /// Whether the debug switch was missing at the last check; `None`
    /// before the first.
    switch_missing: Option<bool>,
    window_requested: bool,
    test_recording_timing: TestRecordingTiming,
    next_reserve_check: Duration,
    /// Fill the disk's last check.
    storage: Option<Storage>,
    warned_disk_unreadable: bool,
}

impl<C: RecordChannel, W: GameWindow, H: Host> Engine<C, W, H> {
    pub fn new(game: &'static GameProfile, channel: C, window: W, host: H) -> Self {
        let recorder = ShadowPlayRecorder::new(PathBuf::new(), channel);
        let mut engine = Self {
            game,
            host,
            session: Session::new(game, SessionConfig::default(), recorder, window),
            settings: Settings::default(),
            folder: None,
            tailer: None,
            next_process_poll: Duration::ZERO,
            // Also apply the cap once at startup.
            cleanup_at: Some(Duration::ZERO),
            log_missing_since: None,
            warned_log_missing: false,
            warned_no_folder: false,
            next_switch_check: Duration::ZERO,
            switch_missing: None,
            window_requested: false,
            test_recording_timing: TestRecordingTiming::default(),
            next_reserve_check: Duration::ZERO,
            storage: None,
            warned_disk_unreadable: false,
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
        if settings.console_log != self.settings.console_log {
            self.tailer = None;
        }
        let retention_changed = (
            settings.retention_rule,
            settings.manual_reserve,
            settings.reserve_gb,
        ) != (
            self.settings.retention_rule,
            self.settings.manual_reserve,
            self.settings.reserve_gb,
        );
        if retention_changed {
            // Show the new rule's effect at the next step.
            self.next_reserve_check = Duration::ZERO;
            self.storage = None;
        }
        self.settings = settings;
        self.take_up_folder();
    }

    /// Takes the recordings folder from the settings into use, unless a
    /// match is in progress.
    fn take_up_folder(&mut self) {
        if self.folder == self.settings.recordings_folder {
            return;
        }
        if !matches!(self.session.phase(), Phase::Idle | Phase::GameRunning) {
            return;
        }
        if self.folder.is_some() {
            log::info!("now recording to {:?}", self.settings.recordings_folder);
        }
        self.folder = self.settings.recordings_folder.clone();
        self.session
            .recorder_mut()
            .set_folder(self.folder.clone().unwrap_or_default());
        self.warned_no_folder = false;
    }

    pub fn status(&self) -> Status {
        self.status_with(false)
    }

    fn status_with(&self, test_recording_running: bool) -> Status {
        Status {
            storage: self.storage.clone(),
            ..Status::new(
                self.game,
                self.session.phase(),
                self.session.match_id().map(str::to_string),
                test_recording_running,
            )
        }
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
        self.take_up_folder();
        let Some(folder) = self.folder.clone() else {
            return Err(TestRecordingError::NoRecordingsFolder);
        };
        let timing = self.test_recording_timing;
        report(&self.status_with(true));
        let result = test_recording::run(self.channel_mut(), Path::new(&folder), timing);
        report(&self.status());
        result
    }

    /// True, once, if the window should open to show something the user must
    /// fix: the overlay's debug switch has disappeared since the last check.
    /// A switch already missing at startup is the startup check's to show.
    pub fn take_window_request(&mut self) -> bool {
        std::mem::take(&mut self.window_requested)
    }

    fn check_switch(&mut self, now: Duration) {
        if now < self.next_switch_check {
            return;
        }
        self.next_switch_check = now + SWITCH_CHECK_INTERVAL;
        let missing = self.host.overlay_switch_missing();
        if self.switch_missing == Some(false) && missing {
            log::warn!(
                "the overlay's debug switch has disappeared; an NVIDIA App update may have removed it"
            );
            self.window_requested = true;
        }
        self.switch_missing = Some(missing);
    }

    /// Readies the app to quit: a recording in progress is stopped and named
    /// now, and a delayed stop is sent now, so quitting never leaves
    /// ShadowPlay recording. Blocks until the file is named.
    pub fn quit(&mut self) {
        if self.session.phase() != Phase::Idle {
            log::info!("quitting: saving what is in progress");
        }
        self.session.end_now();
    }

    pub fn step(&mut self, now: Duration, wall: SystemTime) -> Status {
        self.take_up_folder();
        self.check_switch(now);
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

        let was_starting = self.session.phase() == Phase::Starting;
        if self.session.is_running() {
            self.read_console_log(now, wall);
        }
        // A recordable map has just loaded: make room before the start
        // command is sent.
        if !was_starting && self.session.phase() == Phase::Starting {
            self.next_reserve_check = now;
        }
        self.keep_reserve(now);
        self.session.tick(now);
        // A match may have just ended.
        self.take_up_folder();

        if self.cleanup_at.is_some_and(|at| now >= at) {
            self.cleanup_at = None;
            if let Some(folder) = &self.folder
                && self.settings.retention_rule == RetentionRule::SizeCap
            {
                let cap = retention::gib_to_bytes(self.settings.max_size_gb);
                retention::enforce_size_cap(Path::new(folder), cap);
            }
        }
        self.status()
    }

    /// Under fill the disk, deletes the oldest recordings until the
    /// free-space reserve is free, once a minute or when asked for sooner.
    fn keep_reserve(&mut self, now: Duration) {
        if self.settings.retention_rule != RetentionRule::FillDisk {
            self.storage = None;
            self.warned_disk_unreadable = false;
            return;
        }
        if now < self.next_reserve_check {
            return;
        }
        self.next_reserve_check = now + RESERVE_CHECK_INTERVAL;
        let Some(folder) = self.folder.clone() else {
            self.storage = None;
            return;
        };
        let folder_path = Path::new(&folder);
        let disk = disk_name(&folder);
        let Some(space) = self.host.disk_space(folder_path) else {
            if !self.warned_disk_unreadable {
                self.warned_disk_unreadable = true;
                log::warn!(
                    "could not read the free space on {disk}; no recordings are deleted until it can be read"
                );
            }
            self.storage = None;
            return;
        };
        if self.warned_disk_unreadable {
            self.warned_disk_unreadable = false;
            log::info!("the free space on {disk} can be read again");
        }

        let manual = self.settings.manual_reserve_bytes(space.total);
        let reserve = manual.unwrap_or_else(|| retention::automatic_reserve(folder_path));
        let outcome = retention::keep_reserve(
            folder_path,
            space.free,
            reserve,
            self.session.recording_since(),
        );
        let short = outcome == ReserveOutcome::Short;
        let was_short = self.storage.as_ref().is_some_and(|s| s.short);
        if short && !was_short {
            log::warn!(
                "{disk} is short of its {} GB free-space reserve, and deleting every recording would not free it; the next recording may not fit",
                reserve.div_ceil(GIB)
            );
        } else if !short && was_short {
            log::info!("{disk} has its free-space reserve again");
        }
        let free = if space.free < reserve && !short {
            // Deleted something: show the space it freed.
            self.host
                .disk_space(folder_path)
                .map_or(space.free, |s| s.free)
        } else {
            space.free
        };
        self.storage = Some(Storage {
            disk,
            free_gb: whole_gb(free),
            total_gb: whole_gb(space.total),
            reserve_gb: u32::try_from(reserve.div_ceil(GIB)).unwrap_or(u32::MAX),
            automatic: manual.is_none(),
            short,
        });
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
            if self.folder.is_none() {
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

    /// A disk of `total` bytes holding `other` bytes besides the recordings
    /// folder, or `None` for a disk that cannot be read.
    type FakeDisk = Rc<Cell<Option<(u64, u64)>>>;

    struct FakeHost {
        running: Rc<Cell<bool>>,
        switch_missing: Rc<Cell<bool>>,
        disk: FakeDisk,
    }

    impl Host for FakeHost {
        fn is_game_running(&self, _game: &GameProfile) -> bool {
            self.running.get()
        }

        fn overlay_switch_missing(&self) -> bool {
            self.switch_missing.get()
        }

        fn steam_root(&self) -> Option<PathBuf> {
            None
        }

        /// The free space follows what the recordings folder holds, so
        /// deleting a recording frees its size.
        fn disk_space(&self, folder: &Path) -> Option<DiskSpace> {
            let (total, other) = self.disk.get()?;
            let recordings: u64 = fs::read_dir(folder)
                .unwrap()
                .map(|e| e.unwrap().metadata().unwrap().len())
                .sum();
            Some(DiskSpace {
                free: total.saturating_sub(other + recordings),
                total,
            })
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
        switch_missing: Rc<Cell<bool>>,
        disk: FakeDisk,
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
            let switch_missing = Rc::new(Cell::new(false));
            let disk = Rc::new(Cell::new(Some((1000 * GIB, 0))));

            let channel = FakeChannel::new(Some(&recordings), Mode::Ok);
            let host = FakeHost {
                running: running.clone(),
                switch_missing: switch_missing.clone(),
                disk: disk.clone(),
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
                switch_missing,
                disk,
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

        /// Writes a video of `gib` GB, last modified `age_hours` ago.
        fn video(&self, name: &str, gib: u64, age_hours: u64) -> PathBuf {
            let path = self.recordings.join(name);
            let file = File::create(&path).unwrap();
            file.set_len(gib * GIB).unwrap();
            file.set_modified(SystemTime::now() - Duration::from_secs(age_hours * 3600))
                .unwrap();
            path
        }

        /// Switches to fill the disk with a manual reserve of `reserve_gb`,
        /// on a 100 GB disk holding `other_gb` besides the recordings.
        fn fill_the_disk(&mut self, reserve_gb: f64, other_gb: u64) {
            self.disk.set(Some((100 * GIB, other_gb * GIB)));
            self.change_settings(|s| {
                s.retention_rule = RetentionRule::FillDisk;
                s.manual_reserve = true;
                s.reserve_gb = reserve_gb;
            });
        }

        /// Something other than a recording now takes `other_gb` of the disk.
        fn set_other_gb(&self, other_gb: u64) {
            self.disk.set(Some((100 * GIB, other_gb * GIB)));
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
    fn quitting_mid_match_stops_and_names_the_recording() {
        let mut f = Fixture::new(true);
        f.running.set(true);
        f.run(3);
        f.write_log("Lobby 5 for Match 7 created\n[Client] Map: \"start\"\n");
        assert_eq!(f.run(2).phase, Phase::Recording);

        f.engine.quit();
        assert_eq!(f.sent(), [RecordCommand::Start, RecordCommand::Stop]);
        assert!(f.recordings.join("7.mp4").exists());
    }

    #[test]
    fn quitting_during_the_stop_delay_sends_the_stop_now() {
        let mut f = Fixture::new(true);
        f.change_settings(|s| s.stop_delay_seconds = 30);
        f.running.set(true);
        f.run(3);
        f.write_log("Lobby 5 for Match 7 created\n[Client] Map: \"start\"\n");
        f.run(2);
        f.write_log("Lobby 5 for Match 7 destroyed\n");
        assert_eq!(f.run(1).phase, Phase::Finishing);

        f.engine.quit();
        assert_eq!(f.sent(), [RecordCommand::Start, RecordCommand::Stop]);
        assert!(f.recordings.join("7.mp4").exists());
    }

    #[test]
    fn quitting_out_of_a_match_sends_nothing() {
        let mut f = Fixture::new(true);
        f.running.set(true);
        assert_eq!(f.run(3).phase, Phase::GameRunning);
        f.engine.quit();
        assert!(f.sent().is_empty());
    }

    #[test]
    fn a_folder_change_mid_match_waits_for_the_match_to_end() {
        let mut f = Fixture::new(true);
        f.running.set(true);
        f.run(3);
        f.write_log("Lobby 5 for Match 1 created\n[Client] Map: \"start\"\n");
        assert_eq!(f.run(2).phase, Phase::Recording);

        // The player picks another folder while ShadowPlay still records
        // into the old one.
        let moved = f.recordings.parent().unwrap().join("moved");
        fs::create_dir(&moved).unwrap();
        f.change_settings(|s| s.recordings_folder = Some(moved.to_string_lossy().into()));
        f.write_log("Lobby 5 for Match 1 destroyed\n");
        assert_eq!(f.run(1).phase, Phase::GameRunning);
        assert_eq!(f.sent(), [RecordCommand::Start, RecordCommand::Stop]);
        assert!(
            f.recordings.join("1.mp4").exists(),
            "named in the old folder"
        );

        // The next match uses the new folder.
        f.engine.channel_mut().folder = Some(moved.clone());
        f.write_log("Lobby 6 for Match 2 created\n[Client] Map: \"start\"\n");
        assert_eq!(f.run(2).phase, Phase::Recording);
        f.write_log("Lobby 6 for Match 2 destroyed\n");
        f.run(1);
        assert!(moved.join("2.mp4").exists());
    }

    #[test]
    fn asks_for_the_window_once_when_the_debug_switch_disappears() {
        let mut f = Fixture::new(true);
        f.run(SWITCH_CHECK_INTERVAL.as_secs() + 1);
        assert!(!f.engine.take_window_request(), "the switch is there");

        // An NVIDIA App update removes the switch while the app runs.
        f.switch_missing.set(true);
        f.run(SWITCH_CHECK_INTERVAL.as_secs());
        assert!(f.engine.take_window_request());
        assert!(!f.engine.take_window_request(), "asked once");
        f.run(SWITCH_CHECK_INTERVAL.as_secs() * 2);
        assert!(
            !f.engine.take_window_request(),
            "still missing, not asked again"
        );

        // Fixed, then removed again by the next update.
        f.switch_missing.set(false);
        f.run(SWITCH_CHECK_INTERVAL.as_secs());
        f.switch_missing.set(true);
        f.run(SWITCH_CHECK_INTERVAL.as_secs());
        assert!(f.engine.take_window_request());
    }

    #[test]
    fn a_switch_missing_at_startup_is_left_to_the_startup_check() {
        let mut f = Fixture::new(true);
        f.switch_missing.set(true);
        f.run(SWITCH_CHECK_INTERVAL.as_secs() * 2);
        assert!(!f.engine.take_window_request());
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
    fn fill_the_disk_keeps_the_reserve_free_every_minute() {
        let mut f = Fixture::new(true);
        let old = f.video("1.mp4", 1, 3);
        let mid = f.video("2.mp4", 1, 2);
        f.fill_the_disk(2.0, 95);
        let storage = f.run(1).storage.unwrap();
        assert_eq!(
            (storage.free_gb, storage.total_gb, storage.reserve_gb),
            (3, 100, 2)
        );
        assert!(!storage.automatic);
        assert!(old.exists() && mid.exists(), "3 GB free covers the reserve");

        // A Steam download takes 2 GB more between checks.
        f.set_other_gb(97);
        f.run(RESERVE_CHECK_INTERVAL.as_secs() - 1);
        assert!(old.exists(), "not checked again before the minute is up");
        let storage = f.run(1).storage.unwrap();
        assert!(!old.exists());
        assert!(mid.exists());
        assert_eq!(storage.free_gb, 2);
        assert!(!storage.short);
    }

    #[test]
    fn fill_the_disk_makes_room_as_a_recordable_map_loads() {
        let mut f = Fixture::new(true);
        let old = f.video("1.mp4", 1, 3);
        f.fill_the_disk(2.0, 90);
        f.run(1);
        f.running.set(true);
        f.run(3);

        f.set_other_gb(98);
        f.write_log("Lobby 5 for Match 9 created\n[Client] Map: \"start\"\n");
        assert_eq!(f.run(2).phase, Phase::Recording);
        assert!(!old.exists(), "deleted well before the minute's check");
        assert!(f.recordings.join("DefaultShadowPlay.mp4").exists());
    }

    #[test]
    fn fill_the_disk_never_deletes_the_recording_in_progress() {
        let mut f = Fixture::new(true);
        let old = f.video("1.mp4", 1, 3);
        f.fill_the_disk(4.0, 50);
        f.running.set(true);
        f.run(3);
        f.write_log("Lobby 5 for Match 9 created\n[Client] Map: \"start\"\n");
        assert_eq!(f.run(2).phase, Phase::Recording);
        // The recording grows to 3 GB while the disk fills up.
        let in_progress = f.recordings.join("DefaultShadowPlay.mp4");
        File::options()
            .write(true)
            .open(&in_progress)
            .unwrap()
            .set_len(3 * GIB)
            .unwrap();
        f.set_other_gb(94);

        // 2 GB free; deleting the old recording would only make it 3 GB.
        let storage = f.run(RESERVE_CHECK_INTERVAL.as_secs()).storage.unwrap();
        assert!(storage.short);
        assert!(
            old.exists(),
            "nothing is deleted when it would not be enough"
        );
        assert!(in_progress.exists());

        // The download is cleaned up; the reserve can be kept again.
        f.set_other_gb(93);
        let storage = f.run(RESERVE_CHECK_INTERVAL.as_secs()).storage.unwrap();
        assert!(!storage.short);
        assert!(!old.exists());
        assert!(in_progress.exists());
    }

    #[test]
    fn fill_the_disk_deletes_nothing_when_the_disk_cannot_be_read() {
        let mut f = Fixture::new(true);
        let old = f.video("1.mp4", 1, 3);
        f.fill_the_disk(2.0, 99);
        f.disk.set(None);
        assert_eq!(f.run(1).storage, None);
        assert!(old.exists());

        f.set_other_gb(98);
        f.run(RESERVE_CHECK_INTERVAL.as_secs());
        assert!(!old.exists(), "checked again once the disk can be read");
    }

    #[test]
    fn fill_the_disk_starts_with_the_automatic_reserve() {
        let mut f = Fixture::new(true);
        f.fill_the_disk(2.0, 0);
        f.change_settings(|s| s.manual_reserve = false);
        let storage = f.run(1).storage.unwrap();
        assert!(storage.automatic);
        assert_eq!(storage.reserve_gb, 30);
    }

    #[test]
    fn fill_the_disk_replaces_the_size_cap() {
        let mut f = Fixture::new(true);
        let old = f.video("1.mp4", 1, 2);
        let new = f.video("2.mp4", 1, 1);
        f.fill_the_disk(2.0, 0);
        f.running.set(true);
        f.run(3);
        f.running.set(false);
        f.run(6 + CLEANUP_DELAY.as_secs());
        assert!(old.exists() && new.exists(), "2 GB is over the 1 GB cap");

        // Back on the size cap, the status no longer shows the disk.
        f.change_settings(|s| s.retention_rule = RetentionRule::SizeCap);
        assert_eq!(f.run(1).storage, None);
    }

    #[test]
    fn names_the_disk_of_a_folder() {
        assert_eq!(disk_name(r"d:\Videos\NVIDIA"), "D:");
        assert_eq!(disk_name(r"\\nas\videos\NVIDIA"), r"\\nas\videos");
        assert_eq!(disk_name("/Users/me"), "/Users/me");
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
