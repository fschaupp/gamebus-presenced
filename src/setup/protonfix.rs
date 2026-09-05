//! Game identity recovered from a trace's own game log.
//!
//! The third and last way to key a compat finding, after the appid beisl
//! reads off `org.gamebus.Presence.v1` and before giving up.
//!
//! ProtonFixes announces which game Proton thinks it is launching, with the
//! title and the Steam appid together:
//!
//! ```text
//! Using early stage global defaults for "WARDOGS Playtest" (4809930)
//! ```
//!
//! That line is Proton's own resolution of the launch, so it is stronger
//! evidence than matching an executable basename against a naming database -
//! and it is present in runs that nothing else can key. On this machine it
//! appears in 10 of 12 traces, including all five analyzed WARDOGS runs,
//! whose launcher executable and playtest appid are in no naming database at
//! all. Without this rung the first payload we want to file cannot be keyed.
//!
//! Read from the run directory this side already has a pointer to, for the
//! same reason machine specs are (see [`super::specs`]): gamebus-setup runs
//! on the same machine, so it reads the file itself rather than asking beisl
//! to widen an interface. beisl deliberately does not treat this as an
//! identity source of its own - presence is its sanctioned path - and this
//! rung does not ask it to.

#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::Path;

/// How many lines to read before giving up. The line lands in the launch
/// preamble - line 16 of 22 and 16 of 28 in the traces here - and a game log
/// can be long, so this never walks a whole file.
const MAX_LINES: usize = 400;

/// A title and appid ProtonFixes named together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub title: String,
    pub steam_appid: String,
}

/// Pull the identity out of one log line, if it is one of ProtonFixes'
/// announcements.
///
/// Matches both stages (`early stage`, `main stage`) and both outcomes
/// (`defaults`, `protonfix`), which are the four forms these logs carry.
/// Deliberately anchored on the quoted title followed by a parenthesised
/// all-digit appid: a looser match would happily read a title out of an
/// unrelated line, and a wrong appid keys a finding onto another game.
pub fn parse_line(line: &str) -> Option<Identity> {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("using ") {
        return None;
    }
    if !(lower.contains(" defaults for ") || lower.contains(" protonfix for ")) {
        return None;
    }
    // ... for "TITLE" (APPID)
    let (_, rest) = line.split_once('"')?;
    let (title, rest) = rest.split_once('"')?;
    let rest = rest.trim_start();
    let appid = rest.strip_prefix('(')?.split_once(')')?.0;
    if title.is_empty() || appid.is_empty() || !appid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Identity {
        title: title.to_string(),
        steam_appid: appid.to_string(),
    })
}

/// Read the identity out of a run directory's `game.log`.
pub fn read_from_dir(dir: &Path) -> Option<Identity> {
    let file = std::fs::File::open(dir.join("game.log")).ok()?;
    BufReader::new(file)
        .lines()
        .take(MAX_LINES)
        .map_while(Result::ok)
        .find_map(|line| parse_line(&line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_these_logs_carry_parses() {
        // All four seen across this machine's traces.
        for line in [
            r#"Using early stage global defaults for "WARDOGS Playtest" (4809930)"#,
            r#"Using main stage global defaults for "WARDOGS Playtest" (4809930)"#,
            r#"Using early stage global protonfix for "Assetto Corsa" (244210)"#,
            r#"Using main stage global protonfix for "Assetto Corsa" (244210)"#,
        ] {
            assert!(parse_line(line).is_some(), "{line}");
        }
    }

    #[test]
    fn the_title_and_appid_come_back_together() {
        let id = parse_line(
            r#"Using early stage global defaults for "WARDOGS Playtest" (4809930)"#,
        )
        .unwrap();
        assert_eq!(id.title, "WARDOGS Playtest");
        assert_eq!(id.steam_appid, "4809930");
    }

    #[test]
    fn a_title_with_punctuation_survives() {
        let id = parse_line(r#"Using main stage global defaults for "WarDogs: Red's Return" (1)"#)
            .unwrap();
        assert_eq!(id.title, "WarDogs: Red's Return");
    }

    #[test]
    fn an_unrelated_line_names_nothing() {
        for line in [
            "ERROR: LD_PRELOAD cannot be preloaded",
            r#"Something about "a quoted thing" (not an appid)"#,
            "Using the default renderer",
            r#"Using early stage global defaults for "" (4809930)"#,
        ] {
            assert!(parse_line(line).is_none(), "{line}");
        }
    }

    #[test]
    fn a_non_numeric_appid_is_refused_rather_than_keyed_on() {
        assert!(
            parse_line(r#"Using main stage global defaults for "Game" (umu-0)"#).is_none(),
            "a wrong appid keys a finding onto another game"
        );
    }

    #[test]
    fn a_directory_with_no_game_log_yields_nothing() {
        let dir = std::env::temp_dir().join(format!("pfix-none-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert!(read_from_dir(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_announcement_wins_and_the_whole_file_is_not_walked() {
        let dir = std::env::temp_dir().join(format!("pfix-big-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut body = String::from("noise\nmore noise\n");
        body.push_str(r#"Using early stage global defaults for "WARDOGS Playtest" (4809930)"#);
        body.push('\n');
        body.push_str(&"filler\n".repeat(10_000));
        std::fs::write(dir.join("game.log"), body).unwrap();
        let id = read_from_dir(&dir).expect("identity found");
        assert_eq!(id.steam_appid, "4809930");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
