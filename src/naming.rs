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
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

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

/// The shipped shared-helper list (helper executables that can never name a
/// game — see `shared-helpers.txt` at the repo root, incident history
/// included), compiled in so the protections exist even with no file on disk.
const BUNDLED_SHARED_HELPERS: &str = include_str!("../shared-helpers.txt");

pub const SHARED_HELPERS_NAME: &str = "shared-helpers.txt";

/// The effective shared-helper set: the bundled list unioned with every
/// `shared-helpers.txt` found on disk. Union, not override — each file only
/// adds entries, so a local file can extend the shipped protections but never
/// remove them. Parsed once per process; the daemon is long-running and the
/// files do not change under it.
/// Whether a basename is one of the helper executables that ship beside many
/// games and therefore never identify one. Case-insensitive, like the lookup.
/// Used by the setup tool; the daemon reaches the set through the lookup.
#[allow(dead_code)]
pub(crate) fn is_shared_helper(basename: &str) -> bool {
    shared_helper_exes().contains(&basename.to_lowercase())
}

fn shared_helper_exes() -> &'static HashSet<String> {
    static EXES: OnceLock<HashSet<String>> = OnceLock::new();
    EXES.get_or_init(|| {
        let mut exes = parse_shared_helpers(BUNDLED_SHARED_HELPERS);
        for path in shared_helpers_candidates() {
            if let Ok(raw) = std::fs::read_to_string(&path) {
                exes.extend(parse_shared_helpers(&raw));
            }
        }
        exes
    })
}

/// The on-disk copies to union in, in the order documented in the shipped
/// file: user additions in the config dir, then the installed reference
/// copies (user data dir, then each system data dir).
fn shared_helpers_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    {
        paths.push(config.join("gamebus-presenced").join(SHARED_HELPERS_NAME));
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    {
        paths.push(data.join("gamebus-presenced").join(SHARED_HELPERS_NAME));
    }
    paths.extend(
        xdg_data_dirs()
            .into_iter()
            .map(|d| d.join("gamebus-presenced").join(SHARED_HELPERS_NAME)),
    );
    paths
}

/// Lines → lowercase basenames. Blank lines and `#` comments are skipped; a
/// junk line is an entry that matches nothing, not an error — a typo in a
/// user file must not cost the shipped protections.
fn parse_shared_helpers(raw: &str) -> HashSet<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_lowercase)
        .collect()
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
        // A shared helper exe names no game, whatever the bucket holds.
        if shared_helper_exes().contains(basename) {
            return None;
        }
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

/// Known non-game wrapper, helper, and plumbing executables. Their stems are
/// never useful game names, and they must never be classified as the game.
///
/// Three rules over a backslash-aware lowercase basename (Wine paths like
/// `C:\windows\system32\services.exe` contain no `/`):
/// 1. literal basenames — shells, launchers, Wine service processes;
/// 2. prefix families — the pressure-vessel / steam-runtime-tools crowd,
///    which ships dozens of helpers (`pv-verify`, `srt-logger`,
///    `x86_64-linux-gnu-check-vulkan`, …) that appear and vanish around a
///    launch, all preloaded into GameMode by libgamemodeauto;
/// 3. version-suffixed interpreters — `/usr/bin/python3.13` must match like
///    `python3` did (observed live: a python3.13 wrapper identified as a
///    game exe because the bare-literal list missed it).
///
/// Deliberately OFF the list, both load-bearing:
/// - `wine64-preloader` / `wine-preloader` / `wine64` — Wine games are only
///   identifiable through the cmdline layer, which `classify_member`
///   restricts for listed wrappers;
/// - `sleep` — the integration fixtures register real `sleep` processes and
///   assert their stem publishes.
fn wrapper_basename(executable: &str) -> Option<String> {
    let base = executable.rsplit(['/', '\\']).next()?;
    (!base.is_empty()).then(|| base.to_lowercase())
}

/// Lives here rather than in the enricher so both binaries can reach it:
/// the daemon classifies live processes with it, the setup tool reads
/// stashed launches. No `allow(dead_code)` needed - the daemon uses it,
/// and gamebus-setup already allows the whole module.
pub(crate) fn is_wrapper_executable(executable: &str) -> bool {
    let Some(name) = wrapper_basename(executable) else {
        return false;
    };

    const LITERALS: &[&str] = &[
        // Shells and launch plumbing.
        "env",
        "bash",
        "sh",
        "zsh",
        "fish",
        "dash",
        "ash",
        "reaper",
        "bwrap",
        "umu-run",
        "umu-shim",
        "gamemoderun",
        "lutris-wrapper",
        // Wine service processes — prefix-shaped like games, never the game.
        "wineserver",
        "services.exe",
        "winedevice.exe",
        "explorer.exe",
        "rpcss.exe",
        "plugplay.exe",
        "conhost.exe",
        "start.exe",
        "tabtip.exe",
        "svchost.exe",
        // Steam client plumbing.
        "steamwebhelper",
    ];
    if LITERALS.contains(&name.as_str()) {
        return true;
    }

    const PREFIXES: &[&str] = &[
        "steam-runtime-",
        "pressure-vessel-",
        "pv-",
        "srt-",
        "i386-linux-gnu-",
        "x86_64-linux-gnu-",
    ];
    if PREFIXES.iter().any(|p| name.starts_with(p)) {
        return true;
    }

    // `python3.13` → `python`; a name that merely CONTAINS an interpreter
    // name ("pythia") or ends in digits of its own ("portal2") never trims
    // to an exact interpreter match.
    const INTERPRETERS: &[&str] = &["python", "perl", "ruby", "node"];
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    INTERPRETERS.contains(&trimmed)
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
    fn shared_helpers_parse_skips_comments_blanks_case_and_whitespace() {
        let set = parse_shared_helpers(
            "# a comment\n\n  UnityCrashHandler64.EXE  \nhelper.exe\n\t# indented comment\n",
        );
        assert_eq!(set.len(), 2);
        assert!(set.contains("unitycrashhandler64.exe"));
        assert!(set.contains("helper.exe"));
        assert!(parse_shared_helpers("").is_empty());
        assert!(parse_shared_helpers("# only comments\n\n").is_empty());
    }

    #[test]
    fn the_bundled_shared_helpers_carry_the_unity_crash_handlers() {
        // The floor of the union: whatever local files add, these must parse
        // out of the bundled file or the Spellcraft protection is gone.
        let set = parse_shared_helpers(BUNDLED_SHARED_HELPERS);
        for exe in [
            "unitycrashhandler.exe",
            "unitycrashhandler32.exe",
            "unitycrashhandler64.exe",
        ] {
            assert!(set.contains(exe), "bundled shared-helpers.txt lost {exe}");
        }
    }

    #[test]
    fn shared_helper_exes_never_name_a_game() {
        // The Spellcraft incident: another game's install runs the shared
        // Unity crash handler; no path suffix matches, and the bucket
        // fallback would name the one game that happens to list the exe.
        let json = r#"[
            {"name": "Spellcraft", "executables": [{"name": "some game/unitycrashhandler64.exe"}], "third_party_skus": []}
        ]"#;
        let db = NamingDb::parse(json).unwrap();
        assert_eq!(
            db.lookup_by_executable("H:\\Spiele\\Other Game\\UnityCrashHandler64.exe"),
            None
        );
        // Even the listing game's own install must not resolve through the
        // helper — the real game exe is the one that identifies it.
        assert_eq!(
            db.lookup_by_executable("some game/unitycrashhandler64.exe"),
            None
        );
        assert_eq!(db.lookup_by_executable("UnityCrashHandler.exe"), None);
        assert_eq!(db.lookup_by_executable("unitycrashhandler32.exe"), None);
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

    #[test]
    fn wrapper_patterns_match_live_inventory() {
        // Positives drawn from the real pressure-vessel inventory, the Wine
        // service set, and the live journal's python3.13 incident.
        for name in [
            "/x/pv-verify",
            "/x/srt-logger",
            "/x/steam-runtime-launcher-service",
            "/x/steam-runtime-system-info",
            "/x/steam-runtime-launch-client",
            "/x/pressure-vessel-wrap",
            "/x/i386-linux-gnu-check-vulkan",
            "/x/x86_64-linux-gnu-capsule-capture-libs",
            "/x/x86_64-linux-gnu-detect-platform",
            "/x/x86_64-linux-gnu-inspect-library",
            "/usr/bin/python3.13",
            "/usr/bin/python3",
            "/usr/bin/perl5.36.0",
            "/usr/bin/node22",
            "/x/wineserver",
            "C:\\windows\\system32\\services.exe",
            "C:\\windows\\system32\\winedevice.exe",
            "C:\\windows\\system32\\conhost.exe",
            "/x/steamwebhelper",
        ] {
            assert!(is_wrapper_executable(name), "{name} must be a wrapper");
        }
    }

    #[test]
    fn wrapper_patterns_keep_games_and_preloader_off() {
        for (name, guards) in [
            // Wine games are only identifiable via the cmdline layer, which
            // classify_member restricts for listed wrappers.
            ("/x/wine64-preloader", "wine cmdline identification"),
            ("/x/wine-preloader", "wine cmdline identification"),
            ("/x/wine64", "wine cmdline identification"),
            // Integration fixtures register real sleeps and assert the stem.
            ("/usr/bin/sleep", "test fixture"),
            // Real games with digits or interpreter-ish substrings.
            ("/games/Brotato.x86_64", "real game"),
            ("Z:\\game\\Portal2.exe", "trailing digits are not a version"),
            ("/games/pythia", "contains an interpreter name"),
            ("/games/eldenring.exe", "real game"),
        ] {
            assert!(
                !is_wrapper_executable(name),
                "{name} must stay off ({guards})"
            );
        }
    }
}
