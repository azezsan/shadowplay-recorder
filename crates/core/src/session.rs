//! The match lifecycle: decides when to start and stop a recording.
//!
//! Driven entirely from outside, with no clock or I/O of its own:
//! `on_running` reports whether the game process exists, `on_event` feeds
//! parsed console-log events, and `tick` advances the timers. `now` is a
//! monotonic time; wall-clock times are only used to compare against file
//! modification times.
//!
//! Rules it enforces:
//! - Recording starts only when a recordable map loads inside a match.
//! - The start command is re-sent on an interval until a recording file
//!   appears or the start window runs out.
//! - Before each send the game is brought to the foreground, because
//!   ShadowPlay only records the game it considers active.
//! - The stop is delayed after the match ends to capture the post-match
//!   screen, and is never sent unless a recording exists.

use std::time::{Duration, SystemTime};

use crate::game::{GameEvent, GameProfile};
use crate::recorder::Recorder;

/// Consecutive missed process polls before the game counts as closed.
const MISSES_BEFORE_EXIT: u32 = 2;
/// Pause between focusing the game and sending the record command, so the
/// overlay notices which game is active.
pub const FOCUS_SETTLE: Duration = Duration::from_millis(1500);

/// The game's window, as far as the session cares.
pub trait GameWindow {
    /// Brings the game to the foreground. Best effort: returns whether it
    /// worked, and a refusal must not stall anything.
    fn focus(&mut self) -> bool;
    fn is_foreground(&self) -> bool;
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub focus_on_match_start: bool,
    /// Wait after the map loads before the first start command, so the
    /// fullscreen load settles.
    pub start_delay: Duration,
    /// Keep retrying the start for this long.
    pub start_window: Duration,
    /// Gap between start commands. Must exceed ShadowPlay's file-creation
    /// latency, or a retry would toggle off a recording that just started.
    pub start_attempt_interval: Duration,
    /// Keep recording this long after the match ends.
    pub stop_delay: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            focus_on_match_start: true,
            start_delay: Duration::from_secs(4),
            start_window: Duration::from_secs(120),
            start_attempt_interval: Duration::from_secs(15),
            stop_delay: Duration::from_secs(10),
        }
    }
}

/// What the session is doing, for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// The game is not running.
    Idle,
    /// The game is running, outside a match.
    GameRunning,
    /// In a match that is not being recorded (not a recordable map, or the
    /// recording never started).
    InMatch,
    /// Start commands are being sent; no recording file yet.
    Starting,
    /// A recording file exists for the current match.
    Recording,
    /// The match ended and the recording is kept running for the stop delay.
    Finishing,
}

struct StartAttempt {
    deadline: Duration,
    next_send_at: Duration,
    sends: u32,
    /// The current send was deferred once to let focus settle.
    focus_settling: bool,
}

struct PendingStop {
    at: Duration,
    match_id: String,
    since: SystemTime,
}

struct Match {
    id: String,
    started: SystemTime,
    /// A recordable map loaded, so a recording was asked for.
    entered: bool,
}

pub struct Session<R: Recorder, W: GameWindow> {
    pub config: SessionConfig,
    game: &'static GameProfile,
    recorder: R,
    window: W,
    running: bool,
    misses: u32,
    current: Option<Match>,
    /// A recording file exists for the current (or just-ended) match.
    recording: bool,
    start: Option<StartAttempt>,
    stop: Option<PendingStop>,
}

impl<R: Recorder, W: GameWindow> Session<R, W> {
    pub fn new(game: &'static GameProfile, config: SessionConfig, recorder: R, window: W) -> Self {
        Self {
            config,
            game,
            recorder,
            window,
            running: false,
            misses: 0,
            current: None,
            recording: false,
            start: None,
            stop: None,
        }
    }

    pub fn recorder(&self) -> &R {
        &self.recorder
    }

    pub fn recorder_mut(&mut self) -> &mut R {
        &mut self.recorder
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn match_id(&self) -> Option<&str> {
        self.current.as_ref().map(|m| m.id.as_str())
    }

    pub fn phase(&self) -> Phase {
        if !self.running {
            Phase::Idle
        } else if self.start.is_some() {
            Phase::Starting
        } else if self.stop.is_some() {
            Phase::Finishing
        } else if self.recording {
            Phase::Recording
        } else if self.current.is_some() {
            Phase::InMatch
        } else {
            Phase::GameRunning
        }
    }

    /// Reports a process poll. Returns true when the game has just closed.
    pub fn on_running(&mut self, running: bool) -> bool {
        if running {
            self.misses = 0;
            if !self.running {
                self.running = true;
                log::info!("{} is running", self.game.name);
            }
            return false;
        }
        if !self.running {
            return false;
        }
        self.misses += 1;
        if self.misses < MISSES_BEFORE_EXIT {
            return false;
        }
        self.running = false;
        self.misses = 0;
        log::info!("{} closed", self.game.name);
        self.game_exit();
        true
    }

    pub fn on_event(&mut self, event: GameEvent, now: Duration, wall: SystemTime) {
        match event {
            GameEvent::MatchStarted { match_id } => {
                if self.current.is_some() {
                    return;
                }
                log::info!("match {match_id} started");
                self.current = Some(Match {
                    id: match_id,
                    started: wall,
                    entered: false,
                });
                // A match was just found while the player is likely in
                // another window.
                if self.config.focus_on_match_start {
                    self.window.focus();
                }
            }
            GameEvent::MapLoaded { map } => {
                let Some(current) = &mut self.current else {
                    return;
                };
                if current.entered || !self.game.is_recordable(&map) {
                    return;
                }
                current.entered = true;
                log::info!("recordable map {map:?} loaded; starting the recording");
                self.begin_start(now);
            }
            GameEvent::MatchEnded => {
                let Some(ended) = self.current.take() else {
                    return;
                };
                log::info!("match {} ended", ended.id);
                self.cancel_start();
                if ended.entered {
                    self.begin_stop(ended, now);
                } else {
                    log::info!("the match was not on a recordable map; nothing to save");
                }
            }
        }
    }

    /// Advances the start retries and the delayed stop.
    pub fn tick(&mut self, now: Duration) {
        if self.stop.as_ref().is_some_and(|s| now >= s.at) {
            self.flush_stop();
        }

        let (Some(attempt), Some(current)) = (&mut self.start, &self.current) else {
            return;
        };

        if self.recorder.has_recording_since(current.started) {
            log::info!("recording confirmed after {} send(s)", attempt.sends);
            self.recording = true;
            self.start = None;
            return;
        }
        if now >= attempt.deadline {
            log::warn!(
                "no recording appeared within {:?} after {} send(s); this match is not recorded",
                self.config.start_window,
                attempt.sends
            );
            self.start = None;
            return;
        }
        if now < attempt.next_send_at {
            return;
        }

        // The player may have switched away since the last attempt. Focus the
        // game, defer this send once so the overlay notices, then send whether
        // or not the focus took.
        if self.config.focus_on_match_start
            && !attempt.focus_settling
            && !self.window.is_foreground()
        {
            log::info!("the game is not in the foreground; focusing it before the record command");
            self.window.focus();
            attempt.focus_settling = true;
            attempt.next_send_at = now + FOCUS_SETTLE;
            return;
        }

        attempt.focus_settling = false;
        self.recorder.start();
        attempt.sends += 1;
        attempt.next_send_at = now + self.config.start_attempt_interval;
        log::info!(
            "start command #{} sent; waiting for a recording file",
            attempt.sends
        );
    }

    fn begin_start(&mut self, now: Duration) {
        // Finish the previous match's delayed stop first, so a new recording
        // never starts on top of the old one.
        self.flush_stop();
        self.start = Some(StartAttempt {
            deadline: now + self.config.start_window,
            next_send_at: now + self.config.start_delay,
            sends: 0,
            focus_settling: false,
        });
    }

    fn cancel_start(&mut self) {
        if self.start.take().is_some() {
            log::info!("stopped retrying the start");
        }
    }

    fn begin_stop(&mut self, ended: Match, now: Duration) {
        self.stop = Some(PendingStop {
            at: now + self.config.stop_delay,
            match_id: ended.id,
            since: ended.started,
        });
        if self.config.stop_delay.is_zero() {
            self.flush_stop();
        } else {
            log::info!("stopping the recording in {:?}", self.config.stop_delay);
        }
    }

    fn flush_stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            self.stop_and_rename(&stop.match_id, stop.since);
        }
    }

    fn stop_and_rename(&mut self, match_id: &str, since: SystemTime) {
        let stopped = self.recorder.stop(self.recording, since);
        self.recording = false;
        // Nothing was recording, so there is no file to wait for.
        if !stopped {
            return;
        }
        if let Some(path) = self.recorder.rename(match_id, since) {
            log::info!(
                "recording saved as {:?}",
                path.file_name().unwrap_or_default()
            );
        }
    }

    fn game_exit(&mut self) {
        // The game is gone: stop retrying, and save now instead of waiting
        // out the rest of the stop delay.
        self.cancel_start();
        if self.stop.is_some() {
            self.flush_stop();
        } else if let Some(current) = self.current.take()
            && current.entered
        {
            self.stop_and_rename(&current.id, current.started);
        }
        self.current = None;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::Path;
    use std::rc::Rc;

    use super::*;
    use crate::game::DEADLOCK;
    use crate::overlay::RecordCommand;
    use crate::recorder::ShadowPlayRecorder;
    use crate::recorder::testing::{FakeChannel, Mode, fast_recorder};

    #[derive(Clone)]
    struct FakeWindow {
        focuses: Rc<Cell<u32>>,
        foreground: bool,
        focus_works: bool,
    }

    impl FakeWindow {
        fn in_front() -> Self {
            Self {
                focuses: Rc::default(),
                foreground: true,
                focus_works: true,
            }
        }

        fn behind() -> Self {
            Self {
                foreground: false,
                ..Self::in_front()
            }
        }
    }

    impl GameWindow for FakeWindow {
        fn focus(&mut self) -> bool {
            self.focuses.set(self.focuses.get() + 1);
            self.focus_works
        }

        fn is_foreground(&self) -> bool {
            self.foreground
        }
    }

    type TestSession = Session<ShadowPlayRecorder<FakeChannel>, FakeWindow>;

    fn config() -> SessionConfig {
        SessionConfig {
            focus_on_match_start: true,
            start_delay: Duration::ZERO,
            start_window: Duration::from_secs(120),
            start_attempt_interval: Duration::from_secs(5),
            stop_delay: Duration::ZERO,
        }
    }

    fn session(
        folder: &Path,
        channel: FakeChannel,
        config: SessionConfig,
        window: FakeWindow,
    ) -> TestSession {
        Session::new(&DEADLOCK, config, fast_recorder(folder, channel), window)
    }

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    fn started(id: &str) -> GameEvent {
        GameEvent::MatchStarted {
            match_id: id.into(),
        }
    }

    fn map(name: &str) -> GameEvent {
        GameEvent::MapLoaded { map: name.into() }
    }

    fn sent(s: &TestSession) -> &[RecordCommand] {
        &s.recorder().channel().sent
    }

    /// Runs a match up to a confirmed recording.
    fn record(s: &mut TestSession, id: &str) {
        let wall = SystemTime::now();
        s.on_running(true);
        s.on_event(started(id), secs(100.0), wall);
        s.on_event(map("start"), secs(100.0), wall);
        s.tick(secs(100.0));
        s.tick(secs(101.0));
        assert_eq!(s.phase(), Phase::Recording);
    }

    #[test]
    fn records_a_real_match() {
        let dir = tempfile::tempdir().unwrap();
        let channel = FakeChannel::new(Some(dir.path()), Mode::Ok);
        let mut s = session(dir.path(), channel, config(), FakeWindow::in_front());
        let wall = SystemTime::now();

        s.on_running(true);
        assert_eq!(s.phase(), Phase::GameRunning);
        s.on_event(started("81038409"), secs(100.0), wall);
        assert_eq!(s.phase(), Phase::InMatch, "waiting for the map");
        s.on_event(map("start"), secs(100.0), wall);
        assert_eq!(s.phase(), Phase::Starting);
        assert!(sent(&s).is_empty(), "nothing is sent until a tick");

        s.tick(secs(100.0));
        assert_eq!(sent(&s), [RecordCommand::Start]);
        s.tick(secs(101.0));
        assert_eq!(
            s.phase(),
            Phase::Recording,
            "confirmed once the file appears"
        );

        s.on_event(GameEvent::MatchEnded, secs(102.0), wall);
        assert_eq!(sent(&s).last(), Some(&RecordCommand::Stop));
        assert_eq!(s.phase(), Phase::GameRunning);
        assert!(dir.path().join("81038409.mp4").exists());
    }

    #[test]
    fn retries_the_start_until_a_recording_appears() {
        let dir = tempfile::tempdir().unwrap();
        let mut channel = FakeChannel::new(Some(dir.path()), Mode::Ok);
        channel.fail_first = 2;
        let mut s = session(dir.path(), channel, config(), FakeWindow::in_front());
        let wall = SystemTime::now();

        s.on_running(true);
        s.on_event(started("4242"), secs(100.0), wall);
        s.on_event(map("start"), secs(100.0), wall);
        s.tick(secs(100.0));
        assert_eq!(sent(&s).len(), 1);
        s.tick(secs(101.0));
        assert_eq!(sent(&s).len(), 1, "no send before the interval elapses");
        s.tick(secs(105.0));
        assert_eq!(sent(&s).len(), 2);
        assert_eq!(s.phase(), Phase::Starting);
        s.tick(secs(110.0));
        assert_eq!(sent(&s).len(), 3);
        s.tick(secs(111.0));
        assert_eq!(s.phase(), Phase::Recording);

        s.on_event(GameEvent::MatchEnded, secs(112.0), wall);
        assert!(dir.path().join("4242.mp4").exists());
    }

    #[test]
    fn gives_up_after_the_start_window_and_never_stops_blind() {
        let dir = tempfile::tempdir().unwrap();
        let channel = FakeChannel::new(None, Mode::Fail);
        let config = SessionConfig {
            start_window: Duration::from_secs(10),
            ..config()
        };
        let mut s = session(dir.path(), channel, config, FakeWindow::in_front());
        let wall = SystemTime::now();

        s.on_running(true);
        s.on_event(started("8"), secs(100.0), wall);
        s.on_event(map("start"), secs(100.0), wall);
        s.tick(secs(100.0));
        s.tick(secs(105.0));
        s.tick(secs(110.0));
        assert_eq!(s.phase(), Phase::InMatch, "gave up, still in the match");

        s.on_event(GameEvent::MatchEnded, secs(111.0), wall);
        assert_eq!(s.recorder().channel().stops(), 0);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn skips_a_map_that_is_not_recordable() {
        let dir = tempfile::tempdir().unwrap();
        let channel = FakeChannel::new(Some(dir.path()), Mode::Ok);
        let mut s = session(dir.path(), channel, config(), FakeWindow::in_front());
        let wall = SystemTime::now();

        s.on_running(true);
        s.on_event(started("999"), secs(100.0), wall);
        s.on_event(map("sandbox"), secs(100.0), wall);
        s.tick(secs(100.0));
        assert_eq!(s.phase(), Phase::InMatch);
        s.on_event(GameEvent::MatchEnded, secs(101.0), wall);
        assert!(sent(&s).is_empty());
    }

    #[test]
    fn ignores_a_map_load_outside_a_match() {
        let dir = tempfile::tempdir().unwrap();
        let channel = FakeChannel::new(Some(dir.path()), Mode::Ok);
        let mut s = session(dir.path(), channel, config(), FakeWindow::in_front());
        s.on_running(true);
        s.on_event(map("start"), secs(100.0), SystemTime::now());
        s.tick(secs(100.0));
        assert_eq!(s.phase(), Phase::GameRunning);
        assert!(sent(&s).is_empty());
    }

    #[test]
    fn saves_the_recording_when_the_game_closes_mid_match() {
        let dir = tempfile::tempdir().unwrap();
        let channel = FakeChannel::new(Some(dir.path()), Mode::Ok);
        let mut s = session(dir.path(), channel, config(), FakeWindow::in_front());
        record(&mut s, "555");

        assert!(!s.on_running(false), "one missed poll is not an exit");
        assert!(s.is_running());
        assert!(s.on_running(false), "two missed polls are");
        assert_eq!(s.phase(), Phase::Idle);
        assert_eq!(s.recorder().channel().stops(), 1);
        assert!(dir.path().join("555.mp4").exists());
    }

    #[test]
    fn delays_the_stop_after_the_match_ends() {
        let dir = tempfile::tempdir().unwrap();
        let channel = FakeChannel::new(Some(dir.path()), Mode::Ok);
        let config = SessionConfig {
            stop_delay: Duration::from_secs(10),
            ..config()
        };
        let mut s = session(dir.path(), channel, config, FakeWindow::in_front());
        record(&mut s, "77");

        s.on_event(GameEvent::MatchEnded, secs(102.0), SystemTime::now());
        assert_eq!(s.phase(), Phase::Finishing);
        s.tick(secs(111.0));
        assert_eq!(s.recorder().channel().stops(), 0, "still inside the delay");
        s.tick(secs(112.0));
        assert_eq!(s.recorder().channel().stops(), 1);
        assert!(dir.path().join("77.mp4").exists());
    }

    #[test]
    fn closing_the_game_flushes_a_delayed_stop() {
        let dir = tempfile::tempdir().unwrap();
        let channel = FakeChannel::new(Some(dir.path()), Mode::Ok);
        let config = SessionConfig {
            stop_delay: Duration::from_secs(10),
            ..config()
        };
        let mut s = session(dir.path(), channel, config, FakeWindow::in_front());
        record(&mut s, "88");

        s.on_event(GameEvent::MatchEnded, secs(102.0), SystemTime::now());
        s.on_running(false);
        s.on_running(false);
        assert_eq!(s.recorder().channel().stops(), 1);
        assert!(dir.path().join("88.mp4").exists());
    }

    #[test]
    fn a_new_match_flushes_the_previous_delayed_stop() {
        let dir = tempfile::tempdir().unwrap();
        let channel = FakeChannel::new(Some(dir.path()), Mode::Ok);
        let config = SessionConfig {
            stop_delay: Duration::from_secs(60),
            ..config()
        };
        let mut s = session(dir.path(), channel, config, FakeWindow::in_front());
        record(&mut s, "1");
        let wall = SystemTime::now();

        s.on_event(GameEvent::MatchEnded, secs(102.0), wall);
        s.on_event(started("2"), secs(103.0), wall);
        s.on_event(map("start"), secs(104.0), wall);
        assert_eq!(s.recorder().channel().stops(), 1);
        assert!(dir.path().join("1.mp4").exists());
        assert_eq!(s.phase(), Phase::Starting);
    }

    #[test]
    fn a_start_without_a_reply_is_confirmed_by_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let channel = FakeChannel::new(Some(dir.path()), Mode::NoAck);
        let mut s = session(dir.path(), channel, config(), FakeWindow::in_front());
        record(&mut s, "9");
        assert_eq!(sent(&s), [RecordCommand::Start]);
    }

    #[test]
    fn focuses_the_game_once_when_a_match_starts() {
        let dir = tempfile::tempdir().unwrap();
        let window = FakeWindow::in_front();
        let focuses = window.focuses.clone();
        let channel = FakeChannel::new(Some(dir.path()), Mode::Ok);
        let mut s = session(dir.path(), channel, config(), window);
        let wall = SystemTime::now();

        s.on_running(true);
        s.on_event(started("31337"), secs(100.0), wall);
        s.on_event(started("31337"), secs(101.0), wall);
        s.on_event(map("start"), secs(102.0), wall);
        s.tick(secs(102.0));
        s.on_event(GameEvent::MatchEnded, secs(103.0), wall);
        assert_eq!(focuses.get(), 1);
    }

    #[test]
    fn never_focuses_when_the_setting_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let window = FakeWindow::behind();
        let focuses = window.focuses.clone();
        let config = SessionConfig {
            focus_on_match_start: false,
            ..config()
        };
        let channel = FakeChannel::new(None, Mode::Fail);
        let mut s = session(dir.path(), channel, config, window);
        let wall = SystemTime::now();

        s.on_running(true);
        s.on_event(started("1"), secs(100.0), wall);
        s.on_event(map("start"), secs(100.0), wall);
        s.tick(secs(100.0));
        assert_eq!(focuses.get(), 0);
        assert_eq!(sent(&s).len(), 1);
    }

    #[test]
    fn refocuses_before_each_start_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let window = FakeWindow::behind();
        let focuses = window.focuses.clone();
        let config = SessionConfig {
            start_window: Duration::from_secs(60),
            start_attempt_interval: Duration::from_secs(15),
            ..config()
        };
        let channel = FakeChannel::new(None, Mode::Fail);
        let mut s = session(dir.path(), channel, config, window);
        let wall = SystemTime::now();
        let settle = FOCUS_SETTLE.as_secs_f64();

        s.on_running(true);
        s.on_event(started("6161"), secs(100.0), wall);
        assert_eq!(focuses.get(), 1);
        s.on_event(map("start"), secs(100.0), wall);
        s.tick(secs(100.0));
        assert_eq!(
            (focuses.get(), sent(&s).len()),
            (2, 0),
            "focus, not a blind send"
        );
        s.tick(secs(100.0 + settle));
        assert_eq!(
            (focuses.get(), sent(&s).len()),
            (2, 1),
            "sent once focus settled"
        );
        s.tick(secs(100.0 + settle + 15.0));
        assert_eq!(
            (focuses.get(), sent(&s).len()),
            (3, 1),
            "each retry refocuses"
        );
        s.tick(secs(100.0 + settle * 2.0 + 15.0));
        assert_eq!(sent(&s).len(), 2);
    }

    #[test]
    fn a_refused_focus_does_not_stall_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let window = FakeWindow {
            focus_works: false,
            ..FakeWindow::behind()
        };
        let channel = FakeChannel::new(None, Mode::Fail);
        let mut s = session(dir.path(), channel, config(), window);
        let wall = SystemTime::now();

        s.on_running(true);
        s.on_event(started("6162"), secs(100.0), wall);
        s.on_event(map("start"), secs(100.0), wall);
        s.tick(secs(100.0));
        s.tick(secs(100.0) + FOCUS_SETTLE);
        assert_eq!(sent(&s).len(), 1);
    }
}
