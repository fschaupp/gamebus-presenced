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
            naming: None,
        }
    }

    /// Create an Enricher with a specific naming database (for testing).
    #[cfg(test)]
    fn with_naming(naming: Option<NamingDb>) -> Self {
        Self {
            active_sources: HashMap::new(),
            steam_ids: HashMap::new(),
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

        let mut events = Vec::with_capacity(2);

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

        // S4b: naming enrichment — modify the activity's name from
        // detectable.json before forwarding. Enrichment-only: never
        // overrides a non-empty name from a more authoritative source.
        let mut activity = activity;
        self.apply_naming(&mut activity);

        // Always forward the original event.
        events.push(SourceEvent::Updated(Box::new(activity)));

        // Probe for Steam data when a non-Steam source reports a pid.
        if source != Source::Steam && pid > 0 {
            if let Some(mut steam_activity) = probe_steam(pid) {
                // Also apply naming to the Steam partial (by appid).
                self.apply_naming(&mut steam_activity);
                let steam_id = steam_activity.id.clone();
                self.steam_ids.insert(pid, steam_id);
                events.push(SourceEvent::Updated(Box::new(steam_activity)));
            }
        }

        events
    }

    /// Apply naming enrichment to an activity.
    ///
    /// Precedence: Discord name > detectable.json lookup > executable stem.
    /// Only modifies the name if it's currently empty or an executable stem
    /// (i.e., not a human-curated name from Discord).
    fn apply_naming(&self, activity: &mut Activity) {
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
/// `SteamGameId`.
fn find_steam_appid(environ: &str) -> Option<String> {
    for entry in environ.split('\0') {
        if let Some(value) = entry.strip_prefix("UMU_ID=umu-") {
            // umu-launcher: numeric N implies steam appid N
            if value.chars().all(|c| c.is_ascii_digit()) && !value.is_empty() {
                return Some(value.to_string());
            }
        }
        if let Some(value) = entry.strip_prefix("SteamAppId=") {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        if let Some(value) = entry.strip_prefix("SteamGameId=") {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
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
