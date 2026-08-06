//! The umu-database miss report (S9).
//!
//! Every game launched through umu with no database entry (`GAMEID=umu-0`,
//! `UMU_ID=umu-default`) is a gap in the shared umu-database — and this
//! daemon usually works out what the game actually was. This module stashes
//! those resolutions so the setup tool can show them for review and export a
//! submission in the database's own CSV shape
//! (https://github.com/Open-Wine-Components/umu-database:
//! `TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM,NOTE,EXE_STRINGS`).
//!
//! Persisted at `$XDG_DATA_HOME/gamebus-presenced/umu-misses.json` — data,
//! not cache: it accumulates across sessions and is the raw material for a
//! human-reviewed contribution. Written atomically on change, read-only
//! everywhere else. The daemon stays network-free; submitting is the user's
//! act, not ours.

// Compiled into both the daemon (which uses everything) and gamebus-setup
// (which reads only Miss/Confidence/default_path — the write half is the
// daemon's alone). Each binary alone reports the other's subset as dead;
// audited 2026-08-07, every item is live in at least one binary.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How much to trust a resolved title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Launcher's own install record, or a detectable.json hit on the real
    /// game process — exact titles from curated sources.
    High,
    /// A wrapper-layer identification: the Lutris title argv, a descendant
    /// walk, a sandbox-family match. Human-set, occasionally edited.
    Medium,
    /// An MPRIS hint or an executable stem — better than nothing, verify
    /// before submitting.
    Low,
}

/// One observed umu-database miss and what we made of it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Miss {
    /// Resolved display title, when anything resolved one.
    pub title: Option<String>,
    /// umu-database store id we believe this came from (`egs`, `gog`,
    /// `ubisoft`, … or `none`) — a guess, labelled as such.
    pub store: String,
    /// Store-internal codename when known (for EGS this is the App Name the
    /// database wants verbatim).
    pub codename: Option<String>,
    /// What umu reported (`umu-0` or `umu-default`) — the miss itself.
    pub umu_id: String,
    /// Where the title came from (`heroic-config`, `detectable`,
    /// `lutris-wrapper`, `mpris-hint`, `stem`).
    pub title_source: Option<String>,
    pub confidence: Option<Confidence>,
    /// The game executable, when identified — feeds `EXE_STRINGS`/`NOTE`.
    pub executable: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
}

/// The stash: keyed by `store:codename` (or the merge key when no codename
/// exists), loaded once, written through on change.
#[derive(Debug, Default)]
pub struct UmuReport {
    entries: HashMap<String, Miss>,
    path: Option<PathBuf>,
    dirty: bool,
}

impl UmuReport {
    /// Load from `$XDG_DATA_HOME/gamebus-presenced/umu-misses.json`.
    pub fn load() -> Self {
        let Some(path) = Self::default_path() else {
            return Self::default();
        };
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Self {
            entries,
            path: Some(path),
            dirty: false,
        }
    }

    pub fn default_path() -> Option<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(data.join("gamebus-presenced").join("umu-misses.json"))
    }

    /// A umu-miss launch was observed. Creates or refreshes the entry; the
    /// title may arrive later via [`note_title`].
    pub fn note_launch(
        &mut self,
        store: &str,
        codename: Option<&str>,
        umu_id: &str,
        fallback_key: &str,
    ) {
        let key = Self::entry_key(store, codename, fallback_key);
        let today = today();
        let entry = self.entries.entry(key).or_insert_with(|| Miss {
            title: None,
            store: store.to_string(),
            codename: codename.map(str::to_string),
            umu_id: umu_id.to_string(),
            title_source: None,
            confidence: None,
            executable: None,
            first_seen: today.clone(),
            last_seen: today.clone(),
        });
        entry.last_seen = today;
        self.dirty = true;
        self.persist();
    }

    /// A title (or a better title) resolved for a missed launch. Upgrades
    /// only: an existing higher-confidence resolution is never overwritten
    /// by a weaker one.
    #[allow(clippy::too_many_arguments)]
    pub fn note_title(
        &mut self,
        store: &str,
        codename: Option<&str>,
        fallback_key: &str,
        title: &str,
        source: &str,
        confidence: Confidence,
        executable: Option<&str>,
    ) {
        if title.is_empty() {
            return;
        }
        let key = Self::entry_key(store, codename, fallback_key);
        let Some(entry) = self.entries.get_mut(&key) else {
            return; // note_launch records the miss first; no launch, no entry.
        };
        let stronger = match (entry.confidence, confidence) {
            (None, _) => true,
            (Some(old), new) => rank(new) > rank(old),
        };
        let same_but_fresher =
            entry.confidence == Some(confidence) && entry.title.as_deref() != Some(title);
        if stronger || same_but_fresher {
            entry.title = Some(title.to_string());
            entry.title_source = Some(source.to_string());
            entry.confidence = Some(confidence);
            if let Some(exe) = executable {
                if !exe.is_empty() {
                    entry.executable = Some(exe.to_string());
                }
            }
            self.dirty = true;
            self.persist();
        }
    }

    fn entry_key(store: &str, codename: Option<&str>, fallback_key: &str) -> String {
        match codename {
            Some(code) if !code.is_empty() => format!("{store}:{code}"),
            _ => fallback_key.to_string(),
        }
    }

    fn persist(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = &self.path else { return };
        let Ok(json) = serde_json::to_string_pretty(&self.entries) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Atomic: a half-written stash must never eat accumulated knowledge.
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, path).is_ok() {
            self.dirty = false;
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn rank(c: Confidence) -> u8 {
    match c {
        Confidence::Low => 1,
        Confidence::Medium => 2,
        Confidence::High => 3,
    }
}

/// Today as `YYYY-MM-DD`, computed without a chrono dependency.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil-date conversion (days → y/m/d), Howard Hinnant's algorithm.
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Map launch evidence to a umu-database store id. `HEROIC_APP_SOURCE`
/// speaks for Heroic launches; for the rest, the install path sometimes
/// does. Anything unproven is `none` — the label is a guess and says so.
pub fn guess_store(heroic_source: Option<&str>, executable: &str) -> String {
    match heroic_source {
        Some("epic") => return "egs".to_string(),
        Some("gog") => return "gog".to_string(),
        Some("amazon" | "nile") => return "amazon".to_string(),
        _ => {}
    }
    let exe = executable.to_lowercase();
    if exe.contains("ubisoft") {
        "ubisoft".to_string()
    } else if exe.contains("gog galaxy") || exe.contains("gog games") {
        "gog".to_string()
    } else {
        "none".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> UmuReport {
        UmuReport::default() // no path: persist() no-ops, pure in-memory
    }

    #[test]
    fn launch_then_title_builds_a_submission_shaped_entry() {
        let mut r = report();
        r.note_launch("egs", Some("Calluna"), "umu-0", "heroic:Calluna");
        r.note_title(
            "egs",
            Some("Calluna"),
            "heroic:Calluna",
            "Control",
            "heroic-config",
            Confidence::High,
            Some("Control_DX12.exe"),
        );
        let e = r.entries.get("egs:Calluna").expect("entry exists");
        assert_eq!(e.title.as_deref(), Some("Control"));
        assert_eq!(e.store, "egs");
        assert_eq!(e.umu_id, "umu-0");
        assert_eq!(e.confidence, Some(Confidence::High));
        assert_eq!(e.executable.as_deref(), Some("Control_DX12.exe"));
    }

    #[test]
    fn weaker_titles_never_overwrite_stronger_ones() {
        let mut r = report();
        r.note_launch("none", None, "umu-default", "lutris:abc");
        r.note_title(
            "none",
            None,
            "lutris:abc",
            "Amnesia: The Bunker",
            "lutris-wrapper",
            Confidence::Medium,
            None,
        );
        r.note_title(
            "none",
            None,
            "lutris:abc",
            "amnesia",
            "stem",
            Confidence::Low,
            None,
        );
        let e = r.entries.get("lutris:abc").unwrap();
        assert_eq!(e.title.as_deref(), Some("Amnesia: The Bunker"));
        assert_eq!(e.confidence, Some(Confidence::Medium));
        // A stronger source upgrades.
        r.note_title(
            "none",
            None,
            "lutris:abc",
            "Amnesia: The Bunker",
            "detectable",
            Confidence::High,
            Some("AmnesiaTheBunker.exe"),
        );
        let e = r.entries.get("lutris:abc").unwrap();
        assert_eq!(e.confidence, Some(Confidence::High));
    }

    #[test]
    fn a_title_without_a_launch_records_nothing() {
        let mut r = report();
        r.note_title(
            "egs",
            Some("X"),
            "heroic:X",
            "Ghost",
            "stem",
            Confidence::Low,
            None,
        );
        assert!(r.entries.is_empty());
    }

    #[test]
    fn store_guessing_is_conservative() {
        assert_eq!(guess_store(Some("epic"), ""), "egs");
        assert_eq!(guess_store(Some("gog"), ""), "gog");
        assert_eq!(
            guess_store(
                None,
                "/games/ubisoft/drive_c/Program Files (x86)/Ubisoft/x.exe"
            ),
            "ubisoft"
        );
        assert_eq!(guess_store(None, "/games/somewhere/game.exe"), "none");
    }

    #[test]
    fn today_is_a_plausible_iso_date() {
        let d = today();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20"), "{d}");
    }
}
