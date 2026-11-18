//! Finds a game's console log through the Steam install, and reads whether
//! the game's launch options make it write that log.
//!
//! Steam's files are only ever read. Steam rewrites its config on exit, so
//! editing it from outside is unreliable; the app tells the player which
//! launch option to add instead.

use std::fs;
use std::path::{Path, PathBuf};

use crate::game::GameProfile;

/// A value in Valve's KeyValues text format.
#[derive(Debug, PartialEq, Eq)]
enum Vdf {
    Text(String),
    Block(Vec<(String, Vdf)>),
}

impl Vdf {
    /// Child by key, ignoring case: Steam is inconsistent about it.
    fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Block(entries) => entries
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Vdf::Text(_) => None,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Vdf::Text(t) => Some(t),
            Vdf::Block(_) => None,
        }
    }
}

/// Parses a KeyValues document into its top-level block.
fn parse_vdf(text: &str) -> Option<Vdf> {
    let mut chars = text.chars().peekable();
    let block = parse_block(&mut chars, false)?;
    Some(Vdf::Block(block))
}

fn parse_block(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    nested: bool,
) -> Option<Vec<(String, Vdf)>> {
    let mut entries = Vec::new();
    loop {
        match next_token(chars) {
            Some(Token::Text(key)) => match next_token(chars)? {
                Token::Text(value) => entries.push((key, Vdf::Text(value))),
                Token::Open => entries.push((key, Vdf::Block(parse_block(chars, true)?))),
                Token::Close => return None,
            },
            Some(Token::Close) if nested => return Some(entries),
            None if !nested => return Some(entries),
            _ => return None,
        }
    }
}

enum Token {
    Text(String),
    Open,
    Close,
}

fn next_token(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<Token> {
    while chars.next_if(|c| c.is_whitespace()).is_some() {}
    match chars.next()? {
        '{' => Some(Token::Open),
        '}' => Some(Token::Close),
        '"' => {
            let mut text = String::new();
            loop {
                match chars.next()? {
                    '"' => return Some(Token::Text(text)),
                    '\\' => text.push(chars.next()?),
                    c => text.push(c),
                }
            }
        }
        _ => None,
    }
}

/// Every Steam library folder, starting with the Steam install itself.
fn library_folders(steam_root: &Path) -> Vec<PathBuf> {
    let mut folders = vec![steam_root.to_path_buf()];
    let listed = fs::read_to_string(steam_root.join("steamapps").join("libraryfolders.vdf"))
        .ok()
        .and_then(|text| parse_vdf(&text));
    if let Some(Vdf::Block(libraries)) = listed.as_ref().and_then(|v| v.get("libraryfolders")) {
        for (_, library) in libraries {
            if let Some(path) = library.get("path").and_then(Vdf::text) {
                let path = PathBuf::from(path);
                if !folders.contains(&path) {
                    folders.push(path);
                }
            }
        }
    }
    folders
}

/// Where the game writes its console log, if the game is installed. The log
/// itself may not exist yet.
pub fn console_log_path(steam_root: &Path, game: &GameProfile) -> Option<PathBuf> {
    library_folders(steam_root).into_iter().find_map(|library| {
        let steamapps = library.join("steamapps");
        let manifest = steamapps.join(format!("appmanifest_{}.acf", game.steam_app_id));
        manifest.is_file().then(|| {
            steamapps
                .join("common")
                .join(game.install_dir)
                .join(game.console_log)
        })
    })
}

/// Whether the game's Steam launch options include the console-log option.
/// `None` when Steam's per-user config could not be read. With several Steam
/// accounts on the machine, one that has the option is enough.
pub fn has_console_log_option(steam_root: &Path, game: &GameProfile) -> Option<bool> {
    let users = fs::read_dir(steam_root.join("userdata")).ok()?;
    let mut answer = None;
    for user in users.flatten() {
        let config = user.path().join("config").join("localconfig.vdf");
        let Some(vdf) = fs::read_to_string(config).ok().and_then(|t| parse_vdf(&t)) else {
            continue;
        };
        let options = ["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"]
            .iter()
            .try_fold(&vdf, |node, key| node.get(key))
            .and_then(|apps| apps.get(game.steam_app_id))
            .and_then(|app| app.get("LaunchOptions"))
            .and_then(Vdf::text)
            .unwrap_or_default();
        let has_option = options
            .split_whitespace()
            .any(|o| o.eq_ignore_ascii_case(game.console_log_launch_option));
        if has_option {
            return Some(true);
        }
        answer = Some(false);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::DEADLOCK;

    #[test]
    fn parses_nested_blocks_and_escapes() {
        let vdf = parse_vdf(
            r#"
            "root"
            {
                "path"		"D:\\SteamLibrary"
                "apps" { "1422450" "123" }
            }
            "#,
        )
        .unwrap();
        let root = vdf.get("ROOT").unwrap();
        assert_eq!(root.get("path").unwrap().text(), Some(r"D:\SteamLibrary"));
        assert_eq!(
            root.get("apps").unwrap().get("1422450").unwrap().text(),
            Some("123")
        );
        assert_eq!(parse_vdf(r#""a" { "b" "c" "#), None, "unclosed block");
        assert_eq!(parse_vdf(r#""a" }"#), None, "stray brace");
    }

    /// A Steam install whose second library holds Deadlock.
    fn steam_with_deadlock(dir: &Path) -> (PathBuf, PathBuf) {
        let steam = dir.join("Steam");
        let library = dir.join("Library");
        fs::create_dir_all(steam.join("steamapps")).unwrap();
        fs::create_dir_all(library.join("steamapps")).unwrap();
        let escaped = library.to_string_lossy().replace('\\', r"\\");
        fs::write(
            steam.join("steamapps").join("libraryfolders.vdf"),
            format!(
                r#""libraryfolders" {{ "0" {{ "path" "{}" }} "1" {{ "path" "{escaped}" }} }}"#,
                steam.to_string_lossy().replace('\\', r"\\")
            ),
        )
        .unwrap();
        fs::write(
            library.join("steamapps").join("appmanifest_1422450.acf"),
            "",
        )
        .unwrap();
        (steam, library)
    }

    #[test]
    fn finds_the_console_log_in_another_library() {
        let dir = tempfile::tempdir().unwrap();
        let (steam, library) = steam_with_deadlock(dir.path());
        assert_eq!(
            console_log_path(&steam, &DEADLOCK),
            Some(
                library
                    .join("steamapps")
                    .join("common")
                    .join("Deadlock")
                    .join("game/citadel/console.log")
            )
        );
        fs::remove_file(library.join("steamapps").join("appmanifest_1422450.acf")).unwrap();
        assert_eq!(console_log_path(&steam, &DEADLOCK), None);
    }

    fn write_launch_options(steam: &Path, user: &str, options: &str) {
        let config = steam.join("userdata").join(user).join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("localconfig.vdf"),
            format!(
                r#""UserLocalConfigStore" {{ "Software" {{ "valve" {{ "Steam" {{ "apps" {{
                    "730" {{ "LaunchOptions" "-novid" }}
                    "1422450" {{ "LaunchOptions" "{options}" }}
                }} }} }} }} }}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn reads_the_launch_options() {
        let dir = tempfile::tempdir().unwrap();
        let steam = dir.path();
        assert_eq!(has_console_log_option(steam, &DEADLOCK), None);

        write_launch_options(steam, "1", "-novid");
        assert_eq!(has_console_log_option(steam, &DEADLOCK), Some(false));

        write_launch_options(steam, "2", "-novid -CONDEBUG");
        assert_eq!(has_console_log_option(steam, &DEADLOCK), Some(true));
    }
}
