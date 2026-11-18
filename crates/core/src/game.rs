//! The one place that knows about a specific game.
//!
//! A [`GameProfile`] says how to find the game and how to read match events
//! out of its console log. Everything downstream works on [`GameEvent`]s, so
//! supporting another game means adding another profile here and nothing else.

use std::sync::LazyLock;

use regex::Regex;

/// Something the game's console log told us about a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    MatchStarted { match_id: String },
    MapLoaded { map: String },
    MatchEnded,
}

pub struct GameProfile {
    pub name: &'static str,
    /// Executable name, compared case-insensitively.
    pub process: &'static str,
    pub steam_app_id: &'static str,
    /// Folder name under `steamapps/common`.
    pub install_dir: &'static str,
    /// Console log path inside the install folder.
    pub console_log: &'static str,
    /// Steam launch option that makes the game write its console log.
    pub console_log_launch_option: &'static str,
    /// Maps that mean a real match. Bot, sandbox and custom maps are absent.
    pub recordable_maps: &'static [&'static str],
    pub parse_line: fn(&str) -> Option<GameEvent>,
}

impl GameProfile {
    pub fn is_recordable(&self, map: &str) -> bool {
        self.recordable_maps.contains(&map)
    }
}

pub const DEADLOCK: GameProfile = GameProfile {
    name: "Deadlock",
    process: "deadlock.exe",
    steam_app_id: "1422450",
    install_dir: "Deadlock",
    console_log: "game/citadel/console.log",
    console_log_launch_option: "-condebug",
    recordable_maps: &["start"],
    parse_line: parse_deadlock_line,
};

static MATCH_STARTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Lobby\s+\d+\s+for\s+Match\s+(\d+)\s+created").unwrap());
static MATCH_ENDED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Lobby\s+\d+\s+for\s+Match\s+\d+\s+destroyed").unwrap());
static MAP_LOADED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\[Client\]\s+Map:\s+"([^"]+)""#).unwrap());

fn parse_deadlock_line(line: &str) -> Option<GameEvent> {
    if let Some(c) = MATCH_STARTED.captures(line) {
        return Some(GameEvent::MatchStarted {
            match_id: c[1].to_string(),
        });
    }
    if MATCH_ENDED.is_match(line) {
        return Some(GameEvent::MatchEnded);
    }
    MAP_LOADED.captures(line).map(|c| GameEvent::MapLoaded {
        map: c[1].to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_match_events() {
        let parse = DEADLOCK.parse_line;
        assert_eq!(
            parse("Lobby 5 for Match 81038409 created"),
            Some(GameEvent::MatchStarted {
                match_id: "81038409".into()
            })
        );
        assert_eq!(
            parse("Lobby 5 for Match 81038409 destroyed"),
            Some(GameEvent::MatchEnded)
        );
        assert_eq!(
            parse(r#"[Client] Map: "start""#),
            Some(GameEvent::MapLoaded {
                map: "start".into()
            })
        );
        assert_eq!(parse("some unrelated line"), None);
        assert_eq!(parse("[Client] Players: 12 (0 bots) / 12 humans"), None);
    }

    #[test]
    fn only_listed_maps_are_recordable() {
        assert!(DEADLOCK.is_recordable("start"));
        assert!(!DEADLOCK.is_recordable("sandbox"));
    }
}
