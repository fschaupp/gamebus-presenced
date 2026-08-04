//! The correlator: joins per-source records by pid into one activity.
//!
//! Sources publish partial records that each know a fragment of the truth:
//! GameMode knows pid + executable, Discord knows title/chapter/artwork. The
//! correlator keeps every source's latest partial per pid and derives the
//! published [`Activity`] from them, so a record carries every fragment at
//! once and degrades - never dies - when one source goes away ("the record
//! dies with the last source").
//!
//! Join rules:
//! - The join key is the exact pid. A umu/Proton wrapper tree, where GameMode
//!   registers a wrapper pid and Discord connects from a child process, is a
//!   documented miss - no fabricated joins (design non-goal).
//! - A merged record keeps GameMode's `pid_<pid>` identity: GameMode is the
//!   more reliable source (kernel-tracked registration vs. self-reported
//!   presence), and a stable id spares consumers object-path churn.
//! - Sources a record carries are listed in canonical order: gamemode first,
//!   then discord.
//!
//! The correlator is pure state + functions over records - no D-Bus, no
//! sockets - so the merge/split rules are unit-testable in isolation.

use crate::dbus::types::{Activity, Kind, Source};
use std::collections::HashMap;

/// What the daemon core must do to the bus surface after a correlator input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Register a new activity object and emit ActivityAdded.
    PublishNew(Activity),
    /// Refresh an existing object in place (PropertiesChanged).
    UpdateInPlace(Activity),
    /// Remove the object with this id and emit ActivityRemoved.
    Remove(String),
}

/// One pid's per-source partial records.
#[derive(Debug, Default)]
struct Partials {
    gamemode: Option<Activity>,
    discord: Option<Activity>,
    steam: Option<Activity>,
}

impl Partials {
    fn slot(&mut self, source: Source) -> Option<&mut Option<Activity>> {
        match source {
            Source::GameMode => Some(&mut self.gamemode),
            Source::Discord => Some(&mut self.discord),
            Source::Steam => Some(&mut self.steam),
            _ => None,
        }
    }

    fn is_empty(&self) -> bool {
        self.gamemode.is_none() && self.discord.is_none() && self.steam.is_none()
    }
}

/// Per-pid join state plus which activity id is currently on the bus.
#[derive(Debug, Default)]
pub struct Correlator {
    partials: HashMap<u32, Partials>,
    /// pid -> currently published activity id.
    published: HashMap<u32, String>,
    /// published activity id -> pid (for source-scoped removal lookups).
    id_index: HashMap<String, u32>,
}

impl Correlator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Re-adopt a cache-restored record.
    ///
    /// The record is published under its cached identity and indexed, but it
    /// is NOT stored as a source partial - no live source is asserting it.
    /// Consequences, all deliberate (design doc: best-effort):
    /// - Sources that re-derive overwrite it naturally (GameMode's startup
    ///   seed merges in place; a reconnecting Discord client updates or
    ///   re-publishes it).
    /// - Fields whose source never comes back stay at their last-known
    ///   values instead of going quiet.
    /// - Nothing watches the pid afterwards, so a re-adopted record whose
    ///   process later dies lingers until the next restart. Accepted.
    pub fn adopt(&mut self, activity: Activity) -> Vec<Effect> {
        let pid = activity.process_id;
        if pid == 0 {
            return Vec::new();
        }
        self.id_index.insert(activity.id.clone(), pid);
        self.published.insert(pid, activity.id.clone());
        vec![Effect::PublishNew(activity)]
    }

    /// Currently published (pid, id) pairs - the cache's write set.
    pub fn published_pairs(&self) -> Vec<(u32, String)> {
        self.published
            .iter()
            .map(|(pid, id)| (*pid, id.clone()))
            .collect()
    }

    /// A source created or updated a record. Returns the bus effects.
    ///
    /// `activity` must be single-source with a non-zero `process_id`; both
    /// hold for every source implemented so far. Records that cannot be tied
    /// to a pid are the caller's problem (no source emits them today).
    pub fn on_updated(&mut self, activity: Activity) -> Vec<Effect> {
        let pid = activity.process_id;
        let source = activity.sources.first().copied().unwrap_or(Source::Unknown);
        if pid == 0 {
            // Uncorrelated record: publish as-is, unjoined, as a
            // lower-confidence record. Not exercised by any current source.
            return vec![Effect::UpdateInPlace(activity)];
        }

        let partials = self.partials.entry(pid).or_default();
        match partials.slot(source) {
            Some(slot) => {
                // Index the partial's source-scoped id regardless of what
                // ends up published: sources reference their own id in
                // removal events, whatever identity the record has on the bus.
                self.id_index.insert(activity.id.clone(), pid);
                *slot = Some(activity);
            }
            None => {
                // No partial slot for this source (Steam partials arrive later):
                // publish without joining rather than drop on the floor.
                return vec![Effect::UpdateInPlace(activity)];
            }
        }

        let merged = merge(
            pid,
            partials.gamemode.as_ref(),
            partials.discord.as_ref(),
            partials.steam.as_ref(),
        );
        self.publish(pid, merged)
    }

    /// A source stopped asserting one record (by its source-scoped id).
    pub fn on_removed(&mut self, id: &str, source: Source) -> Vec<Effect> {
        let Some(&pid) = self.id_index.get(id) else {
            return Vec::new();
        };
        self.drop_partial(pid, source)
    }

    /// A source disappeared entirely: every record it backed degrades.
    pub fn on_source_lost(&mut self, source: Source) -> Vec<Effect> {
        let pids: Vec<u32> = self
            .partials
            .iter()
            .filter(|(_, p)| match source {
                Source::GameMode => p.gamemode.is_some(),
                Source::Discord => p.discord.is_some(),
                _ => false,
            })
            .map(|(pid, _)| *pid)
            .collect();
        pids.into_iter()
            .flat_map(|pid| self.drop_partial(pid, source))
            .collect()
    }

    /// Drop one partial and recompute the pid's published record.
    ///
    /// Cache-adopted records have no partial - they're published directly
    /// via `adopt()`. If the pid has no partial entry but IS published,
    /// the removal targets the adopted record itself.
    fn drop_partial(&mut self, pid: u32, source: Source) -> Vec<Effect> {
        // Fast path: no partial entry at all.
        if !self.partials.contains_key(&pid) {
            // Check if this is a cache-adopted record (published but no partials).
            if let Some(old_id) = self.published.remove(&pid) {
                self.id_index.retain(|_, p| *p != pid);
                return vec![Effect::Remove(old_id)];
            }
            return Vec::new();
        }

        let partials = self.partials.get_mut(&pid).unwrap();
        match partials.slot(source) {
            Some(slot) => *slot = None,
            None => return Vec::new(),
        }

        if partials.is_empty() {
            // The record dies with the last source.
            self.partials.remove(&pid);
            self.id_index.retain(|_, p| *p != pid);
            if let Some(old_id) = self.published.remove(&pid) {
                return vec![Effect::Remove(old_id)];
            }
            return Vec::new();
        }

        let partials = self.partials.get(&pid).unwrap();
        let merged = merge(
            pid,
            partials.gamemode.as_ref(),
            partials.discord.as_ref(),
            partials.steam.as_ref(),
        );
        self.publish(pid, merged)
    }

    /// Publish (or refresh) the derived record, emitting absorb effects when
    /// the published identity changes (e.g. solo `discord_X` joined by
    /// GameMode becomes `pid_X`; a merged record losing GameMode splits back
    /// to `discord_X`).
    ///
    /// `id_index` keeps every id a pid has ever published under, not just the
    /// current one: sources reference their own scoped id in removal events,
    /// and after an absorb the published id no longer matches the id the
    /// absorbed source will use when it says goodbye.
    fn publish(&mut self, pid: u32, merged: Activity) -> Vec<Effect> {
        let new_id = merged.id.clone();
        match self.published.get(&pid) {
            None => {
                self.published.insert(pid, new_id.clone());
                self.id_index.insert(new_id, pid);
                vec![Effect::PublishNew(merged)]
            }
            Some(old_id) if *old_id == new_id => vec![Effect::UpdateInPlace(merged)],
            Some(old_id) => {
                let old_id = old_id.clone();
                self.id_index.insert(new_id.clone(), pid);
                self.published.insert(pid, new_id);
                vec![Effect::Remove(old_id), Effect::PublishNew(merged)]
            }
        }
    }
}

/// Derive the published record from a pid's partials.
///
/// Field precedence: Discord's human-facing fields win
/// (name, details/state, timestamps, artwork, party) because they are the
/// point of the Discord source; Steam's detectable.json-enriched name beats
/// GameMode's executable stem (a curated database outranks a filename);
/// GameMode owns the machine-facing fields (executable).
/// `since` prefers Discord's game-reported timestamp, falling back to
/// GameMode's registration time. Exactly one partial may be `None`.
fn merge(
    pid: u32,
    gamemode: Option<&Activity>,
    discord: Option<&Activity>,
    steam: Option<&Activity>,
) -> Activity {
    let (gm, dc, st) = (gamemode.cloned(), discord.cloned(), steam.cloned());
    let mut merged = Activity::new(if gm.is_some() {
        format!("pid_{pid}")
    } else if dc.is_some() {
        format!("discord_{pid}")
    } else {
        format!("steam_{pid}")
    });

    merged.process_id = pid;
    if gm.is_some() {
        merged.sources.push(Source::GameMode);
    }
    if dc.is_some() {
        merged.sources.push(Source::Discord);
    }
    if st.is_some() {
        merged.sources.push(Source::Steam);
    }

    merged.kind = if [gm.as_ref(), dc.as_ref(), st.as_ref()]
        .into_iter()
        .flatten()
        .any(|a| a.kind == Kind::Game)
    {
        Kind::Game
    } else if [gm.as_ref(), dc.as_ref(), st.as_ref()]
        .into_iter()
        .flatten()
        .any(|a| a.kind == Kind::App)
    {
        Kind::App
    } else {
        Kind::Unknown
    };

    merged.name = dc
        .as_ref()
        .map(|d| d.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| {
            st.as_ref()
                .map(|s| s.name.clone())
                .filter(|n| !n.is_empty())
        })
        .or_else(|| {
            gm.as_ref()
                .map(|g| g.name.clone())
                .filter(|n| !n.is_empty())
        })
        .unwrap_or_default();

    // Discord-only fields (GameMode never sets them).
    if let Some(d) = &dc {
        merged.details = d.details.clone();
        merged.state = d.state.clone();
        merged.until = d.until;
        merged.large_image = d.large_image.clone();
        merged.large_text = d.large_text.clone();
        merged.small_image = d.small_image.clone();
        merged.small_text = d.small_text.clone();
        merged.party_size = d.party_size;
        merged.party_max = d.party_max;
    }

    // GameMode-only fields (Discord never sets them).
    if let Some(g) = &gm {
        merged.executable = g.executable.clone();
    }

    merged.since = [dc.as_ref(), gm.as_ref()]
        .into_iter()
        .flatten()
        .map(|a| a.since)
        .find(|&s| s != 0)
        .unwrap_or(0);

    if let Some(g) = &gm {
        merged.app_ids.extend(g.app_ids.clone());
        merged.extra.extend(g.extra.clone());
    }
    if let Some(d) = &dc {
        merged.app_ids.extend(d.app_ids.clone());
        merged.extra.extend(d.extra.clone());
    }
    if let Some(s) = &st {
        merged.app_ids.extend(s.app_ids.clone());
        merged.extra.extend(s.extra.clone());
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gamemode(pid: u32) -> Activity {
        Activity::from_gamemode(pid as i32, "/games/eldenring.exe", 1_700_000_100)
    }

    fn steam(pid: u32) -> Activity {
        Activity::from_steam(pid as i32, "480")
    }

    fn discord(pid: u32) -> Activity {
        let mut a = Activity::new(format!("discord_{pid}"));
        a.sources = vec![Source::Discord];
        a.kind = Kind::Game;
        a.name = "Elden Ring".to_string();
        a.details = "Limgrave".to_string();
        a.state = "Exploring".to_string();
        a.process_id = pid;
        a.app_ids
            .insert("discord".to_string(), "client-9".to_string());
        a.since = 1_700_000_000;
        a.party_size = 2;
        a
    }

    #[test]
    fn solo_discord_then_gamemode_is_absorbed() {
        let mut c = Correlator::new();
        let effects = c.on_updated(discord(42));
        assert_eq!(
            effects,
            vec![Effect::PublishNew(discord(42))],
            "solo discord publishes under its own id"
        );
        assert_eq!(effects[0], Effect::PublishNew(discord(42)));

        let effects = c.on_updated(gamemode(42));
        let [Effect::Remove(old), Effect::PublishNew(merged)] = effects.as_slice() else {
            panic!("expected absorb effects, got {effects:?}");
        };
        assert_eq!(old, "discord_42");
        assert_eq!(merged.id, "pid_42");
        assert_eq!(merged.sources, vec![Source::GameMode, Source::Discord]);
        assert_eq!(merged.name, "Elden Ring");
        assert_eq!(merged.details, "Limgrave");
        assert_eq!(merged.executable, "/games/eldenring.exe");
        assert_eq!(merged.app_ids.get("discord").unwrap(), "client-9");
        // Discord's game-reported timestamp wins over GameMode's.
        assert_eq!(merged.since, 1_700_000_000);
        assert_eq!(merged.party_size, 2);
        assert_eq!(merged.kind, Kind::Game);
    }

    #[test]
    fn solo_gamemode_then_discord_merges_in_place() {
        let mut c = Correlator::new();
        let effects = c.on_updated(gamemode(7));
        assert!(matches!(effects[0], Effect::PublishNew(_)));
        assert_eq!(effects[0], Effect::PublishNew(gamemode(7)));

        let effects = c.on_updated(discord(7));
        let [Effect::UpdateInPlace(merged)] = effects.as_slice() else {
            panic!("expected in-place merge, got {effects:?}");
        };
        assert_eq!(merged.id, "pid_7");
        assert_eq!(merged.sources, vec![Source::GameMode, Source::Discord]);
        assert_eq!(merged.name, "Elden Ring");
    }

    #[test]
    fn losing_discord_degrades_but_record_survives() {
        let mut c = Correlator::new();
        c.on_updated(gamemode(9));
        c.on_updated(discord(9));

        let effects = c.on_removed("discord_9", Source::Discord);
        let [Effect::UpdateInPlace(degraded)] = effects.as_slice() else {
            panic!("expected degrade-in-place, got {effects:?}");
        };
        assert_eq!(degraded.id, "pid_9");
        assert_eq!(degraded.sources, vec![Source::GameMode]);
        assert_eq!(degraded.name, "eldenring");
        assert_eq!(degraded.details, "");
        assert_eq!(degraded.executable, "/games/eldenring.exe");
        // GameMode's registration time returns as the since value.
        assert_eq!(degraded.since, 1_700_000_100);
    }

    #[test]
    fn losing_gamemode_splits_back_to_discord_id() {
        let mut c = Correlator::new();
        c.on_updated(gamemode(11));
        c.on_updated(discord(11));

        let effects = c.on_removed("pid_11", Source::GameMode);
        let [Effect::Remove(old), Effect::PublishNew(solo)] = effects.as_slice() else {
            panic!("expected split effects, got {effects:?}");
        };
        assert_eq!(old, "pid_11");
        assert_eq!(solo.id, "discord_11");
        assert_eq!(solo.sources, vec![Source::Discord]);
        assert_eq!(solo.name, "Elden Ring");
        assert_eq!(solo.executable, "");
    }

    #[test]
    fn record_dies_with_the_last_source() {
        let mut c = Correlator::new();
        c.on_updated(discord(13));
        let effects = c.on_removed("discord_13", Source::Discord);
        assert_eq!(effects, vec![Effect::Remove("discord_13".to_string())]);
        // Further removals for the same id are no-ops.
        assert!(c.on_removed("discord_13", Source::Discord).is_empty());
    }

    #[test]
    fn different_pids_never_join() {
        let mut c = Correlator::new();
        let e1 = c.on_updated(gamemode(100));
        let e2 = c.on_updated(discord(200));
        assert_eq!(e1.len(), 1);
        assert_eq!(e2.len(), 1);
        assert!(matches!(e2[0], Effect::PublishNew(_)));
        // SourceLost removes only that source's records.
        let effects = c.on_source_lost(Source::GameMode);
        assert_eq!(effects, vec![Effect::Remove("pid_100".to_string())]);
        let effects = c.on_source_lost(Source::Discord);
        assert_eq!(effects, vec![Effect::Remove("discord_200".to_string())]);
    }

    #[test]
    fn updates_refresh_in_place_without_id_churn() {
        let mut c = Correlator::new();
        c.on_updated(discord(21));
        c.on_updated(gamemode(21));
        let mut updated = discord(21);
        updated.state = "Boss: Margit".to_string();
        let effects = c.on_updated(updated);
        let [Effect::UpdateInPlace(merged)] = effects.as_slice() else {
            panic!("expected in-place update, got {effects:?}");
        };
        assert_eq!(merged.id, "pid_21");
        assert_eq!(merged.state, "Boss: Margit");
    }

    #[test]
    fn steam_merges_with_gamemode_and_discord() {
        let mut c = Correlator::new();
        c.on_updated(gamemode(50));
        c.on_updated(discord(50));

        let effects = c.on_updated(steam(50));
        let [Effect::UpdateInPlace(merged)] = effects.as_slice() else {
            panic!("expected in-place merge, got {effects:?}");
        };
        assert_eq!(merged.id, "pid_50");
        assert_eq!(
            merged.sources,
            vec![Source::GameMode, Source::Discord, Source::Steam]
        );
        assert_eq!(merged.app_ids.get("steam").unwrap(), "480");
        assert_eq!(merged.app_ids.get("discord").unwrap(), "client-9");
        assert_eq!(merged.name, "Elden Ring");
        assert_eq!(merged.executable, "/games/eldenring.exe");
    }

    #[test]
    fn steam_solo_publishes_under_own_id() {
        let mut c = Correlator::new();
        let effects = c.on_updated(steam(60));
        assert_eq!(effects, vec![Effect::PublishNew(steam(60))]);
        assert_eq!(effects[0], Effect::PublishNew(steam(60)));
    }

    #[test]
    fn steam_removal_degrades_but_record_survives() {
        let mut c = Correlator::new();
        c.on_updated(gamemode(70));
        c.on_updated(steam(70));

        let effects = c.on_removed("steam_70", Source::Steam);
        let [Effect::UpdateInPlace(degraded)] = effects.as_slice() else {
            panic!("expected degrade-in-place, got {effects:?}");
        };
        assert_eq!(degraded.id, "pid_70");
        assert_eq!(degraded.sources, vec![Source::GameMode]);
        assert!(!degraded.app_ids.contains_key("steam"));
        assert_eq!(degraded.executable, "/games/eldenring.exe");
    }

    #[test]
    fn steam_only_record_dies_with_steam() {
        let mut c = Correlator::new();
        c.on_updated(steam(80));
        let effects = c.on_removed("steam_80", Source::Steam);
        assert_eq!(effects, vec![Effect::Remove("steam_80".to_string())]);
    }

    #[test]
    fn steam_name_beats_gamemode_executable_stem() {
        // Wrapper-process case: GameMode registers a bash wrapper,
        // Steam enrichment finds the real game name via detectable.json.
        let mut c = Correlator::new();
        c.on_updated(gamemode(60)); // name = "eldenring" (executable stem)

        let mut steam_named = steam(60);
        steam_named.name = "Elden Ring".to_string(); // detectable.json enriched

        let effects = c.on_updated(steam_named);
        let [Effect::UpdateInPlace(merged)] = effects.as_slice() else {
            panic!("expected in-place merge, got {effects:?}");
        };
        assert_eq!(merged.id, "pid_60");
        assert_eq!(merged.name, "Elden Ring"); // Steam's curated name wins
        assert_eq!(merged.executable, "/games/eldenring.exe"); // GameMode still owns executable
    }

    #[test]
    fn steam_name_falls_back_to_gamemode_when_empty() {
        // Steam has no detectable.json match (name = ""): GameMode's
        // executable stem is the fallback.
        let mut c = Correlator::new();
        c.on_updated(gamemode(61));
        c.on_updated(steam(61)); // name = "" (from_steam)

        let effects = c.on_updated(steam(61)); // re-update to trigger merge
        let [Effect::UpdateInPlace(merged)] = effects.as_slice() else {
            panic!("expected in-place merge, got {effects:?}");
        };
        assert_eq!(merged.name, "eldenring"); // GameMode's stem is the fallback
    }

    #[test]
    fn pid_zero_passes_through_unjoined() {
        let mut c = Correlator::new();
        let mut a = discord(0);
        a.process_id = 0;
        let effects = c.on_updated(a.clone());
        assert_eq!(effects, vec![Effect::UpdateInPlace(a)]);
    }
}
