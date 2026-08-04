//! The Enricher (S4a): middleware between sources and the correlator.
//!
//! Sources emit raw [`SourceEvent`]s; the Enricher processes each event and
//! may emit additional enriched events (e.g. a Steam partial derived from
//! `/proc/<pid>/environ`). The correlator receives the enriched stream.
//!
//! ```text
//! sources → SourceEvent → Enricher → Vec<SourceEvent> → Correlator
//!                            │
//!                            ├─ steam::probe(pid) on new pid arrival
//!                            ├─ (S4b) detectable.json naming lookup
//!                            └─ (future) other launcher enrichment
//! ```
//!
//! ## Steam partial lifecycle
//!
//! Steam has no watcher and no removal signal. The Enricher tracks which
//! non-Steam sources are active per pid (`{pid: Set<Source>}`). When the
//! last non-Steam source removes its record for a pid, the Enricher emits
//! `SourceEvent::Removed` for Steam — preventing stale Steam data from
//! surviving a pid reuse.

use crate::dbus::types::{Activity, Source};
use crate::naming::NamingDb;
use crate::sources::SourceEvent;
use std::collections::{HashMap, HashSet};

/// Maximum ppid-chain depth for the ancestor walk (S4d).
/// A game launcher tree is typically 3-5 levels deep (supervisor → reaper →
/// srt-bwrap → pv-adverb → game). 10 is generous.
const MAX_ANCESTOR_DEPTH: usize = 10;

/// Maximum depth for the descendant walk (S4e).
/// The wrapper tree can be deep (lutris-wrapper → umu-run → steam-runtime-l →
/// bwrap → bwrap → pv-adverb → python3 → steam.exe → game). 12 is generous.
const MAX_DESCENDANT_DEPTH: usize = 12;

/// Maximum breadth for the descendant walk (S4e).
/// A wrapper tree can have many children (Battle.net spawns dozens of CEF
/// renderers). We only care about the game process, so we limit the total
/// number of processes visited.
const MAX_DESCENDANT_BREADTH: usize = 64;

/// Enrichment middleware: sits between sources and the correlator.
///
/// Processes raw source events, potentially emitting additional enriched
/// events. Currently handles Steam appid enrichment and naming enrichment
/// via `detectable.json`.
pub struct Enricher {
    /// pid → set of non-Steam sources currently asserting a record for it.
    /// When the last non-Steam source goes away, the Steam partial is removed.
    active_sources: HashMap<u32, HashSet<Source>>,
    /// pid → the Steam activity id currently published for it.
    /// Used to emit the correct `Removed` event when the last non-Steam
    /// source goes away.
    steam_ids: HashMap<u32, String>,
    /// pid → the Steam appid found in its environ (S4d ancestor-walk).
    steam_appids: HashMap<u32, String>,
    /// merge key → the deepest pid currently holding a record for it (S4e).
    ///
    /// One record per key: `steam:<appid>` for Steam games,
    /// `lutris:<uuid>` for Lutris games, `umu:<id>` for umu games.
    /// When a new pid arrives with the same key, `tree_depth` decides:
    /// deeper wins (replaces), shallower is suppressed. No pairwise merging,
    /// no convergence issues — one `HashMap`, one record per key.
    appid_records: HashMap<String, u32>,
    /// Wrapper pids whose game identity hasn't been resolved yet (S4e).
    /// Retried periodically — the game may launch minutes after the wrapper.
    unresolved_wrappers: HashSet<u32>,
    /// Naming database for detectable.json lookups (S4b).
    /// `None` if no database file was found — naming enrichment is disabled.
    naming: Option<NamingDb>,
}

impl Enricher {
    /// Create an Enricher without a naming database.
    /// Call [`load_naming`] after sources are spawned to avoid blocking startup.
    pub fn new() -> Self {
        Self {
            active_sources: HashMap::new(),
            steam_ids: HashMap::new(),
            steam_appids: HashMap::new(),
            appid_records: HashMap::new(),
            unresolved_wrappers: HashSet::new(),
            naming: None,
        }
    }

    /// Create an Enricher with a specific naming database (for testing).
    #[cfg(test)]
    fn with_naming(naming: Option<NamingDb>) -> Self {
        Self {
            active_sources: HashMap::new(),
            steam_ids: HashMap::new(),
            steam_appids: HashMap::new(),
            appid_records: HashMap::new(),
            unresolved_wrappers: HashSet::new(),
            naming,
        }
    }

    /// Load the naming database (blocking).
    ///
    /// Called after sources are spawned so the 12MB JSON parse doesn't
    /// delay the Discord listener or GameMode watcher.
    pub fn load_naming(&mut self) {
        self.naming = NamingDb::load();
        if let Some(ref db) = self.naming {
            tracing::info!(entries = db.len(), "Naming database loaded");
        } else {
            tracing::debug!("No naming database found; naming enrichment disabled");
        }
    }

    /// Process a raw source event, returning the events to feed the
    /// correlator. The original event is always included; additional
    /// enriched events (e.g. Steam) may follow.
    pub fn process(&mut self, event: SourceEvent) -> Vec<SourceEvent> {
        match event {
            SourceEvent::Updated(activity) => self.on_updated(*activity),
            SourceEvent::Removed { id, source } => self.on_removed(id, source),
            SourceEvent::SourceLost { source } => self.on_source_lost(source),
        }
    }

    fn on_updated(&mut self, activity: Activity) -> Vec<SourceEvent> {
        let pid = activity.process_id;
        let source = activity.sources.first().copied().unwrap_or(Source::Unknown);

        let mut events = Vec::with_capacity(3);

        // Track non-Steam sources per pid.
        if source != Source::Steam && pid > 0 {
            self.active_sources.entry(pid).or_default().insert(source);
        }

        // S4d instrumentation: log when a Discord pid has no GameMode
        // registration — a join-miss that the ancestor-walk would fix.
        if source == Source::Discord && pid > 0 {
            if let Some(sources) = self.active_sources.get(&pid) {
                if !sources.contains(&Source::GameMode) {
                    tracing::debug!(
                        pid,
                        "join-miss: Discord pid {} has no GameMode registration",
                        pid
                    );
                }
            }
        }

        // S4e: descendant-walk — if the executable is a wrapper, look for
        // the actual game process in the wrapper tree and use its
        // name/executable instead.
        let mut activity = activity;
        self.apply_descendant_walk(&mut activity);

        // S4b: naming enrichment — modify the activity's name from
        // detectable.json before forwarding. Enrichment-only: never
        // overrides a non-empty name from a more authoritative source.
        self.apply_naming(&mut activity);

        // Probe for Steam data when a non-Steam source reports a pid.
        // Skip if already tracked (the periodic scan may have found it first).
        let steam_activity =
            if source != Source::Steam && pid > 0 && !self.steam_appids.contains_key(&pid) {
                probe_steam(pid).map(|mut sa| {
                    self.apply_naming(&mut sa);
                    sa
                })
            } else {
                None
            };

        // S4e: one record per merge key. Probe for the best available key
        // (SteamAppId > LUTRIS_GAME_UUID > UMU_ID). `appid_records` maps
        // each key to the deepest pid holding its record. Deeper wins
        // (replaces), shallower is suppressed.
        if source != Source::Steam && pid > 0 {
            if let Some(key) = probe_merge_key(pid) {
                match self.appid_records.get(&key).copied() {
                    Some(existing_pid) if existing_pid != pid => {
                        let new_deeper = tree_depth(pid) > tree_depth(existing_pid);
                        if new_deeper {
                            // This pid is deeper: replace the existing record.
                            tracing::info!(
                                old = existing_pid,
                                new = pid,
                                merge_key = %key,
                                "merge: deeper pid replaces record"
                            );
                            events.push(SourceEvent::Removed {
                                id: format!("pid_{existing_pid}"),
                                source: Source::GameMode,
                            });
                            if let Some(sid) = self.steam_ids.remove(&existing_pid) {
                                events.push(SourceEvent::Removed {
                                    id: sid,
                                    source: Source::Steam,
                                });
                            }
                            self.active_sources.remove(&existing_pid);
                            self.steam_appids.remove(&existing_pid);
                            self.appid_records.insert(key, pid);
                        } else {
                            // The existing pid is deeper or equal: suppress this one.
                            tracing::info!(
                                suppressed = pid,
                                kept = existing_pid,
                                merge_key = %key,
                                "merge: suppressing shallower pid"
                            );
                            events.push(SourceEvent::Removed {
                                id: format!("pid_{pid}"),
                                source: Source::GameMode,
                            });
                            if let Some(sid) = self.steam_ids.remove(&pid) {
                                events.push(SourceEvent::Removed {
                                    id: sid,
                                    source: Source::Steam,
                                });
                            }
                            return events; // Don't forward the GameMode activity or Steam partial.
                        }
                    }
                    _ => {
                        // First pid for this key.
                        self.appid_records.insert(key, pid);
                    }
                }
            }
        }

        // Forward the original event.
        events.push(SourceEvent::Updated(Box::new(activity)));

        // Forward the Steam partial (if any).
        if let Some(sa) = steam_activity {
            let steam_id = sa.id.clone();
            self.steam_ids.insert(pid, steam_id);
            self.steam_appids.insert(pid, appid_from(&sa));
            events.push(SourceEvent::Updated(Box::new(sa)));
        }

        events
    }

    /// Apply game identification to an activity (S4e).
    ///
    /// When GameMode registers a wrapper process (env, bash, steam-runtime-l,
    /// etc.), the record's real identity comes from the actual game. Three
    /// layers, cheapest first:
    ///
    /// 1. **Wrapper cmdline** — the wrapper's own `/proc/<pid>/cmdline`
    ///    usually names the game at the end
    ///    (`... proton waitforexitandrun /path/Game.exe`).
    /// 2. **Descendant walk** — connected process trees (native Steam,
    ///    non-portal spawns): walk `/proc/*/task/*/children`, matching
    ///    exe links and Wine cmdlines.
    /// 3. **Sandbox-family scan** — Flatpak-portal spawns sever the tree
    ///    (Lutris-Flatpak + umu): all sandbox members share the umu
    ///    `var/tmp-XXXXXX` token in their cmdlines; scan `/proc` for it.
    ///
    /// If nothing is found, the pid is remembered in
    /// [`Self::unresolved_wrappers`] and retried periodically by
    /// [`Self::retry_unresolved`] — the game may launch minutes after the
    /// wrapper (Battle.net launcher → actual game).
    fn apply_descendant_walk(&mut self, activity: &mut Activity) {
        // Only for GameMode activities with wrapper executables.
        if !activity.sources.contains(&Source::GameMode) {
            return;
        }
        if activity.executable.is_empty() || !is_wrapper_executable(&activity.executable) {
            return;
        }
        let pid = activity.process_id;
        if pid == 0 {
            return;
        }

        // If the naming DB isn't loaded yet (it loads after sources spawn),
        // mark for retry instead of silently skipping.
        if self.naming.is_none() {
            self.unresolved_wrappers.insert(pid);
            return;
        }

        match self.identify_wrapper(pid) {
            Some((name, exe)) => {
                tracing::info!(wrapper_pid = pid, game_name = %name, game_exe = %exe, "identified game for wrapper");
                activity.name = name;
                activity.executable = exe;
                self.unresolved_wrappers.remove(&pid);
            }
            None => {
                // Game may not have launched yet (Battle.net launcher → game
                // starts minutes later). Retry periodically.
                self.unresolved_wrappers.insert(pid);
            }
        }
    }

    /// Periodic tick: retry wrapper identification + scan for Steam
    /// processes that no other source reported (S4e).
    ///
    /// The Steam scan closes the reactive design's gap: games launched
    /// without GameMode (no `gamemoderun`, no libgamemodeauto preload)
    /// produce no source event and would otherwise be invisible. A bounded
    /// `/proc/*/environ` scan every tick (~500 processes, ~5ms) finds them.
    pub fn tick(&mut self) -> Vec<SourceEvent> {
        let mut events = self.retry_unresolved();
        events.extend(self.scan_steam_processes());
        events
    }

    /// Retry identification for wrappers whose game hasn't been found yet.
    ///
    /// Called periodically from the main loop. Returns update events for
    /// wrappers that just became identifiable.
    pub fn retry_unresolved(&mut self) -> Vec<SourceEvent> {
        let pids: Vec<u32> = self.unresolved_wrappers.iter().copied().collect();
        let mut out = Vec::new();
        for pid in pids {
            if let Some((name, exe)) = self.identify_wrapper(pid) {
                tracing::info!(wrapper_pid = pid, game_name = %name, game_exe = %exe, "identified game for wrapper (retry)");
                let mut activity = Activity::from_gamemode(pid as i32, &exe, 0);
                activity.name = name;
                out.push(SourceEvent::Updated(Box::new(activity)));
                self.unresolved_wrappers.remove(&pid);
            }
        }
        out
    }

    /// Scan `/proc/*/environ` for Steam appids not yet tracked.
    ///
    /// For each new pid with a numeric appid: emit a Steam partial. The
    /// ancestor-walk merges wrapper-family members (deepest wins). Pids
    /// that vanish or lose their appid are reconciled (removed).
    fn scan_steam_processes(&mut self) -> Vec<SourceEvent> {
        let mut events = Vec::new();
        let mut seen: HashSet<u32> = HashSet::new();

        let Ok(entries) = std::fs::read_dir("/proc") else {
            return events;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(pid) = name.parse::<u32>() else {
                continue;
            };
            let Ok(environ) = std::fs::read_to_string(format!("/proc/{pid}/environ")) else {
                continue;
            };
            let Some(appid) = find_steam_appid(&environ) else {
                continue;
            };
            seen.insert(pid);
            // Skip already-processed pids (emitted, absorbed, or suppressed).
            if self.appid_records.values().any(|&p| p == pid) {
                continue;
            }

            // Only track processes identifiable as games. The wrapper chain
            // includes many utility processes (wineserver, tabtip.exe, etc.)
            // that inherit the appid but are NOT the game. Emitting partials
            // for them creates noise the merge can't cleanly converge.
            if let Some(ref db) = self.naming {
                if identify_process(pid, db).is_none() {
                    continue;
                }
            }

            // One record per appid: if this key already has a record, the
            // deeper pid wins (replaces), shallower is suppressed.
            let key = format!("steam:{appid}");
            match self.appid_records.get(&key).copied() {
                Some(existing_pid) if existing_pid != pid => {
                    if tree_depth(pid) > tree_depth(existing_pid) {
                        // This pid is deeper: replace the existing record.
                        tracing::debug!(old = existing_pid, new = pid, "scan: deeper pid replaces record");
                        events.push(SourceEvent::Removed {
                            id: format!("steam_{existing_pid}"),
                            source: Source::Steam,
                        });
                        self.steam_ids.remove(&existing_pid);
                        self.steam_appids.remove(&existing_pid);
                        self.appid_records.insert(key.clone(), pid);
                    } else {
                        // The existing pid is deeper or equal: suppress this one.
                        tracing::debug!(suppressed = pid, kept = existing_pid, "scan: suppressing shallower pid");
                        continue;
                    }
                }
                _ => {
                    self.appid_records.insert(key.clone(), pid);
                }
            }

            let mut activity = Activity::from_steam(pid as i32, &appid);
            self.apply_naming(&mut activity);
            self.steam_appids.insert(pid, appid);
            self.steam_ids.insert(pid, activity.id.clone());
            tracing::debug!(pid, "Steam scan: new process");
            events.push(SourceEvent::Updated(Box::new(activity)));
        }

        // Reconcile: tracked Steam pids that vanished (process died or pid
        // reused by a non-Steam process). Only remove pids with no other
        // active source — GameMode-tracked pids are managed by the
        // source-removal path.
        let tracked: Vec<u32> = self.steam_appids.keys().copied().collect();
        for pid in tracked {
            if !seen.contains(&pid) && !self.active_sources.contains_key(&pid) {
                if let Some(sid) = self.steam_ids.remove(&pid) {
                    events.push(SourceEvent::Removed {
                        id: sid,
                        source: Source::Steam,
                    });
                }
                self.steam_appids.remove(&pid);
            }
        }

        events
    }

    /// Run the three identification layers for a wrapper pid.
    /// Returns `(game_name, game_executable)` on success.
    fn identify_wrapper(&self, pid: u32) -> Option<(String, String)> {
        let db = self.naming.as_ref()?;

        // Layer 1: the wrapper's own cmdline usually names the game.
        if let Some(found) = identify_via_cmdline(pid, db) {
            return Some(found);
        }
        // Layer 2: connected descendant walk.
        if let Some(found) = find_game_descendant(pid, db) {
            return Some(found);
        }
        // Layer 3: Flatpak-portal sandbox family (umu tmpdir bridge).
        if let Some(found) = find_game_in_sandbox_family(pid, db) {
            return Some(found);
        }
        None
    }

    /// Apply naming enrichment to an activity.
    ///
    /// Precedence: Discord name > detectable.json lookup > executable stem.
    /// Only modifies the name if it's currently empty or an executable stem
    /// (i.e., not a human-curated name from Discord).
    ///
    /// Wrapper executables (env, bash, sh, etc.) are never useful game names.
    /// Their names are cleared so the record shows "(unknown)" or gets a
    /// name from detectable.json.
    fn apply_naming(&self, activity: &mut Activity) {
        // Clear wrapper executable names — "env", "bash", etc. are never
        // useful game names. This runs even without a naming database.
        if !activity.executable.is_empty() && is_wrapper_executable(&activity.executable) {
            // Only clear if the name is the executable stem (not a
            // human-curated name from Discord or detectable.json).
            let stem = std::path::Path::new(&activity.executable)
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase());
            if stem.as_deref() == Some(activity.name.to_lowercase().as_str()) {
                activity.name.clear();
            }
        }

        let Some(ref db) = self.naming else {
            return;
        };

        // Discord activities already have a human-curated name — don't touch.
        if activity.sources.contains(&Source::Discord) && !activity.name.is_empty() {
            return;
        }

        // Try appid lookup first (stronger signal), then executable.
        let name = activity
            .app_ids
            .get("steam")
            .and_then(|appid| db.lookup_by_steam_appid(appid))
            .or_else(|| {
                if activity.executable.is_empty() {
                    None
                } else {
                    db.lookup_by_executable(&activity.executable)
                }
            });

        if let Some(name) = name {
            activity.name = name.to_string();
        }
    }

    fn on_removed(&mut self, id: String, source: Source) -> Vec<SourceEvent> {
        // Extract pid before moving `id` into the event.
        let pid = if source != Source::Steam {
            extract_pid(&id)
        } else {
            None
        };

        let mut events = vec![SourceEvent::Removed { id, source }];

        // If this is a non-Steam source removal, check if the pid has any
        // remaining non-Steam sources. If not, remove the Steam partial.
        if let Some(pid) = pid {
            self.unresolved_wrappers.remove(&pid);
            // Remove this pid from appid_records if it holds one.
            self.appid_records.retain(|_, p| *p != pid);
            if let Some(sources) = self.active_sources.get_mut(&pid) {
                sources.remove(&source);
                if sources.is_empty() {
                    self.active_sources.remove(&pid);
                    if let Some(steam_id) = self.steam_ids.remove(&pid) {
                        events.push(SourceEvent::Removed {
                            id: steam_id,
                            source: Source::Steam,
                        });
                    }
                }
            }
        }

        events
    }

    fn on_source_lost(&mut self, source: Source) -> Vec<SourceEvent> {
        let mut events = vec![SourceEvent::SourceLost { source }];

        // When a non-Steam source is lost, remove its entries from all pids.
        // If a pid's non-Steam source set becomes empty, remove its Steam partial.
        if source != Source::Steam {
            let mut pids_to_check: Vec<u32> = self.active_sources.keys().copied().collect();
            for pid in pids_to_check.drain(..) {
                if let Some(sources) = self.active_sources.get_mut(&pid) {
                    sources.remove(&source);
                    if sources.is_empty() {
                        self.active_sources.remove(&pid);
                        self.appid_records.retain(|_, p| *p != pid);
                        if let Some(steam_id) = self.steam_ids.remove(&pid) {
                            events.push(SourceEvent::Removed {
                                id: steam_id,
                                source: Source::Steam,
                            });
                        }
                    }
                }
            }
        }

        events
    }
}

/// Extract the pid from a source-scoped activity id.
///
/// Ids follow the pattern `<prefix>_<pid>` (e.g. `pid_42`, `discord_42`,
/// `steam_42`). Returns `None` if the id doesn't match this pattern.
fn extract_pid(id: &str) -> Option<u32> {
    let (_, pid_str) = id.rsplit_once('_')?;
    pid_str.parse().ok()
}

/// Probe `/proc/<pid>/environ` for the best available merge key (S4e).
///
/// Priority: `SteamAppId` (numeric) > `LUTRIS_GAME_UUID` > `UMU_ID`
/// (non-default). The merge key is used for ancestor-walk deduplication
/// across wrapper trees — two processes sharing a key and a process tree
/// are the same game.
fn probe_merge_key(pid: u32) -> Option<String> {
    let environ = std::fs::read_to_string(format!("/proc/{pid}/environ")).ok()?;
    let mut steam_appid = None;
    let mut lutris_uuid = None;
    let mut umu_id = None;
    for entry in environ.split('\0') {
        if let Some(v) = entry.strip_prefix("SteamAppId=") {
            if v.chars().all(|c| c.is_ascii_digit()) && !v.is_empty() {
                steam_appid = Some(format!("steam:{v}"));
            }
        } else if let Some(v) = entry.strip_prefix("SteamGameId=") {
            if steam_appid.is_none() && v.chars().all(|c| c.is_ascii_digit()) && !v.is_empty() {
                steam_appid = Some(format!("steam:{v}"));
            }
        } else if let Some(v) = entry.strip_prefix("LUTRIS_GAME_UUID=") {
            if !v.is_empty() {
                lutris_uuid = Some(format!("lutris:{v}"));
            }
        } else if let Some(v) = entry.strip_prefix("UMU_ID=umu-") {
            if v != "default" && !v.is_empty() {
                umu_id = Some(format!("umu:{v}"));
            }
        }
    }
    steam_appid.or(lutris_uuid).or(umu_id)
}

/// Probe `/proc/<pid>/environ` for Steam appid variables.
///
/// Checks in order:
/// 1. `UMU_ID=umu-<N>` — umu-launcher convention; numeric N implies steam appid N
/// 2. `SteamAppId` — native Steam
/// 3. `SteamGameId` — native Steam (alternative variable)
///
/// Returns `Some(Activity)` with `Source::Steam` if any appid is found.
pub fn probe_steam(pid: u32) -> Option<Activity> {
    let environ_path = format!("/proc/{pid}/environ");
    let environ = std::fs::read_to_string(&environ_path).ok()?;

    let appid = find_steam_appid(&environ)?;
    Some(Activity::from_steam(pid as i32, &appid))
}

/// Parse Steam appid from a `/proc/<pid>/environ` string.
///
/// The environ file is NUL-separated `KEY=VALUE` pairs. We check for
/// `UMU_ID=umu-<N>` first (umu-launcher), then `SteamAppId`, then
/// `SteamGameId`. Only non-zero numeric values are accepted — Steam sets
/// `SteamAppId=default` for its own client processes and `SteamAppId=0`
/// for non-Steam games, neither of which is a game appid.
fn find_steam_appid(environ: &str) -> Option<String> {
    for entry in environ.split('\0') {
        if let Some(value) = entry.strip_prefix("UMU_ID=umu-") {
            // umu-launcher: numeric N implies steam appid N
            if value.chars().all(|c| c.is_ascii_digit()) && !value.is_empty() && value != "0" {
                return Some(value.to_string());
            }
        }
        if let Some(value) = entry.strip_prefix("SteamAppId=") {
            if value.chars().all(|c| c.is_ascii_digit()) && !value.is_empty() && value != "0" {
                return Some(value.to_string());
            }
        }
        if let Some(value) = entry.strip_prefix("SteamGameId=") {
            if value.chars().all(|c| c.is_ascii_digit()) && !value.is_empty() && value != "0" {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Identify a process as a game via `/proc/<pid>/exe` (native) or
/// `/proc/<pid>/cmdline` tokens (Wine games: exe link is wine64-preloader,
/// the real exe is a cmdline token with backslash separators).
///
/// Returns `(game_name, game_executable)` on a detectable.json match.
fn identify_process(pid: u32, db: &NamingDb) -> Option<(String, String)> {
    // Native binary: exe link.
    if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        if let Some(exe_str) = exe.to_str() {
            if let Some(name) = db.lookup_by_executable(exe_str) {
                return Some((name.to_string(), exe_str.to_string()));
            }
        }
    }
    // Wine game: cmdline tokens (backslash-normalised by the lookup).
    if let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{pid}/cmdline")) {
        for token in cmdline.split('\0').filter(|t| !t.is_empty()) {
            if let Some(name) = db.lookup_by_executable(token) {
                return Some((name.to_string(), token.to_string()));
            }
        }
    }
    None
}

/// Layer 1: identify a wrapper's game from its own cmdline (S4e).
///
/// Launch wrappers carry the game path at the end of their command line:
/// `... proton waitforexitandrun /path/Game.exe`. Cheap, no tree walk.
fn identify_via_cmdline(pid: u32, db: &NamingDb) -> Option<(String, String)> {
    let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).ok()?;
    let mut result = None;
    for token in cmdline.split('\0').filter(|t| !t.is_empty()) {
        if let Some(name) = db.lookup_by_executable(token) {
            // Keep scanning: the LAST match wins — the game exe is at the
            // end of the wrapper's cmdline (after proton/umu-shim paths).
            result = Some((name.to_string(), token.to_string()));
        }
    }
    result
}

/// Layer 3: find the game inside a Flatpak-portal-spawned sandbox (S4e).
///
/// When Lutris runs as a Flatpak, steam-runtime-launch-client asks
/// `org.freedesktop.portal.Flatpak` to spawn the bwrap sandbox — the
/// process tree is severed (the sandbox's parent is the portal, not the
/// wrapper). All sandbox members share the umu `var/tmp-XXXXXX` token in
/// their cmdlines; scan `/proc` for it, then identify among family members
/// and their descendants.
fn find_game_in_sandbox_family(pid: u32, db: &NamingDb) -> Option<(String, String)> {
    let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).ok()?;
    let token = umu_tmpdir_token(&cmdline)?;

    let mut best: Option<(String, String)> = None;
    for family_pid in scan_proc_for_cmdline_token(&token) {
        // Prefer non-wrapper exes (the actual game over another wrapper).
        if let Some((name, exe)) = identify_process(family_pid, db) {
            let exe_is_wrapper = is_wrapper_executable(&exe);
            match (&best, exe_is_wrapper) {
                (None, _) => best = Some((name, exe)),
                (Some((_, e)), false) if is_wrapper_executable(e) => {
                    best = Some((name, exe));
                }
                _ => {}
            }
        }
        // Also walk the family member's descendants (connected within the
        // sandbox — the game is a child of pv-adverb).
        if let Some((name, exe)) = find_game_descendant(family_pid, db) {
            if !is_wrapper_executable(&exe) {
                return Some((name, exe));
            }
            if best.is_none() {
                best = Some((name, exe));
            }
        }
    }
    best
}

/// Extract the umu `tmp-XXXXXX` sandbox token from a cmdline string.
///
/// umu/pressure-vessel puts `--app-path .../var/tmp-XXXXXX/app` in every
/// sandbox member's cmdline. Returns e.g. `tmp-VI2WT3`.
fn umu_tmpdir_token(cmdline: &str) -> Option<String> {
    let idx = cmdline.find("var/tmp-")?;
    let rest = &cmdline[idx + "var/".len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_string())
}

/// Scan `/proc` for processes whose cmdline contains `token`.
fn scan_proc_for_cmdline_token(token: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return pids;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{pid}/cmdline")) {
            if cmdline.contains(token) {
                pids.push(pid);
            }
        }
    }
    pids
}

/// Walk the process tree downward from `pid`, looking for a process whose
/// executable matches a detectable.json entry.
///
/// Returns `(pid, executable_path)` of the first game found, or `None`.
/// The walk is bounded by `MAX_DESCENDANT_DEPTH` and `MAX_DESCENDANT_BREADTH`.
fn find_game_descendant(pid: u32, db: &NamingDb) -> Option<(String, String)> {
    let mut visited = 0;
    find_game_descendant_inner(pid, db, 0, &mut visited)
}

fn find_game_descendant_inner(
    pid: u32,
    db: &NamingDb,
    depth: usize,
    visited: &mut usize,
) -> Option<(String, String)> {
    if depth > MAX_DESCENDANT_DEPTH || *visited >= MAX_DESCENDANT_BREADTH {
        return None;
    }
    *visited += 1;

    // Check this process (exe link + Wine cmdline).
    if let Some(found) = identify_process(pid, db) {
        return Some(found);
    }

    // Not a game — check children.
    let children = read_children(pid);
    for child in children {
        if let Some(found) = find_game_descendant_inner(child, db, depth + 1, visited) {
            return Some(found);
        }
    }
    None
}

/// Read the child pids from `/proc/<pid>/task/<pid>/children`.
fn read_children(pid: u32) -> Vec<u32> {
    let path = format!("/proc/{pid}/task/{pid}/children");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// Known non-game wrapper executables. The executable stem of a wrapper
/// is never a useful game name — clear it so the record shows "(unknown)"
/// or gets a name from detectable.json.
fn is_wrapper_executable(executable: &str) -> bool {
    let filename = std::path::Path::new(executable)
        .file_name()
        .map(|f| f.to_string_lossy().to_lowercase());
    matches!(
        filename.as_deref(),
        Some(
            "env"
                | "bash"
                | "sh"
                | "zsh"
                | "fish"
                | "dash"
                | "ash"
                | "python"
                | "python3"
                | "python2"
                | "perl"
                | "ruby"
                | "node"
                | "steam-runtime-launch-client"
                | "steam-runtime-supervisor"
                | "reaper"
                | "srt-bwrap"
                | "pv-adverb"
                | "bwrap"
                | "umu-run"
                | "umu-shim"
                | "gamemoderun"
                | "lutris-wrapper"
        )
    )
}

/// Extract the Steam appid from a Steam activity.
fn appid_from(activity: &Activity) -> String {
    activity.app_ids.get("steam").cloned().unwrap_or_default()
}

/// Read the parent pid from `/proc/<pid>/stat`.
///
/// The ppid is field 4 of the stat line. After the last `)` on the line
/// (the comm field can contain spaces and parens), the tokens are:
/// state(0), ppid(1), pgrp(2), ...
fn read_ppid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// The depth of a pid in the process tree (number of ancestors).
/// Deeper pids are closer to the actual game process.
fn tree_depth(pid: u32) -> usize {
    let mut depth = 0;
    let mut current = pid;
    for _ in 0..MAX_ANCESTOR_DEPTH {
        match read_ppid(current) {
            Some(ppid) if ppid > 0 => {
                depth += 1;
                current = ppid;
            }
            _ => break,
        }
    }
    depth
}

/// Check whether `ancestor` is an ancestor of `descendant` by walking the
/// ppid chain from `descendant` upward, bounded to `MAX_ANCESTOR_DEPTH` hops.
///
/// Both pids must be alive. Returns `false` if the chain is broken
/// (a process died mid-walk) or the depth limit is reached.
#[cfg(test)]
fn is_ancestor(ancestor: u32, descendant: u32) -> bool {
    if ancestor == descendant {
        return false;
    }
    let mut current = descendant;
    for _ in 0..MAX_ANCESTOR_DEPTH {
        match read_ppid(current) {
            Some(ppid) if ppid > 0 => {
                if ppid == ancestor {
                    return true;
                }
                current = ppid;
            }
            _ => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gamemode_activity(pid: u32) -> Activity {
        Activity::from_gamemode(pid as i32, "/games/eldenring.exe", 1_700_000_100)
    }

    #[test]
    fn extract_pid_from_source_scoped_ids() {
        assert_eq!(extract_pid("pid_42"), Some(42));
        assert_eq!(extract_pid("discord_123"), Some(123));
        assert_eq!(extract_pid("steam_7"), Some(7));
        assert_eq!(extract_pid("no_pid"), None);
        assert_eq!(extract_pid("pid_"), None);
        assert_eq!(extract_pid(""), None);
    }

    #[test]
    fn find_steam_appid_from_environ() {
        // UMU_ID takes precedence
        let env = "PATH=/usr/bin\0UMU_ID=umu-480\0SteamAppId=123\0";
        assert_eq!(find_steam_appid(env), Some("480".to_string()));

        // SteamAppId fallback
        let env = "PATH=/usr/bin\0SteamAppId=12345\0";
        assert_eq!(find_steam_appid(env), Some("12345".to_string()));

        // SteamGameId fallback
        let env = "PATH=/usr/bin\0SteamGameId=67890\0";
        assert_eq!(find_steam_appid(env), Some("67890".to_string()));

        // No Steam data
        let env = "PATH=/usr/bin\0HOME=/home/user\0";
        assert_eq!(find_steam_appid(env), None);

        // Empty values are ignored
        let env = "SteamAppId=\0";
        assert_eq!(find_steam_appid(env), None);

        // Non-numeric values are ignored (Steam sets SteamAppId=default for
        // its own client processes — not a game appid).
        let env = "SteamAppId=default\0";
        assert_eq!(find_steam_appid(env), None);

        // Non-numeric UMU_ID is ignored
        let env = "UMU_ID=umu-latest\0";
        assert_eq!(find_steam_appid(env), None);
    }

    #[test]
    fn enricher_emits_steam_partial_on_updated() {
        // This test uses the current process's own /proc/<pid>/environ,
        // which won't have Steam vars. So we test the non-Steam path.
        let mut e = Enricher::new();
        let events = e.process(SourceEvent::Updated(Box::new(gamemode_activity(99999))));
        // Should have the original event; Steam probe will fail (no such pid)
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SourceEvent::Updated(_)));
    }

    #[test]
    fn enricher_removes_steam_on_last_non_steam_source() {
        let mut e = Enricher::new();

        // Simulate: GameMode and Discord both report pid 42
        e.active_sources
            .entry(42)
            .or_default()
            .insert(Source::GameMode);
        e.active_sources
            .entry(42)
            .or_default()
            .insert(Source::Discord);
        e.steam_ids.insert(42, "steam_42".to_string());

        // GameMode removes its record — Discord still active
        let events = e.process(SourceEvent::Removed {
            id: "pid_42".to_string(),
            source: Source::GameMode,
        });
        assert_eq!(events.len(), 1); // Only the original removal
        assert!(e.steam_ids.contains_key(&42));

        // Discord removes its record — last non-Steam source gone
        let events = e.process(SourceEvent::Removed {
            id: "discord_42".to_string(),
            source: Source::Discord,
        });
        assert_eq!(events.len(), 2); // Original removal + Steam removal
        assert!(matches!(events[1], SourceEvent::Removed { .. }));
        if let SourceEvent::Removed { id, source } = &events[1] {
            assert_eq!(id, "steam_42");
            assert_eq!(*source, Source::Steam);
        }
        assert!(!e.steam_ids.contains_key(&42));
    }

    #[test]
    fn enricher_source_lost_removes_steam_for_orphaned_pids() {
        let mut e = Enricher::new();

        // Two pids, both with GameMode and Steam
        for pid in [10, 20] {
            e.active_sources
                .entry(pid)
                .or_default()
                .insert(Source::GameMode);
            e.steam_ids.insert(pid, format!("steam_{pid}"));
        }

        // GameMode source lost
        let events = e.process(SourceEvent::SourceLost {
            source: Source::GameMode,
        });
        // Original SourceLost + 2 Steam removals
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], SourceEvent::SourceLost { .. }));
        assert!(e.steam_ids.is_empty());
        assert!(e.active_sources.is_empty());
    }

    #[test]
    fn enricher_steam_events_pass_through() {
        let mut e = Enricher::new();
        let steam = Activity::from_steam(42, "480");
        let events = e.process(SourceEvent::Updated(Box::new(steam)));
        // Steam events are not re-probed (source == Steam)
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn is_ancestor_basic() {
        let own_pid = std::process::id();
        // init (pid 1) is an ancestor of every process.
        assert!(is_ancestor(1, own_pid));
        // A process is not its own ancestor.
        assert!(!is_ancestor(own_pid, own_pid));
        // A non-existent pid breaks the chain.
        assert!(!is_ancestor(999999, own_pid));
    }

    #[test]
    fn ancestor_walk_merges_duplicate_appid() {
        let mut e = Enricher::with_naming(None);

        // Simulate: ancestor pid 100 already tracked with appid "480"
        e.active_sources
            .entry(100)
            .or_default()
            .insert(Source::GameMode);
        e.steam_ids.insert(100, "steam_100".to_string());
        e.steam_appids.insert(100, "480".to_string());

        // Now the descendant pid 101 arrives with the same appid.
        // is_ancestor(100, 101) will be false (they're not related in /proc),
        // so find_related_pid won't match. This tests the non-related case.
        let activity = gamemode_activity(101);
        let events = e.process(SourceEvent::Updated(Box::new(activity)));
        // No merge should happen (pids are not related).
        assert!(!events
            .iter()
            .any(|ev| matches!(ev, SourceEvent::Removed { .. })));
    }

    #[test]
    fn ancestor_walk_no_appid_no_merge() {
        let mut e = Enricher::with_naming(None);

        // Track a pid without a Steam appid
        e.active_sources
            .entry(200)
            .or_default()
            .insert(Source::GameMode);

        // A new pid arrives — no Steam probe (no appid in /proc), no merge
        let activity = gamemode_activity(201);
        let events = e.process(SourceEvent::Updated(Box::new(activity)));
        assert!(!events
            .iter()
            .any(|ev| matches!(ev, SourceEvent::Removed { .. })));
    }

    #[test]
    fn read_ppid_own_process() {
        let own_pid = std::process::id();
        let ppid = read_ppid(own_pid);
        assert!(ppid.is_some());
        assert!(ppid.unwrap() > 0);
    }

    #[test]
    fn enricher_pid_zero_skipped() {
        let mut e = Enricher::new();
        let mut a = gamemode_activity(0);
        a.process_id = 0;
        let events = e.process(SourceEvent::Updated(Box::new(a)));
        assert_eq!(events.len(), 1); // No Steam probe for pid 0
        assert!(e.active_sources.is_empty());
    }

    #[test]
    fn naming_enriches_gamemode_executable() {
        let json = r#"[{"name": "Elden Ring", "executables": [{"name": "eldenring.exe"}], "third_party_skus": []}]"#;
        let db = NamingDb::parse(json).unwrap();
        let mut e = Enricher::with_naming(Some(db));

        let activity = gamemode_activity(42);
        assert_eq!(activity.name, "eldenring"); // executable stem

        let events = e.process(SourceEvent::Updated(Box::new(activity)));
        assert_eq!(events.len(), 1);
        if let SourceEvent::Updated(enriched) = &events[0] {
            assert_eq!(enriched.name, "Elden Ring");
        } else {
            panic!("expected Updated event");
        }
    }

    #[test]
    fn naming_enriches_steam_appid() {
        let json = r#"[{"name": "Elden Ring", "executables": [], "third_party_skus": [{"distributor": "steam", "id": "1245620"}]}]"#;
        let db = NamingDb::parse(json).unwrap();
        let e = Enricher::with_naming(Some(db));

        let mut steam = Activity::from_steam(42, "1245620");
        assert_eq!(steam.name, ""); // Steam knows only the appid

        // The Enricher should enrich the Steam partial's name
        e.apply_naming(&mut steam);
        assert_eq!(steam.name, "Elden Ring");
    }

    #[test]
    fn naming_never_overrides_discord_name() {
        let json = r#"[{"name": "Wrong Name", "executables": [{"name": "eldenring.exe"}], "third_party_skus": []}]"#;
        let db = NamingDb::parse(json).unwrap();
        let e = Enricher::with_naming(Some(db));

        let mut activity = Activity::new("discord_42");
        activity.sources = vec![Source::Discord];
        activity.name = "Elden Ring".to_string();
        activity.process_id = 42;

        e.apply_naming(&mut activity);
        assert_eq!(activity.name, "Elden Ring"); // Discord name preserved
    }

    #[test]
    fn naming_no_db_is_noop() {
        let mut e = Enricher::with_naming(None);
        let activity = gamemode_activity(42);
        let events = e.process(SourceEvent::Updated(Box::new(activity)));
        assert_eq!(events.len(), 1);
        if let SourceEvent::Updated(enriched) = &events[0] {
            assert_eq!(enriched.name, "eldenring"); // unchanged
        }
    }
}
