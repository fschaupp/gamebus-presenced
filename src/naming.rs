//! Naming enrichment (S4b): maps executables and Steam appids to human-readable
//! game names using Discord's `detectable.json` database.
//!
//! The database is fetched at build time by `build.rs` and ships as an
//! installation data file (not embedded in the binary). At runtime, the
//! daemon looks for it in this order:
//!
//! 1. `$XDG_CACHE_HOME/gamebus-presenced/detectable.json` (CLI-refreshed)
//! 2. `$XDG_DATA_HOME/gamebus-presenced/detectable.json` (user-installed)
//! 3. `/usr/share/gamebus-presenced/detectable.json` (system-installed)
//! 4. `OUT_DIR/detectable.json` (build-time, for development)
//!
//! If no database is found, naming enrichment is silently disabled — the
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
    /// executable filename (lowercase, no path) → game name
    by_executable: HashMap<String, String>,
    /// steam appid → game name
    by_steam_appid: HashMap<String, String>,
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
        let mut by_executable = HashMap::new();
        let mut by_steam_appid = HashMap::new();

        for entry in entries {
            for exe in &entry.executables {
                let key = exe.name.to_lowercase();
                by_executable
                    .entry(key)
                    .or_insert_with(|| entry.name.clone());
            }
            for sku in &entry.third_party_skus {
                if sku.distributor == "steam" {
                    if let Some(ref id) = sku.id {
                        by_steam_appid
                            .entry(id.clone())
                            .or_insert_with(|| entry.name.clone());
                    }
                }
            }
        }

        Some(Self {
            by_executable,
            by_steam_appid,
        })
    }

    /// Look up a game name by executable path or filename.
    ///
    /// The lookup is case-insensitive and matches on the filename component
    /// only (path is stripped). Returns `None` if not found.
    pub fn lookup_by_executable(&self, executable: &str) -> Option<&str> {
        let filename = std::path::Path::new(executable)
            .file_name()
            .map(|f| f.to_string_lossy().to_lowercase())?;
        self.by_executable.get(&filename).map(|s| s.as_str())
    }

    /// Look up a game name by Steam appid.
    pub fn lookup_by_steam_appid(&self, appid: &str) -> Option<&str> {
        self.by_steam_appid.get(appid).map(|s| s.as_str())
    }

    /// Number of entries in the database.
    pub fn len(&self) -> usize {
        self.by_executable.len().max(self.by_steam_appid.len())
    }
}

/// Find the detectable.json file in the standard search paths.
fn find_detectable_json() -> Option<PathBuf> {
    let candidates = [
        // CLI-refreshed cache (highest priority)
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .map(|p| p.join("gamebus-presenced/detectable.json")),
        // User data directory
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|p| p.join("gamebus-presenced/detectable.json")),
        // System data directory
        Some(PathBuf::from(
            "/usr/share/gamebus-presenced/detectable.json",
        )),
        // Build-time output directory (development)
        Some(PathBuf::from(concat!(env!("OUT_DIR"), "/detectable.json"))),
    ];

    candidates.into_iter().flatten().find(|p| p.exists())
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
}
