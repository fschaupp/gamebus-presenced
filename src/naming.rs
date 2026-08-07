//! Naming enrichment: maps executables and Steam appids to human-readable
//! game names using Discord's `detectable.json` database.
//!
//! The database is never fetched at build time and never embedded in the
//! binary: `gamebus-presence fetch-detectable` downloads it (the setup tool
//! runs that as an install step), keeping builds network-free and
//! reproducible. At runtime, the daemon looks for it in this order:
//!
//! 1. `$XDG_CACHE_HOME/gamebus-presenced/detectable.json` (CLI-refreshed)
//! 2. `$XDG_DATA_HOME/gamebus-presenced/detectable.json` (user-installed)
//! 3. `/usr/share/gamebus-presenced/detectable.json` (system-installed)
//!
//! If no database is found, naming enrichment is silently disabled - the
//! daemon works without it ("no network, no naming, everything else still
//! works", design doc).

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// One entry in Discord's detectable database.
#[derive(Debug, Deserialize)]
struct DetectableEntry {
    name: String,
    #[serde(default)]
    executables: Vec<DetectableExecutable>,
    #[serde(default)]
    third_party_skus: Vec<DetectableSku>,
}

#[derive(Debug, Deserialize)]
struct DetectableExecutable {
    name: String,
}

#[derive(Debug, Deserialize)]
struct DetectableSku {
    distributor: String,
    id: Option<String>,
}

/// Naming database with pre-built lookup indexes.
pub struct NamingDb {
    /// executable basename (lowercase) → [(entry name, game name)].
    ///
    /// detectable.json entries come in two forms: plain (`eldenring.exe`)
    /// and path-prefixed (`amnesia the bunker/amnesiathebunker.exe`) - the
    /// prefixed form is the majority (~83% of entries). Both are bucketed by
    /// basename; path-prefixed entries additionally match by path suffix at
    /// lookup time (mirroring Discord's own scanner), which disambiguates
    /// basename collisions like `amnesia/amnesia.exe` vs
    /// `amnesia the dark descent/amnesia.exe`.
    by_executable: HashMap<String, Vec<(String, String)>>,
    /// steam appid → game name
    by_steam_appid: HashMap<String, String>,
    /// game name (lowercase) → steam appid - the reverse direction, used by
    /// gamebus-setup to draft umu ids from a resolved title. Dead in
    /// the daemon's copy of this shared module, live in gamebus-setup's.
    #[allow(dead_code)]
    appid_by_title: HashMap<String, String>,
}

impl NamingDb {
    /// Load the naming database from the first available path.
    /// Returns `None` if no database file is found.
    pub fn load() -> Option<Self> {
        let path = find_detectable_json()?;
        let data = std::fs::read_to_string(&path).ok()?;
        Self::parse(&data)
    }

    /// Parse from a JSON string (for testing).
    pub fn parse(json: &str) -> Option<Self> {
        let entries: Vec<DetectableEntry> = serde_json::from_str(json).ok()?;
        let mut by_executable: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut by_steam_appid = HashMap::new();
        let mut appid_by_title: HashMap<String, String> = HashMap::new();

        for entry in entries {
            for exe in &entry.executables {
                let full = exe.name.to_lowercase();
                let basename = full
                    .rsplit_once('/')
                    .map(|(_, b)| b.to_string())
                    .unwrap_or_else(|| full.clone());
                let bucket = by_executable.entry(basename).or_default();
                if !bucket.iter().any(|(e, _)| e == &full) {
                    bucket.push((full, entry.name.clone()));
                }
            }
            for sku in &entry.third_party_skus {
                if sku.distributor == "steam" {
                    if let Some(ref id) = sku.id {
                        by_steam_appid
                            .entry(id.clone())
                            .or_insert_with(|| entry.name.clone());
                        appid_by_title
                            .entry(entry.name.to_lowercase())
                            .or_insert_with(|| id.clone());
                    }
                }
            }
        }

        Some(Self {
            by_executable,
            by_steam_appid,
            appid_by_title,
        })
    }

    /// Look up a game name by executable path or filename.
    ///
    /// Matching order per basename bucket:
    /// 1. **Path-suffix match** - a path-prefixed entry whose full name is a
    ///    suffix of the lowercased input path
    ///    (`amnesia the bunker/amnesiathebunker.exe` matches
    ///    `.../Amnesia The Bunker/AmnesiaTheBunker.exe`). Most specific.
    /// 2. **Plain entry** - an entry with no path component (`eldenring.exe`).
    /// 3. **First entry in bucket** - deterministic fallback for ambiguous
    ///    basenames.
    pub fn lookup_by_executable(&self, executable: &str) -> Option<&str> {
        // Normalise Windows path separators: Wine process cmdlines carry
        // `S:\Spiele\game\game.exe`, detectable.json uses `/`.
        let lower = executable.replace('\\', "/").to_lowercase();
        let basename = lower.rsplit_once('/').map(|(_, b)| b).unwrap_or(&lower);
        let bucket = self.by_executable.get(basename)?;

        // 1. Path-suffix match (most specific).
        for (entry, name) in bucket {
            if entry.contains('/') && lower.ends_with(entry.as_str()) {
                return Some(name.as_str());
            }
        }
        // 2. Plain entry (no path component).
        for (entry, name) in bucket {
            if !entry.contains('/') {
                return Some(name.as_str());
            }
        }
        // 3. Deterministic fallback.
        bucket.first().map(|(_, name)| name.as_str())
    }

    /// Look up a game name by Steam appid.
    pub fn lookup_by_steam_appid(&self, appid: &str) -> Option<&str> {
        self.by_steam_appid.get(appid).map(|s| s.as_str())
    }

    /// The reverse: a Steam appid for an exact (case-insensitive) title.
    /// This is Discord's curated sku data - an offline source for the
    /// umu-database rule "on Steam → umu-<appid>".
    /// Dead in the daemon's copy, live in gamebus-setup's (see the field).
    #[allow(dead_code)]
    pub fn steam_appid_for_title(&self, title: &str) -> Option<&str> {
        self.appid_by_title
            .get(&title.to_lowercase())
            .map(|s| s.as_str())
    }

    /// Number of entries in the database.
    pub fn len(&self) -> usize {
        self.by_executable.len().max(self.by_steam_appid.len())
    }
}

/// System data directories, per the XDG base directory specification.
///
/// The default is `/usr/local/share:/usr/share`, so software installed by hand
/// under `/usr/local` - the FHS home for locally built software - is found
/// without colliding with the paths a distribution package owns.
pub(crate) fn xdg_data_dirs() -> Vec<PathBuf> {
    std::env::var_os("XDG_DATA_DIRS")
        .filter(|v| !v.is_empty())
        .map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .unwrap_or_else(|| {
            vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]
        })
}

/// Find the detectable.json file in the standard search paths.
///
/// `gamebus-setup` mirrors this order to report which copy is live - keep the
/// two in step (`src/setup/paths.rs`, `detectable_candidates`).
fn find_detectable_json() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    // CLI-refreshed cache (highest priority)
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    {
        candidates.push(cache.join("gamebus-presenced/detectable.json"));
    }

    // User data directory
    if let Some(data) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    {
        candidates.push(data.join("gamebus-presenced/detectable.json"));
    }

    // System data directories, in XDG order
    candidates.extend(
        xdg_data_dirs()
            .into_iter()
            .map(|d| d.join("gamebus-presenced/detectable.json")),
    );

    candidates.into_iter().find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"[
        {
            "name": "Elden Ring",
            "executables": [
                {"name": "eldenring.exe", "os": "win32", "is_launcher": false}
            ],
            "third_party_skus": [
                {"distributor": "steam", "id": "1245620"}
            ]
        },
        {
            "name": "Overwatch",
            "executables": [
                {"name": "overwatch.exe", "os": "win32", "is_launcher": false}
            ],
            "third_party_skus": [
                {"distributor": "steam", "id": "2357570"},
                {"distributor": "battlenet", "id": "Pro"}
            ]
        },
        {
            "name": "No Exe Game",
            "executables": [],
            "third_party_skus": []
        }
    ]"#;

    #[test]
    fn steam_appid_for_title_is_exact_and_case_insensitive() {
        let db = NamingDb::parse(SAMPLE_JSON).unwrap();
        assert_eq!(db.steam_appid_for_title("Elden Ring"), Some("1245620"));
        assert_eq!(db.steam_appid_for_title("elden ring"), Some("1245620"));
        assert_eq!(db.steam_appid_for_title("Overwatch"), Some("2357570"));
        // No sku, no appid - and no fuzzy matching.
        assert_eq!(db.steam_appid_for_title("No Exe Game"), None);
        assert_eq!(db.steam_appid_for_title("Elden"), None);
    }

    #[test]
    fn parse_and_lookup_by_executable() {
        let db = NamingDb::parse(SAMPLE_JSON).unwrap();
        assert_eq!(db.lookup_by_executable("eldenring.exe"), Some("Elden Ring"));
        assert_eq!(
            db.lookup_by_executable("/games/eldenring.exe"),
            Some("Elden Ring")
        );
        assert_eq!(db.lookup_by_executable("ELDENRING.EXE"), Some("Elden Ring"));
        assert_eq!(db.lookup_by_executable("overwatch.exe"), Some("Overwatch"));
        assert_eq!(db.lookup_by_executable("nonexistent.exe"), None);
    }

    #[test]
    fn parse_and_lookup_by_steam_appid() {
        let db = NamingDb::parse(SAMPLE_JSON).unwrap();
        assert_eq!(db.lookup_by_steam_appid("1245620"), Some("Elden Ring"));
        assert_eq!(db.lookup_by_steam_appid("2357570"), Some("Overwatch"));
        assert_eq!(db.lookup_by_steam_appid("999999"), None);
    }

    #[test]
    fn empty_entries_are_skipped() {
        let db = NamingDb::parse(SAMPLE_JSON).unwrap();
        assert_eq!(db.lookup_by_executable(""), None);
        assert_eq!(db.lookup_by_steam_appid(""), None);
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(NamingDb::parse("not json").is_none());
        assert!(NamingDb::parse("").is_none());
    }

    #[test]
    fn first_entry_wins_on_duplicate() {
        let json = r#"[
            {"name": "First", "executables": [{"name": "game.exe"}], "third_party_skus": []},
            {"name": "Second", "executables": [{"name": "game.exe"}], "third_party_skus": []}
        ]"#;
        let db = NamingDb::parse(json).unwrap();
        assert_eq!(db.lookup_by_executable("game.exe"), Some("First"));
    }

    #[test]
    fn path_prefixed_entry_matches_by_suffix() {
        // The Amnesia case: detectable.json stores the exe path-prefixed.
        let json = r#"[
            {"name": "Amnesia: The Bunker", "executables": [{"name": "amnesia the bunker/amnesiathebunker.exe"}], "third_party_skus": []}
        ]"#;
        let db = NamingDb::parse(json).unwrap();
        // Full path suffix match (case-insensitive).
        assert_eq!(
            db.lookup_by_executable(
                "/media/Data/Spiele/Amnesia The Bunker/Amnesia The Bunker/AmnesiaTheBunker.exe"
            ),
            Some("Amnesia: The Bunker")
        );
        // Exact entry name also matches.
        assert_eq!(
            db.lookup_by_executable("amnesia the bunker/amnesiathebunker.exe"),
            Some("Amnesia: The Bunker")
        );
    }

    #[test]
    fn basename_collision_resolved_by_suffix() {
        // Two games share the same exe basename; the path suffix picks right.
        let json = r#"[
            {"name": "Amnesia: Memories", "executables": [{"name": "amnesia/amnesia.exe"}], "third_party_skus": []},
            {"name": "Amnesia: The Dark Descent", "executables": [{"name": "amnesia the dark descent/amnesia.exe"}], "third_party_skus": []}
        ]"#;
        let db = NamingDb::parse(json).unwrap();
        assert_eq!(
            db.lookup_by_executable("/games/Amnesia The Dark Descent/Amnesia.exe"),
            Some("Amnesia: The Dark Descent")
        );
        // No suffix match: deterministic fallback (first in bucket).
        assert_eq!(
            db.lookup_by_executable("amnesia.exe"),
            Some("Amnesia: Memories")
        );
    }

    #[test]
    fn plain_and_prefixed_coexist() {
        // BlackOps Cold War has BOTH a plain and a path-prefixed entry.
        let json = r#"[
            {"name": "Call of Duty: Black Ops Cold War", "executables": [
                {"name": "call of duty black ops cold war/blackopscoldwar.exe"},
                {"name": "blackopscoldwar.exe"}
            ], "third_party_skus": []}
        ]"#;
        let db = NamingDb::parse(json).unwrap();
        assert_eq!(
            db.lookup_by_executable("blackopscoldwar.exe"),
            Some("Call of Duty: Black Ops Cold War")
        );
        assert_eq!(
            db.lookup_by_executable(
                "S:/Spiele/Call of Duty Black Ops Cold War/BlackOpsColdWar.exe"
            ),
            Some("Call of Duty: Black Ops Cold War")
        );
    }
}
