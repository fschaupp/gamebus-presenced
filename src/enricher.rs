//! The Enricher: middleware between sources and the correlator.
//!
//! Sources emit raw [`SourceEvent`]s; the Enricher processes each event and
//! may emit additional enriched events (e.g. a Steam partial derived from
//! `/proc/<pid>/environ`). The correlator receives the enriched stream.
//!
//! ```text
//! sources → SourceEvent → Enricher → Vec<SourceEvent> → Correlator
//!                            │
//!                            ├─ steam::probe(pid) on new pid arrival
//!                            ├─ detectable.json naming lookup
//!                            └─ (future) other launcher enrichment
//! ```
//!
//! ## Steam partial lifecycle
//!
//! Steam has no watcher and no removal signal. The Enricher tracks which
//! non-Steam sources are active per pid (`{pid: Set<Source>}`). When the
//! last non-Steam source removes its record for a pid, the Enricher emits
//! `SourceEvent::Removed` for Steam - preventing stale Steam data from
//! surviving a pid reuse.

use crate::cache;
use crate::dbus::types::{Activity, Source};
use crate::group::{GameGroup, GroupEffect, Identity, IdentityClass, Member, MemberClass};
use crate::naming::{is_wrapper_executable, NamingDb};
use crate::sources::SourceEvent;
use crate::umu_report::{self, Confidence, UmuReport};
use std::collections::{HashMap, HashSet};

/// Maximum ppid-chain depth for the ancestor walk.
/// A game launcher tree is typically 3-5 levels deep (supervisor → reaper →
/// srt-bwrap → pv-adverb → game). 10 is generous.
const MAX_ANCESTOR_DEPTH: usize = 10;

/// Maximum depth for the descendant walk.
/// The wrapper tree can be deep (lutris-wrapper → umu-run → steam-runtime-l →
/// bwrap → bwrap → pv-adverb → python3 → steam.exe → game). 12 is generous.
const MAX_DESCENDANT_DEPTH: usize = 12;

/// Maximum breadth for the descendant walk.
/// A wrapper tree can have many children (Battle.net spawns dozens of CEF
/// renderers). We only care about the game process, so we limit the total
/// number of processes visited.
const MAX_DESCENDANT_BREADTH: usize = 64;

/// Where a group identity's name came from (S9c title provenance). A tag
/// BESIDE [`IdentityClass`], never part of it: provenance must not change
/// the election order, only the stash's `title_source` label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentitySource {
    /// A curated record named it: detectable.json, or a launcher's own
    /// install records (the Heroic library title).
    Curated,
    /// The walk resolved the actual game process (cmdline, descendant tree,
    /// sandbox family).
    Walk,
    /// The `lutris-wrapper` ancestor's argv named it (layer 4) — Lutris
    /// telling us what it launched, one rung below a walk hit.
    LutrisArgv,
    /// Steam's own appmanifest named it — the official title of the
    /// installed appid, curated-grade but distinct so downstream labels
    /// never claim detectable.json knew a game it did not.
    SteamManifest,
}

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
    /// pid → the Steam appid found in its environ (ancestor-walk).
    steam_appids: HashMap<u32, String>,
    /// merge key → game group.
    ///
    /// One published record per key: `steam:<appid>` for Steam games,
    /// `lutris:<uuid>` for Lutris games, `umu:<id>` for umu games. The group
    /// tracks every member pid and decides which one - the representative -
    /// carries the record; everyone else is absorbed silently.
    groups: HashMap<String, GameGroup>,
    /// member pid → its group's merge key (reverse index).
    pid_to_group: HashMap<u32, String>,
    /// Keys whose published record is Steam-only (created by the scan, no
    /// GameMode partial on the bus). Their migrations and removals move only
    /// the Steam partial; a GameMode event upgrades the record and clears
    /// the flag.
    steam_only_groups: HashSet<String>,
    /// Wrapper pids whose game identity hasn't been resolved yet.
    /// Retried periodically - the game may launch minutes after the wrapper.
    unresolved_wrappers: HashSet<u32>,
    /// Naming database for detectable.json lookups.
    /// `None` if no database file was found - naming enrichment is disabled.
    naming: Option<NamingDb>,
    /// umu-database misses and their resolutions: raw material for a
    /// user-reviewed submission upstream. In-memory no-op until
    /// [`load_umu_report`] attaches the on-disk stash.
    umu_report: UmuReport,
    /// merge key → (store guess, codename, stable entry fallback) for keys
    /// whose launch went into the identity-miss stash, so later title
    /// resolutions can find their stash entry. The fallback may be a
    /// GAME_NAME slug rather than the per-launch merge key (S9c), which is
    /// what collapses repeated per-uuid Lutris launches onto one entry.
    stash_keys: HashMap<String, (String, Option<String>, String)>,
    /// merge key → provenance of the group's CURRENT identity (S9c). Kept in
    /// lockstep with [`GameGroup::set_identity`]'s monotone rule: recorded
    /// only when the offered identity was actually adopted, so the tag
    /// always describes the identity the group holds. Never consulted for
    /// election — only for the stash's `title_source` label.
    identity_sources: HashMap<String, IdentitySource>,
    /// Ungrouped GameMode records withheld from the bus because nothing has
    /// named them yet. The monitor would show "(unknown)" - instead the
    /// bus sees nothing until any evidence names the record, which also makes
    /// the µs-lived keyless-helper corpses fully silent. Stored whole so the
    /// authoritative `since` survives until publication.
    withheld: HashMap<u32, Activity>,
    /// pid → MPRIS player Identity. The weakest naming evidence: fills a
    /// name only when Discord, the group identity, and detectable.json have
    /// all left it empty. Pruned for dead pids on the tick; applied names are
    /// monotone, so pruning never un-names a record.
    name_hints: HashMap<u32, String>,
}

impl Enricher {
    /// Create an Enricher without a naming database.
    /// Call [`load_naming`] after sources are spawned to avoid blocking startup.
    pub fn new() -> Self {
        Self {
            active_sources: HashMap::new(),
            steam_ids: HashMap::new(),
            steam_appids: HashMap::new(),
            groups: HashMap::new(),
            pid_to_group: HashMap::new(),
            steam_only_groups: HashSet::new(),
            unresolved_wrappers: HashSet::new(),
            naming: None,
            name_hints: HashMap::new(),
            withheld: HashMap::new(),
            umu_report: UmuReport::default(),
            stash_keys: HashMap::new(),
            identity_sources: HashMap::new(),
        }
    }

    /// Create an Enricher with a specific naming database (for testing).
    #[cfg(test)]
    fn with_naming(naming: Option<NamingDb>) -> Self {
        Self {
            naming,
            ..Self::new()
        }
    }

    /// Attach the persistent umu-miss stash. Called from main after
    /// sources spawn; tests keep the in-memory default so they never touch
    /// the user's data.
    pub fn load_umu_report(&mut self) {
        self.umu_report = UmuReport::load();
        if let Some(e) = self.umu_report.load_error() {
            // The stash keeps the accumulated umu-miss knowledge; refusing
            // to write over an unreadable file is the report's job, saying
            // so out loud is ours.
            tracing::warn!(
                error = e,
                "umu-miss stash unreadable; not recording misses this session"
            );
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
            SourceEvent::NameHint { pid, name } => self.on_name_hint(pid, name),
        }
    }

    /// Store an MPRIS naming hint and, when it improves an already
    /// published group record, refresh that record in place.
    ///
    /// Swallows the event - hints never reach the correlator. Records that
    /// are not refreshed here pick the hint up on their next `Updated`, at
    /// the latest via the 15s ListGames reseed.
    fn on_name_hint(&mut self, pid: u32, name: String) -> Vec<SourceEvent> {
        if name.is_empty() {
            return Vec::new();
        }
        self.name_hints.insert(pid, name.clone());

        // A withheld record publishes the moment a hint names it - with
        // its original, authoritative `since`.
        if let Some(mut w) = self.withheld.remove(&pid) {
            w.name = name;
            return vec![SourceEvent::Updated(Box::new(w))];
        }

        // A hint can arrive after the record published under a stem-cleared
        // (empty) name. If the hinted pid belongs to a group whose identity
        // is unresolved, re-emit the representative so the name lands now
        // rather than a reseed later.
        let key = match self.pid_to_group.get(&pid) {
            Some(k) => k.clone(),
            None => match probe_merge_key(pid) {
                Some(k) if self.groups.contains_key(&k) => k,
                _ => return Vec::new(),
            },
        };
        let Some(group) = self.groups.get(&key) else {
            return Vec::new();
        };
        if group.identity.is_some() {
            // A resolved identity always outranks a hint.
            return Vec::new();
        }
        let rep = group.rep;
        let since = group.since;
        let Some(hint) = self.group_hint(&key) else {
            return Vec::new();
        };

        // Rebuild the representative's activity the same way the reseed
        // would; apply_naming leaves the name empty (no identity, no
        // detectable hit - that is why we are here), then the hint fills it.
        let executable = std::fs::read_link(format!("/proc/{rep}/exe"))
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let mut activity = Activity::from_gamemode(rep as i32, &executable, since);
        self.apply_naming(&mut activity);
        if !name_is_default(&activity) {
            // Something better than a stem appeared meanwhile; nothing to do.
            return Vec::new();
        }
        activity.name = hint;
        vec![SourceEvent::Updated(Box::new(activity))]
    }

    /// The best hint for a group: the representative's own, else any member's.
    fn group_hint(&self, key: &str) -> Option<String> {
        let group = self.groups.get(key)?;
        if let Some(h) = self.name_hints.get(&group.rep) {
            return Some(h.clone());
        }
        group
            .members
            .keys()
            .find_map(|pid| self.name_hints.get(pid))
            .cloned()
    }

    fn on_updated(&mut self, activity: Activity) -> Vec<SourceEvent> {
        let pid = activity.process_id;
        let source = activity.sources.first().copied().unwrap_or(Source::Unknown);

        // Track non-Steam sources per pid.
        if source != Source::Steam && pid > 0 {
            self.active_sources.entry(pid).or_default().insert(source);
        }

        // Instrumentation: log when a Discord pid has no GameMode
        // registration - a join-miss that the ancestor-walk would fix.
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

        // Capture the raw `/proc/<pid>/exe` BEFORE the descendant walk
        // rewrites `activity.executable` - member classification (helper
        // detection) must judge the process itself, not the identified game.
        let raw_exe = if pid > 0 {
            std::fs::read_link(format!("/proc/{pid}/exe"))
                .ok()
                .and_then(|p| p.to_str().map(str::to_string))
                .unwrap_or_else(|| activity.executable.clone())
        } else {
            activity.executable.clone()
        };

        // Descendant-walk - if the executable is a wrapper, look for
        // the actual game process in the wrapper tree and use its
        // name/executable instead.
        let mut activity = activity;
        let identified = self.apply_descendant_walk(&mut activity);

        // Naming enrichment - modify the activity's name from
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

        // Every GameMode pid that probes to a merge key joins that
        // key's group; the group decides what (if anything) reaches the
        // correlator. Pids with no key follow the ungrouped path, untouched.
        if source == Source::GameMode && pid > 0 {
            let environ =
                std::fs::read_to_string(format!("/proc/{pid}/environ")).unwrap_or_default();
            if let Some(key) = merge_key_from_environ(&environ) {
                // A launch that went through umu without a database entry, or
                // that a launcher handed us without an authoritative store
                // identity, is a gap worth recording - together with
                // whatever this daemon later works out about it.
                self.maybe_stash_launch(&key, &environ, &raw_exe);
                let (class, mut identity) =
                    self.classify_member(pid, &raw_exe, &key, identified.as_ref());
                // A Heroic group whose members prove nothing themselves
                // is still nameable - the launcher's own install records map
                // the store codename to the display title. Launcher-curated,
                // Wrapper class: a detectable.json hit on the real game
                // process still upgrades it.
                if identity.is_none() && self.groups.get(&key).is_none_or(|g| g.identity.is_none())
                {
                    if let Some(app) = key.strip_prefix("heroic:") {
                        if let Some(title) = heroic_title(app) {
                            if let Some((store, code, fallback)) =
                                self.stash_keys.get(&key).cloned()
                            {
                                self.umu_report.note_title(
                                    &store,
                                    code.as_deref(),
                                    &fallback,
                                    &title,
                                    "heroic-config",
                                    Confidence::High,
                                    None,
                                );
                            }
                            identity = Some((
                                Identity {
                                    name: title,
                                    exe: raw_exe.clone(),
                                    class: IdentityClass::Wrapper,
                                },
                                IdentitySource::Curated,
                            ));
                        }
                    }
                }
                let member = Member {
                    gamemode: true,
                    scan: false,
                    class,
                    depth: tree_depth(pid),
                    start_time: cache::process_start_time(pid),
                    alive: true,
                };
                return self.grouped_update(&key, pid, member, identity, activity, steam_activity);
            }
        }

        // Publish-once-named. An ungrouped GameMode record whose name is
        // empty after all enrichment - a known wrapper whose stem was
        // cleared, or an unreadable exe - is withheld rather than published
        // as "(unknown)". It publishes the moment anything names it (hint,
        // late identification, Discord, or the 15s reseed after the naming
        // DB resolves). Grouped records are never withheld: a merge key is
        // game evidence in itself. Stem names still publish - pid+executable
        // presence is the GameMode source's contract.
        if source == Source::GameMode
            && pid > 0
            && activity.name.is_empty()
            && steam_activity.is_none()
            && !self
                .active_sources
                .get(&pid)
                .is_some_and(|s| s.contains(&Source::Discord))
        {
            tracing::debug!(pid, exe = %activity.executable, "withholding nameless record");
            self.withheld.insert(pid, activity);
            return Vec::new();
        }
        // Ungrouped path: forward the original event and the Steam partial.
        let mut events = Vec::with_capacity(3);
        // A withheld GameMode record flushes FIRST when evidence arrives for
        // its pid: Discord names the merge, so the bus sees one
        // PublishNew(pid_<pid>) followed by the joining update - never an
        // absorb pair, and never a silently dropped record.
        if let Some(w) = self.withheld.remove(&pid) {
            if source != Source::GameMode {
                events.push(SourceEvent::Updated(Box::new(w)));
            }
            // A GameMode event for the pid IS the withheld record's
            // successor - superseded, not flushed.
        }
        events.push(SourceEvent::Updated(Box::new(activity)));
        if let Some(sa) = steam_activity {
            let steam_id = sa.id.clone();
            self.steam_ids.insert(pid, steam_id);
            self.steam_appids.insert(pid, appid_from(&sa));
            events.push(SourceEvent::Updated(Box::new(sa)));
        }

        events
    }

    /// Decide whether this launch belongs in the identity-miss stash and
    /// record it with everything the launcher said about it. Recorded:
    /// launches umu ran without a database entry (the `umu-0`/`umu-default`
    /// marker), and launcher-keyed launches (`lutris:`/`heroic:`) - a
    /// launcher handing us a process without an authoritative store
    /// identity. Never `steam:` keys and never curated `umu:<id>` launches
    /// without the marker: their identity is authoritative.
    fn maybe_stash_launch(&mut self, key: &str, environ: &str, raw_exe: &str) {
        let missed_id = umu_miss_id(environ);
        let is_lutris = key.starts_with("lutris:");
        let is_heroic = key.starts_with("heroic:");
        // A steam-keyed launch is recorded only when it is NOT obvious: the
        // appid is absent from detectable.json (so the mapping is knowledge
        // gamebus-gamedb lacks) yet Steam's own manifest names the install
        // (so nothing here is guessed). Shortcut appids have no manifest and
        // never qualify; a game detectable knows never qualifies either, so
        // ordinary Steam launches leave no trace.
        let steam_gap = key.strip_prefix("steam:").and_then(|appid| {
            let db = self.naming.as_ref()?;
            if db.lookup_by_steam_appid(appid).is_some() {
                return None;
            }
            let name = steam_manifest_name(raw_exe, appid)?;
            Some((appid.to_string(), name))
        });
        if missed_id.is_none() && !is_lutris && !is_heroic && steam_gap.is_none() {
            return;
        }
        if let Some((appid, name)) = steam_gap {
            self.umu_report.note_launch_with(
                "steam",
                Some(&appid),
                "",
                key,
                umu_report::LaunchFacts {
                    launcher: Some("steam".to_string()),
                    launcher_name: Some(name),
                    launcher_dir: None,
                    codename_source: Some("steam-manifest".to_string()),
                    runner: Some(runner_of(false, raw_exe).to_string()),
                },
            );
            self.stash_keys.insert(
                key.to_string(),
                ("steam".to_string(), Some(appid), key.to_string()),
            );
            return;
        }

        let store_env = env_value(environ, "STORE");
        let game_name = env_value(environ, "GAME_NAME");
        let game_dir = env_value(environ, "GAME_DIRECTORY");
        let heroic_source = env_value(environ, "HEROIC_APP_SOURCE");
        let heroic_app = env_value(environ, "HEROIC_APP_NAME");
        let store =
            umu_report::guess_store(store_env.as_deref(), heroic_source.as_deref(), raw_exe);

        // Codename: Lutris writes one beside the game (itch.io installs);
        // Heroic carries one in the environment of every launch.
        let (codename, codename_source) = if is_lutris {
            match game_dir.as_deref().and_then(lutris_marker_appid) {
                Some(appid) => (Some(appid), Some("lutris-config")),
                None => (None, None),
            }
        } else {
            match &heroic_app {
                Some(app) => (Some(app.clone()), Some("heroic-env")),
                None => (None, None),
            }
        };

        // Stable stash key when no codename exists: the GAME_NAME slug for
        // Lutris (the merge key's uuid is per-launch and would fragment the
        // stash), the merge key itself otherwise. Never an exe basename —
        // every Wine launch would collapse onto wine64-preloader.
        let fallback = if is_lutris {
            match game_name.as_deref().map(slug).filter(|s| !s.is_empty()) {
                Some(s) => format!("lutris:{s}"),
                None => key.to_string(),
            }
        } else {
            key.to_string()
        };

        let launcher = if is_lutris {
            Some("lutris")
        } else if is_heroic {
            Some("heroic")
        } else {
            None
        };
        let launcher_name = if is_lutris {
            game_name
        } else {
            // The Heroic library resolves the codename to a display title.
            heroic_app.as_deref().and_then(heroic_title)
        };

        self.umu_report.note_launch_with(
            &store,
            codename.as_deref(),
            missed_id.unwrap_or(""),
            &fallback,
            umu_report::LaunchFacts {
                launcher: launcher.map(str::to_string),
                launcher_name,
                launcher_dir: game_dir,
                codename_source: codename_source.map(str::to_string),
                runner: Some(runner_of(missed_id.is_some(), raw_exe).to_string()),
            },
        );
        self.stash_keys
            .insert(key.to_string(), (store, codename, fallback));
    }

    /// Classify a group member and derive any identity it proves.
    ///
    /// `GameProcess`: `identify_process` hit, or exe under `/steamapps/`
    /// with a resolvable Steam appid. `IdentifiedWrapper`: the descendant
    /// walk resolved the game through this pid. `Helper`: known wrapper
    /// executable, judged on the RAW exe. `Plain`: everything else.
    /// Every identity comes tagged with its [`IdentitySource`].
    fn classify_member(
        &self,
        pid: u32,
        raw_exe: &str,
        key: &str,
        identified: Option<&(String, String, IdentitySource)>,
    ) -> (MemberClass, Option<(Identity, IdentitySource)>) {
        if let Some(ref db) = self.naming {
            // A known wrapper's cmdline carries the full launch command,
            // game binary included (`reaper SteamLaunch ... /path/Game`),
            // so the cmdline layer would inflate the helper to GameProcess
            // and block the real game's dethrone (helpers are judged
            // on the raw exe). Wrappers only count an exe-link match; Wine
            // games are unaffected - their exe is wine64-preloader, which
            // is not in the wrapper list, so they keep the cmdline layer.
            let hit = if is_wrapper_executable(raw_exe) {
                identify_process_exe(pid, db)
            } else {
                identify_process(pid, db)
            };
            if let Some((name, exe)) = hit {
                return (
                    MemberClass::GameProcess,
                    Some((
                        Identity {
                            name,
                            exe,
                            class: IdentityClass::GameProcess,
                        },
                        IdentitySource::Curated,
                    )),
                );
            }
            if raw_exe.contains("/steamapps/") {
                if let Some(name) = key
                    .strip_prefix("steam:")
                    .and_then(|appid| db.lookup_by_steam_appid(appid))
                {
                    return (
                        MemberClass::GameProcess,
                        Some((
                            Identity {
                                name: name.to_string(),
                                exe: raw_exe.to_string(),
                                class: IdentityClass::GameProcess,
                            },
                            IdentitySource::Curated,
                        )),
                    );
                }
            }
        }
        if let Some((name, exe, source)) = identified {
            return (
                MemberClass::IdentifiedWrapper,
                Some((
                    Identity {
                        name: name.clone(),
                        exe: exe.clone(),
                        class: IdentityClass::Wrapper,
                    },
                    *source,
                )),
            );
        }
        if is_wrapper_executable(raw_exe) {
            (MemberClass::Helper, None)
        } else {
            // Steam names its own installs: for a steam-keyed member whose
            // exe runs out of a steamapps library, the appmanifest beside it
            // carries the official title — authoritative even when
            // detectable.json has never heard of the game (observed live
            // 2026-08-30: Danger Scavenger, a real Steam app absent from
            // detectable, published nothing at all). The member stays Plain;
            // the identity is GameProcess-class because Steam's own record
            // for the running appid is as curated as it gets.
            if let Some(name) = key
                .strip_prefix("steam:")
                .and_then(|appid| steam_manifest_name(raw_exe, appid))
            {
                return (
                    MemberClass::Plain,
                    Some((
                        Identity {
                            name,
                            exe: raw_exe.to_string(),
                            class: IdentityClass::GameProcess,
                        },
                        IdentitySource::SteamManifest,
                    )),
                );
            }
            // A game binary Lutris launched directly — a native Linux game,
            // `gamemoderun ./Game.x86_64` — registers with GameMode itself,
            // so no wrapper pid ever runs the ancestor layer on its behalf
            // (`apply_descendant_walk` is wrappers-only) and the record kept
            // the executable stem. Ask the lutris-wrapper ancestor here. The
            // member stays Plain (this changes no election) and the identity
            // is launcher-class, so a curated hit still replaces it:
            // set_identity is monotone by class.
            let identity = identify_via_lutris_ancestor(pid).map(|(name, exe)| {
                (
                    Identity {
                        name,
                        exe,
                        class: IdentityClass::Wrapper,
                    },
                    IdentitySource::LutrisArgv,
                )
            });
            (MemberClass::Plain, identity)
        }
    }

    /// Route one member arrival through its group and translate
    /// the [`GroupEffect`] into correlator events. Used by the GameMode event
    /// path (`member.gamemode == true`) and by scan adoption (`scan == true`).
    fn grouped_update(
        &mut self,
        key: &str,
        pid: u32,
        mut member: Member,
        identity: Option<(Identity, IdentitySource)>,
        mut activity: Activity,
        steam_activity: Option<Activity>,
    ) -> Vec<SourceEvent> {
        let via_gamemode = member.gamemode;

        // Discord pin: a rep carrying a joined Discord partial
        // is displaced by rep death only, never by class.
        let rep_pinned = self.groups.get(key).is_some_and(|g| {
            self.active_sources
                .get(&g.rep)
                .is_some_and(|s| s.contains(&Source::Discord))
        });

        let group = self
            .groups
            .entry(key.to_string())
            .or_insert_with(|| GameGroup::new(key, activity.since));
        self.pid_to_group.insert(pid, key.to_string());
        if let Some((id, source)) = identity {
            // Mirror set_identity's monotone rule (spec §1.3) so the
            // provenance tag always describes the identity the group holds.
            let adopted = match &group.identity {
                None => true,
                Some(current) => id.class > current.class,
            };
            group.set_identity(id);
            if adopted {
                self.identity_sources.insert(key.to_string(), source);
            }
        }
        // The rep carries the group's one Steam partial; remember the appid
        // so it can move with the record on migration.
        if group.steam_appid.is_none() {
            if let Some(appid) = key.strip_prefix("steam:") {
                group.steam_appid = Some(appid.to_string());
            } else if let Some(sa) = &steam_activity {
                let appid = appid_from(sa);
                if !appid.is_empty() {
                    group.steam_appid = Some(appid);
                }
            }
        }
        // Scan evidence survives re-registration.
        if let Some(existing) = group.members.get(&pid) {
            member.scan = member.scan || existing.scan;
        }
        // A scan-created group was born blind (`since` 0): the
        // first GameMode evidence supplies the authoritative timestamp. A
        // 0→real transition is still set-once, so `Since` can never jump.
        if via_gamemode && group.since == 0 {
            group.since = activity.since;
        }
        // The published record's name/exe come from the group identity when
        // set; `Since` is pinned at group creation and never jumps.
        activity.since = group.since;
        if let Some(id) = &group.identity {
            activity.name = id.name.clone();
            activity.executable = id.exe.clone();
        }
        // With no identity and no curated name, any member's MPRIS hint
        // beats the stem. Group lookup runs after the member upsert below is
        // reflected in `pid_to_group`, so use the members map directly.
        if group.identity.is_none() && name_is_default(&activity) {
            let hint = self
                .name_hints
                .get(&pid)
                .or_else(|| group.members.keys().find_map(|m| self.name_hints.get(m)))
                .or_else(|| self.name_hints.get(&group.rep));
            if let Some(h) = hint {
                activity.name = h.clone();
            }
        }
        let group_appid = group.steam_appid.clone();

        let effect = group.upsert(pid, member, rep_pinned);
        // S9: the stash mirrors the group's ELECTED identity, never a
        // member's raw claim. A claim that loses the election must not reach
        // the stash — the Unity crash handler resolved to another game and
        // overwrote a correct same-confidence title, while the published
        // record stayed right because set_identity is monotone. Noting the
        // group identity after routing hands that monotonicity to the stash.
        self.note_group_identity(key);
        match effect {
            GroupEffect::PublishRep => {
                if via_gamemode {
                    self.steam_only_groups.remove(key);
                }
                let mut events = Vec::with_capacity(2);
                events.push(SourceEvent::Updated(Box::new(activity)));
                if let Some(sa) = steam_activity {
                    self.steam_ids.insert(pid, sa.id.clone());
                    self.steam_appids.insert(pid, appid_from(&sa));
                    events.push(SourceEvent::Updated(Box::new(sa)));
                }
                events
            }
            GroupEffect::Absorb => {
                // The bus never sees this pid: drop the event, the Steam
                // probe, and the source bookkeeping.
                tracing::debug!(pid, merge_key = %key, "group: member absorbed");
                self.active_sources.remove(&pid);
                // One exception can already BE on the bus: a restart-cache
                // record adopted before sources spawned. Adoption publishes
                // without creating correlator partials, so when the reseed
                // then absorbs that pid into a group, the stale record would
                // linger beside the representative's - two records for one
                // game (observed live with Ubisoft Connect). The
                // removal is a no-op for the common case: a pid the
                // correlator never published produces no effect.
                vec![SourceEvent::Removed {
                    id: format!("pid_{pid}"),
                    source: Source::GameMode,
                }]
            }
            GroupEffect::Migrate { old } => {
                tracing::info!(
                    old,
                    new = pid,
                    merge_key = %key,
                    "group: higher-class member dethrones representative"
                );
                let sa = steam_activity.or_else(|| {
                    group_appid.map(|appid| {
                        let mut sa = Activity::from_steam(pid as i32, &appid);
                        self.apply_naming(&mut sa);
                        sa
                    })
                });
                // A Steam-only group stays Steam-only under scan-driven
                // migration; a GameMode arrival upgrades the record.
                let gamemode_activity = if via_gamemode || !self.steam_only_groups.contains(key) {
                    Some(activity)
                } else {
                    None
                };
                let events = self.emit_migration(key, old, pid, gamemode_activity, sa);
                if via_gamemode {
                    self.steam_only_groups.remove(key);
                }
                events
            }
            // `upsert` never returns RemoveAll.
            GroupEffect::RemoveAll => Vec::new(),
        }
    }

    /// Emit the publish-first migration sequence: `Updated(new rep)`,
    /// `Updated(new Steam partial)`, `Removed(old Steam id, Steam)`,
    /// `Removed(pid_<old>, GameMode)` - the bus never dips empty. Steam-only
    /// groups have no GameMode partial: their sequence is the Steam pair.
    ///
    /// Deliberately removes Steam first, not GameMode first, honoring the
    /// exactly-one-ActivityRemoved promise:
    /// removing the GameMode partial first would leave the old Steam partial
    /// briefly sole owner and re-publish it as a transient `steam_<old>`
    /// record before its own removal lands. Steam-first degrades the old
    /// record in place, then removes it once.
    fn emit_migration(
        &mut self,
        key: &str,
        old: u32,
        new_pid: u32,
        gamemode_activity: Option<Activity>,
        steam_activity: Option<Activity>,
    ) -> Vec<SourceEvent> {
        let was_steam_only = self.steam_only_groups.contains(key);
        let mut events = Vec::with_capacity(4);
        if let Some(a) = gamemode_activity {
            events.push(SourceEvent::Updated(Box::new(a)));
        }
        if let Some(sa) = steam_activity {
            self.steam_ids.insert(new_pid, sa.id.clone());
            self.steam_appids.insert(new_pid, appid_from(&sa));
            events.push(SourceEvent::Updated(Box::new(sa)));
        }
        if let Some(old_sid) = self.steam_ids.remove(&old) {
            events.push(SourceEvent::Removed {
                id: old_sid,
                source: Source::Steam,
            });
        }
        if !was_steam_only {
            events.push(SourceEvent::Removed {
                id: format!("pid_{old}"),
                source: Source::GameMode,
            });
        }
        self.steam_appids.remove(&old);
        // The dethroned rep is an absorbed member now - nothing may leak.
        self.active_sources.remove(&old);
        events
    }

    /// Apply game identification to an activity.
    ///
    /// When GameMode registers a wrapper process (env, bash, steam-runtime-l,
    /// etc.), the record's real identity comes from the actual game. Three
    /// layers, cheapest first:
    ///
    /// 1. **Wrapper cmdline** - the wrapper's own `/proc/<pid>/cmdline`
    ///    usually names the game at the end
    ///    (`... proton waitforexitandrun /path/Game.exe`).
    /// 2. **Descendant walk** - connected process trees (native Steam,
    ///    non-portal spawns): walk `/proc/*/task/*/children`, matching
    ///    exe links and Wine cmdlines.
    /// 3. **Sandbox-family scan** - Flatpak-portal spawns sever the tree
    ///    (Lutris-Flatpak + umu): all sandbox members share the umu
    ///    `var/tmp-XXXXXX` token in their cmdlines; scan `/proc` for it.
    ///
    /// If nothing is found, the pid is remembered in
    /// [`Self::unresolved_wrappers`] and retried periodically by
    /// [`Self::retry_unresolved`] - the game may launch minutes after the
    /// wrapper (Battle.net launcher → actual game).
    /// Returns the `(name, exe)` identification when the walk resolved the
    /// game through this pid - the group model records it as the member's
    /// [`IdentifiedWrapper`](MemberClass::IdentifiedWrapper) proof.
    fn apply_descendant_walk(
        &mut self,
        activity: &mut Activity,
    ) -> Option<(String, String, IdentitySource)> {
        // Only for GameMode activities with wrapper executables.
        if !activity.sources.contains(&Source::GameMode) {
            return None;
        }
        if activity.executable.is_empty() || !is_wrapper_executable(&activity.executable) {
            return None;
        }
        let pid = activity.process_id;
        if pid == 0 {
            return None;
        }

        // If the naming DB isn't loaded yet (it loads after sources spawn),
        // mark for retry instead of silently skipping.
        if self.naming.is_none() {
            self.unresolved_wrappers.insert(pid);
            return None;
        }

        match self.identify_wrapper(pid) {
            Some((name, exe, source)) => {
                tracing::info!(wrapper_pid = pid, game_name = %name, game_exe = %exe, "identified game for wrapper");
                activity.name = name.clone();
                activity.executable = exe.clone();
                self.unresolved_wrappers.remove(&pid);
                Some((name, exe, source))
            }
            None => {
                // Game may not have launched yet (Battle.net launcher → game
                // starts minutes later). Retry periodically.
                self.unresolved_wrappers.insert(pid);
                None
            }
        }
    }

    /// Periodic tick: retry wrapper identification + reconcile the game
    /// groups against `/proc`.
    pub fn tick(&mut self) -> Vec<SourceEvent> {
        // Drop hints whose player process is gone. Applied names are
        // monotone, so this never un-names a published record.
        self.name_hints
            .retain(|pid, _| std::path::Path::new(&format!("/proc/{pid}")).exists());
        // A withheld wrapper that died without an unregister (missed
        // signal) must not leak.
        self.withheld
            .retain(|pid, _| std::path::Path::new(&format!("/proc/{pid}")).exists());
        let mut events = self.retry_unresolved();
        events.extend(self.reconcile_groups());
        events
    }

    /// Retry identification for wrappers whose game hasn't been found yet.
    ///
    /// Called periodically from the main loop. Returns update events for
    /// wrappers that just became identifiable. For a **grouped** pid the
    /// identity belongs to the group: set it and re-emit the current rep's
    /// activity in place - never construct a fresh activity for the wrapper
    /// pid, which would publish a duplicate record.
    pub fn retry_unresolved(&mut self) -> Vec<SourceEvent> {
        let pids: Vec<u32> = self.unresolved_wrappers.iter().copied().collect();
        let mut out = Vec::new();
        for pid in pids {
            let Some((name, exe, source)) = self.identify_wrapper(pid) else {
                continue;
            };
            tracing::info!(wrapper_pid = pid, game_name = %name, game_exe = %exe, "identified game for wrapper (retry)");
            self.unresolved_wrappers.remove(&pid);
            match self.pid_to_group.get(&pid).cloned() {
                Some(key) => {
                    let Some(group) = self.groups.get_mut(&key) else {
                        continue;
                    };
                    let id = Identity {
                        name,
                        exe,
                        class: IdentityClass::Wrapper,
                    };
                    // Mirror set_identity's monotone rule (spec §1.3) so
                    // the provenance tag tracks the held identity.
                    let adopted = match &group.identity {
                        None => true,
                        Some(current) => id.class > current.class,
                    };
                    group.set_identity(id);
                    if adopted {
                        self.identity_sources.insert(key.clone(), source);
                    }
                    // The pid proved the group identity: upgrade its class.
                    if let Some(member) = group.members.get_mut(&pid) {
                        if member.class < MemberClass::IdentifiedWrapper {
                            member.class = MemberClass::IdentifiedWrapper;
                        }
                    }
                    let rep = group.rep;
                    self.note_group_identity(&key);
                    out.extend(self.emit_rep_refresh(&key, rep));
                }
                None => {
                    // A withheld record identified late publishes with
                    // its original `since`, not a fabricated zero.
                    let mut activity = match self.withheld.remove(&pid) {
                        Some(w) => w,
                        None => Activity::from_gamemode(pid as i32, &exe, 0),
                    };
                    activity.name = name;
                    if !exe.is_empty() {
                        activity.executable = exe.clone();
                    }
                    out.push(SourceEvent::Updated(Box::new(activity)));
                }
            }
        }
        out
    }

    /// S9: write the group's elected identity through to the umu-miss stash.
    /// GameProcess identities are curated-database hits; wrapper layers are
    /// launcher/human titles — a Lutris-argv title is labelled with its own
    /// source (`lutris-wrapper`, S9c) so downstream knows the name came off
    /// the launcher's command line. note_title never downgrades, so the
    /// heroic-config High note (recorded at its creation site) survives the
    /// Medium wrapper-class mapping here. No-op for keys that never missed.
    fn note_group_identity(&mut self, key: &str) {
        let Some((store, code, fallback)) = self.stash_keys.get(key).cloned() else {
            return;
        };
        let Some(id) = self.groups.get(key).and_then(|g| g.identity.clone()) else {
            return;
        };
        let (source_label, confidence) = match id.class {
            IdentityClass::GameProcess
                if self.identity_sources.get(key) == Some(&IdentitySource::SteamManifest) =>
            {
                ("steam-manifest", Confidence::High)
            }
            IdentityClass::GameProcess => ("detectable", Confidence::High),
            IdentityClass::Wrapper
                if self.identity_sources.get(key) == Some(&IdentitySource::LutrisArgv) =>
            {
                ("lutris-wrapper", Confidence::Medium)
            }
            IdentityClass::Wrapper => ("wrapper-layer", Confidence::Medium),
        };
        self.umu_report.note_title(
            &store,
            code.as_deref(),
            &fallback,
            &id.name,
            source_label,
            confidence,
            Some(&id.exe),
        );
    }

    /// Re-emit the rep's current record (UpdateInPlace on the bus via the
    /// correlator; the publish dedup swallows it when nothing changed).
    fn emit_rep_refresh(&self, key: &str, rep: u32) -> Vec<SourceEvent> {
        let Some(group) = self.groups.get(key) else {
            return Vec::new();
        };
        if self.steam_only_groups.contains(key) {
            self.build_group_steam_partial(rep, group)
                .map(|sa| vec![SourceEvent::Updated(Box::new(sa))])
                .unwrap_or_default()
        } else {
            vec![SourceEvent::Updated(Box::new(
                self.build_grouped_activity(rep, group),
            ))]
        }
    }

    /// Build the published activity for a grouped pid from its `/proc`
    /// state, the group identity, and the group's original `since`.
    fn build_grouped_activity(&self, pid: u32, group: &GameGroup) -> Activity {
        let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .and_then(|p| p.to_str().map(str::to_string))
            .or_else(|| group.identity.as_ref().map(|i| i.exe.clone()))
            .unwrap_or_default();
        let mut activity = Activity::from_gamemode(pid as i32, &exe, group.since);
        if let Some(id) = &group.identity {
            activity.name = id.name.clone();
            activity.executable = id.exe.clone();
        } else {
            self.apply_naming(&mut activity);
        }
        activity
    }

    /// Build the group's Steam partial for a pid, named as well as evidence
    /// allows. `None` when the group carries no Steam appid.
    fn build_group_steam_partial(&self, pid: u32, group: &GameGroup) -> Option<Activity> {
        let appid = group.steam_appid.as_deref()?;
        let mut sa = Activity::from_steam(pid as i32, appid);
        self.apply_naming(&mut sa);
        if let Some(id) = &group.identity {
            if !id.name.is_empty() {
                sa.name = id.name.clone();
            }
        }
        Some(sa)
    }

    /// Reconcile the groups against `/proc`:
    /// liveness refresh, one scan pass, then the sweep.
    fn reconcile_groups(&mut self) -> Vec<SourceEvent> {
        self.refresh_liveness();
        let mut events = self.scan_proc();
        events.extend(self.sweep_groups());
        events
    }

    /// Liveness refresh: a member is alive iff `/proc/<pid>`
    /// still resolves to the group's key and is still the same process
    /// (start-time pid-reuse guard). Dead non-rep members are pruned; a dead
    /// rep is kept for the sweep to migrate away from.
    fn refresh_liveness(&mut self) {
        let mut pruned: Vec<u32> = Vec::new();
        for group in self.groups.values_mut() {
            let rep = group.rep;
            let key = group.key.clone();
            group.members.retain(|&pid, member| {
                member.alive = member_alive(pid, &key, member.start_time);
                if member.alive || pid == rep {
                    true
                } else {
                    pruned.push(pid);
                    false
                }
            });
        }
        for pid in pruned {
            self.pid_to_group.remove(&pid);
            self.unresolved_wrappers.remove(&pid);
        }
    }

    /// The scan: one `/proc` pass. Members refresh their scan evidence;
    /// pids matching an existing group are adopted with no gate (failover
    /// memory - though a strictly better class still dethrones);
    /// a NEW group needs `identify_process` or a `/steamapps/` exe with a
    /// resolvable appid, so `Brotato.x86_64` is recoverable while a stray
    /// `SteamAppId` on `/usr/bin/sleep` stays unpublishable.
    fn scan_proc(&mut self) -> Vec<SourceEvent> {
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
            // Probe every key type, not only SteamAppId: a Lutris game's
            // actual game process carries LUTRIS_GAME_UUID (and typically
            // `SteamAppId=default`, which is rejected by design). Gating the
            // whole scan on a Steam appid made lutris/umu-keyed processes
            // invisible - observed live: Far Cry Primal running
            // under Ubisoft Connect stayed unadopted, so the record kept the
            // launcher's name. The Steam appid is still required further
            // down, but only where it belongs: creating a NEW group.
            let Some(key) = merge_key_from_environ(&environ) else {
                continue;
            };
            seen.insert(pid);

            // Known member: nothing to do - the liveness pass owns its
            // refresh, and scan evidence is only ever ADOPTED:
            // a member that unregisters while its process lives is dropped
            // and re-adopted here next tick, as failover memory. Stamping
            // scan evidence onto registered members would instead hold every
            // record open for as long as the process outlives its
            // registration (the `/usr/bin/sleep` fixture would never die).
            if let Some(key) = self.pid_to_group.get(&pid) {
                // Key-fragmentation telemetry: one member
                // carrying two key types would split a game across groups.
                if has_lutris_uuid(&environ) && key.starts_with("steam:") {
                    tracing::warn!(
                        pid,
                        merge_key = %key,
                        "scan: member carries both SteamAppId and LUTRIS_GAME_UUID (key fragmentation)"
                    );
                }
                continue;
            }

            let raw_exe = std::fs::read_link(format!("/proc/{pid}/exe"))
                .ok()
                .and_then(|p| p.to_str().map(str::to_string))
                .unwrap_or_default();

            if self.groups.contains_key(&key) {
                // Adoption, no gate: the group already earned its record.
                let (class, identity) = self.classify_member(pid, &raw_exe, &key, None);
                let member = Member {
                    gamemode: false,
                    scan: true,
                    class,
                    depth: tree_depth(pid),
                    start_time: cache::process_start_time(pid),
                    alive: true,
                };
                let activity = self.build_grouped_activity(pid, &self.groups[&key]);
                tracing::debug!(pid, merge_key = %key, "scan: adopting member");
                events.extend(self.grouped_update(&key, pid, member, identity, activity, None));
                continue;
            }

            // New group: widened gate - still Steam-only. A
            // lutris/umu key with no Steam appid never creates a group from
            // the scan alone; those groups are born from GameMode evidence.
            let Some(appid) = find_steam_appid(&environ) else {
                continue;
            };
            if !self.new_group_gate(pid, &raw_exe, &appid) {
                continue;
            }
            let (class, identity) = self.classify_member(pid, &raw_exe, &key, None);
            let mut group = GameGroup::new(key.clone(), 0);
            group.steam_appid = Some(appid.clone());
            if let Some((id, source)) = identity {
                // A fresh group adopts its first identity unconditionally.
                group.set_identity(id);
                self.identity_sources.insert(key.clone(), source);
            }
            group.upsert(
                pid,
                Member {
                    gamemode: false,
                    scan: true,
                    class,
                    depth: tree_depth(pid),
                    start_time: cache::process_start_time(pid),
                    alive: true,
                },
                false,
            );
            let sa = self
                .build_group_steam_partial(pid, &group)
                .expect("scan group always has a steam appid");
            self.groups.insert(key.clone(), group);
            self.steam_only_groups.insert(key.clone());
            self.pid_to_group.insert(pid, key.clone());
            self.steam_appids.insert(pid, appid);
            self.steam_ids.insert(pid, sa.id.clone());
            tracing::debug!(pid, merge_key = %key, "scan: new Steam-only group");
            // The scan is the only discovery path for a Steam game launched
            // without GameMode; the launch hook never ran for it, so the
            // stash record (a steam gap only - see maybe_stash_launch) and
            // the elected-identity note flow from here.
            self.maybe_stash_launch(&key, &environ, &raw_exe);
            self.note_group_identity(&key);
            events.push(SourceEvent::Updated(Box::new(sa)));
        }

        // Reconcile: tracked Steam pids that vanished (process died or pid
        // reused by a non-Steam process). Only remove pids with no other
        // active source - GameMode-tracked pids are managed by the
        // source-removal path, grouped pids by the group lifecycle.
        let tracked: Vec<u32> = self.steam_appids.keys().copied().collect();
        for pid in tracked {
            if self.pid_to_group.contains_key(&pid) {
                continue;
            }
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

    /// The gate for creating a NEW group from the scan:
    /// `identify_process` hit, OR exe under `/steamapps/` with an appid
    /// detectable.json resolves. Keeps the `/usr/bin/sleep` fixture and
    /// wrapper-chain utility processes unpublishable.
    fn new_group_gate(&self, pid: u32, exe: &str, appid: &str) -> bool {
        if let Some(ref db) = self.naming {
            if identify_process(pid, db).is_some() {
                return true;
            }
            if exe.contains("/steamapps/") && db.lookup_by_steam_appid(appid).is_some() {
                return true;
            }
        }
        // Steam's own appmanifest is the third authority, and the only one
        // that needs no database: a game absent from detectable.json is
        // still a real install with an official name (R5 holds — nothing
        // outside a steamapps library, and no appid without a manifest,
        // gets a record).
        exe.contains("/steamapps/") && steam_manifest_name(exe, appid).is_some()
    }

    /// The sweep: evidence-less groups are removed;
    /// dead/unregistered reps migrate to an eligible survivor (deferred
    /// migration case b); unidentified reps get an identification retry.
    fn sweep_groups(&mut self) -> Vec<SourceEvent> {
        let mut events = Vec::new();
        let keys: Vec<String> = self.groups.keys().cloned().collect();
        for key in keys {
            let Some(group) = self.groups.get(&key) else {
                continue;
            };
            if !group.has_evidence() {
                tracing::info!(
                    merge_key = %key,
                    rep = group.rep,
                    "group sweep: last evidence gone, removing record"
                );
                events.extend(self.remove_group_records(&key));
                continue;
            }
            let rep = group.rep;
            let rep_live_registered = group
                .members
                .get(&rep)
                .is_some_and(|m| m.alive && m.gamemode);
            if !rep_live_registered {
                if let Some(new_rep) = group.elect() {
                    if new_rep != rep {
                        tracing::info!(
                            old = rep,
                            new = new_rep,
                            merge_key = %key,
                            "group sweep: migrating representative (publish-first)"
                        );
                        self.groups
                            .get_mut(&key)
                            .expect("group present in sweep")
                            .rep = new_rep;
                        let group = &self.groups[&key];
                        let gamemode_activity = if self.steam_only_groups.contains(&key) {
                            None
                        } else {
                            Some(self.build_grouped_activity(new_rep, group))
                        };
                        let sa = self.build_group_steam_partial(new_rep, group);
                        events.extend(self.emit_migration(
                            &key,
                            rep,
                            new_rep,
                            gamemode_activity,
                            sa,
                        ));
                    }
                }
            }
            // Unidentified rep: identification retry.
            if self.groups.get(&key).is_some_and(|g| g.identity.is_none()) {
                events.extend(self.retry_group_identity(&key));
            }
        }
        events
    }

    /// Try to identify an unidentified group through its rep's own process
    /// state (exe link or Wine cmdline). On success the identity is set and
    /// the record refreshed in place.
    fn retry_group_identity(&mut self, key: &str) -> Vec<SourceEvent> {
        let Some(ref db) = self.naming else {
            return Vec::new();
        };
        let Some(group) = self.groups.get(key) else {
            return Vec::new();
        };
        let rep = group.rep;
        let Some((name, exe)) = identify_process(rep, db) else {
            return Vec::new();
        };
        tracing::info!(rep, game_name = %name, merge_key = %key, "group sweep: identified representative");
        self.groups
            .get_mut(key)
            .expect("group checked above")
            .set_identity(Identity {
                name,
                exe,
                class: IdentityClass::GameProcess,
            });
        // Only entered while the group is unidentified, so the GameProcess
        // identity is always adopted.
        self.identity_sources
            .insert(key.to_string(), IdentitySource::Curated);
        self.note_group_identity(key);
        self.emit_rep_refresh(key, rep)
    }

    /// Run the three identification layers for a wrapper pid.
    /// Returns `(game_name, game_executable)` on success.
    fn identify_wrapper(&self, pid: u32) -> Option<(String, String, IdentitySource)> {
        if let Some(db) = self.naming.as_ref() {
            // Layer 1: the wrapper's own cmdline usually names the game.
            if let Some((name, exe)) = identify_via_cmdline(pid, db) {
                return Some((name, exe, IdentitySource::Walk));
            }
            // Layer 2: connected descendant walk.
            if let Some((name, exe)) = find_game_descendant(pid, db) {
                return Some((name, exe, IdentitySource::Walk));
            }
            // Layer 3: Flatpak-portal sandbox family (umu tmpdir bridge).
            if let Some((name, exe)) = find_game_in_sandbox_family(pid, db) {
                return Some((name, exe, IdentitySource::Walk));
            }
        }
        // Layer 4: a `lutris-wrapper` ancestor announces the human
        // title in its own argv - Lutris telling us what it launched. The
        // authoritative fallback for games detectable.json does not know
        // (UbisoftConnect.exe was the live case), and the only layer that
        // works without a naming database.
        identify_via_lutris_ancestor(pid).map(|(name, exe)| (name, exe, IdentitySource::LutrisArgv))
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
        // Clear wrapper executable names - "env", "bash", etc. are never
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

        // Discord activities already have a human-curated name - don't touch.
        if activity.sources.contains(&Source::Discord) && !activity.name.is_empty() {
            return;
        }

        // Try appid lookup first (stronger signal), then executable. No early
        // return without a database: the hint fallback below must still run -
        // a machine with no detectable.json is exactly where hints matter.
        if let Some(ref db) = self.naming {
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

        // The weakest rung - an MPRIS hint for this exact pid fills a
        // name that everything above left empty.
        self.apply_name_hint(activity);
    }

    /// Fill a *default* name from an MPRIS hint for this pid. A default
    /// name is empty or the executable stem - what `from_gamemode` sets when
    /// nothing curated exists. A curated name (Discord, group identity,
    /// detectable.json) is never touched: the hint is one rung above the
    /// stem and below everything else.
    fn apply_name_hint(&self, activity: &mut Activity) {
        if !name_is_default(activity) {
            return;
        }
        let pid = activity.process_id;
        if pid == 0 {
            return;
        }
        if let Some(hint) = self.name_hints.get(&pid) {
            activity.name = hint.clone();
        }
    }

    fn on_removed(&mut self, id: String, source: Source) -> Vec<SourceEvent> {
        // Extract pid before moving `id` into the event.
        let pid = if source != Source::Steam {
            extract_pid(&id)
        } else {
            None
        };

        // Grouped GameMode removals are the group's business -
        // deferred migration holds the record through the exit cascade.
        if source == Source::GameMode {
            if let Some(pid) = pid {
                if let Some(key) = self.pid_to_group.get(&pid).cloned() {
                    return self.grouped_removal(&key, pid);
                }
                // A withheld record was never on the bus - its death is
                // fully silent. Neither the GameMode Removed nor the
                // Steam-partial removal below may fire for it. This is what
                // makes the µs-lived keyless helpers produce zero traffic.
                if self.withheld.remove(&pid).is_some() {
                    self.unresolved_wrappers.remove(&pid);
                    if let Some(set) = self.active_sources.get_mut(&pid) {
                        set.remove(&Source::GameMode);
                        if set.is_empty() {
                            self.active_sources.remove(&pid);
                        }
                    }
                    return Vec::new();
                }
            }
        }

        // If this is a non-Steam source removal, check if the pid has any
        // remaining non-Steam sources. If not, remove the Steam partial -
        // BEFORE the triggering removal (per the exactly-one-ActivityRemoved
        // promise: Steam-first degrades
        // the record in place instead of flashing a transient steam_<pid>
        // record between the two removals).
        let mut events = Vec::with_capacity(2);
        if let Some(pid) = pid {
            self.unresolved_wrappers.remove(&pid);
            if let Some(sources) = self.active_sources.get_mut(&pid) {
                sources.remove(&source);
                if sources.is_empty() {
                    self.active_sources.remove(&pid);
                    // A grouped pid's Steam partial is owned by the group
                    // lifecycle (remove_group_records / emit_migration) -
                    // same guard as on_source_lost and the scan reconcile.
                    // A detaching Discord partial must not kill a
                    // scan-backed group's record.
                    if !self.pid_to_group.contains_key(&pid) {
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
        events.push(SourceEvent::Removed { id, source });

        events
    }

    /// GameMode unregistered a grouped pid.
    ///
    /// Non-rep members were never on the bus: bookkeeping only, zero events.
    /// The rep's departure is swallowed while any member still holds evidence
    /// (deferred migration - the tick sweep elects a successor); only when
    /// the last evidence is gone do the group's records leave the bus.
    fn grouped_removal(&mut self, key: &str, pid: u32) -> Vec<SourceEvent> {
        self.unresolved_wrappers.remove(&pid);
        self.active_sources.remove(&pid);
        let Some(group) = self.groups.get_mut(key) else {
            self.pid_to_group.remove(&pid);
            return Vec::new();
        };
        match group.member_gone(pid) {
            GroupEffect::RemoveAll => {
                tracing::info!(
                    merge_key = %key,
                    rep = group.rep,
                    "group: last evidence gone, removing record"
                );
                self.remove_group_records(key)
            }
            _ => {
                if !group.members.contains_key(&pid) {
                    self.pid_to_group.remove(&pid);
                }
                if pid == group.rep {
                    tracing::debug!(
                        pid,
                        merge_key = %key,
                        "group: rep unregistered, holding record (deferred migration)"
                    );
                } else if !group.has_evidence() {
                    // A non-rep departure stripped the LAST evidence (the rep
                    // unregistered earlier and was held): the game is over -
                    // remove now instead of waiting for the sweep (the
                    // exit cascade ends in exactly one prompt ActivityRemoved).
                    tracing::info!(
                        merge_key = %key,
                        rep = group.rep,
                        "group: last evidence gone, removing record"
                    );
                    return self.remove_group_records(key);
                }
                Vec::new()
            }
        }
    }

    /// Drop a group and emit the removal events for its published records:
    /// `Removed(steam id, Steam)` then `Removed(pid_<rep>, GameMode)`.
    /// Steam-only groups only ever published the Steam partial.
    ///
    /// Deliberately removes Steam first, not GameMode first, honoring the
    /// exactly-one-ActivityRemoved promise: with the GameMode
    /// partial gone first, the surviving Steam partial would re-merge and
    /// flash a transient `steam_<rep>` Added+Removed pair at every exit.
    /// Steam-first degrades the record in place; it dies with its last
    /// source - one removal.
    fn remove_group_records(&mut self, key: &str) -> Vec<SourceEvent> {
        let Some(group) = self.groups.remove(key) else {
            return Vec::new();
        };
        let steam_only = self.steam_only_groups.remove(key);
        self.identity_sources.remove(key);
        let mut events = Vec::with_capacity(2);
        if let Some(sid) = self.steam_ids.remove(&group.rep) {
            events.push(SourceEvent::Removed {
                id: sid,
                source: Source::Steam,
            });
        }
        if !steam_only {
            events.push(SourceEvent::Removed {
                id: format!("pid_{}", group.rep),
                source: Source::GameMode,
            });
        }
        self.steam_appids.remove(&group.rep);
        self.active_sources.remove(&group.rep);
        self.pid_to_group.remove(&group.rep);
        for member_pid in group.members.keys() {
            self.pid_to_group.remove(member_pid);
            self.unresolved_wrappers.remove(member_pid);
            self.active_sources.remove(member_pid);
        }
        events
    }

    fn on_source_lost(&mut self, source: Source) -> Vec<SourceEvent> {
        let mut events = vec![SourceEvent::SourceLost { source }];

        // Group bookkeeping: every gamemode flag is void.
        // Groups with scan evidence survive - the correlator degrades their
        // records to Steam-only on SourceLost; the rest are dropped here
        // (bookkeeping) and their Steam partials reaped by the loop below.
        if source == Source::GameMode {
            // Withheld records were GameMode-only by construction.
            self.withheld.clear();
            for group in self.groups.values_mut() {
                for member in group.members.values_mut() {
                    member.gamemode = false;
                }
            }
            let dead: Vec<String> = self
                .groups
                .iter()
                .filter(|(_, g)| !g.has_evidence())
                .map(|(k, _)| k.clone())
                .collect();
            for key in dead {
                tracing::debug!(merge_key = %key, "group: dropped on GameMode source loss");
                self.groups.remove(&key);
                self.steam_only_groups.remove(&key);
                self.identity_sources.remove(&key);
            }
            for key in self.groups.keys() {
                self.steam_only_groups.insert(key.clone());
            }
            let groups = &self.groups;
            self.pid_to_group.retain(|_, key| groups.contains_key(key));
        }

        // When a non-Steam source is lost, remove its entries from all pids.
        // If a pid's non-Steam source set becomes empty, remove its Steam partial.
        if source != Source::Steam {
            let mut pids_to_check: Vec<u32> = self.active_sources.keys().copied().collect();
            for pid in pids_to_check.drain(..) {
                if let Some(sources) = self.active_sources.get_mut(&pid) {
                    sources.remove(&source);
                    if sources.is_empty() {
                        self.active_sources.remove(&pid);
                        // Surviving scan-backed groups keep their Steam
                        // partial - the group lifecycle owns it.
                        if self.pid_to_group.contains_key(&pid) {
                            continue;
                        }
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

/// Probe `/proc/<pid>/environ` for the best available merge key.
///
/// Priority: `SteamAppId` (numeric) > `LUTRIS_GAME_UUID` > `UMU_ID`
/// (non-default). The merge key is used for ancestor-walk deduplication
/// across wrapper trees - two processes sharing a key and a process tree
/// are the same game.
fn probe_merge_key(pid: u32) -> Option<String> {
    let environ = std::fs::read_to_string(format!("/proc/{pid}/environ")).ok()?;
    merge_key_from_environ(&environ)
}

/// Parse the best available merge key from an environ string (see
/// [`probe_merge_key`]). Split out so the scan's single environ read serves
/// both the appid and the key probe.
/// A usable Steam appid: numeric, non-empty, and not the `0` Steam sets for
/// non-Steam titles. The single definition of validity - `find_steam_appid`
/// and `merge_key_from_environ` both use it, because the last time they had
/// separate checks they diverged: the merge key accepted `SteamAppId=0` and
/// pooled every non-Steam game into one shared `steam:0` group whose members
/// dethroned each other (observed live).
fn valid_steam_appid(v: &str) -> bool {
    !v.is_empty() && v != "0" && v.chars().all(|c| c.is_ascii_digit())
}

/// The umu id of a launch that went through umu WITHOUT a database entry:
/// `GAMEID=umu-0` (Heroic's shape) or `UMU_ID=umu-default`/`umu-0` (Lutris).
/// A real entry would have produced a usable id - and a merge key.
fn umu_miss_id(environ: &str) -> Option<&'static str> {
    for entry in environ.split('\0') {
        match entry {
            "GAMEID=umu-0" | "UMU_ID=umu-0" => return Some("umu-0"),
            "UMU_ID=umu-default" | "GAMEID=umu-default" => return Some("umu-default"),
            _ => {}
        }
    }
    None
}

/// The name Steam's own appmanifest records for an appid: the exe runs out
/// of `<library>/steamapps/common/<game>/`, and the manifest sits at
/// `<library>/steamapps/appmanifest_<appid>.acf`. A shallow line parse is
/// enough — the `"name"` key is one quoted pair — and any miss (no
/// steamapps segment, no file, no name line) is a silent None.
fn steam_manifest_name(exe: &str, appid: &str) -> Option<String> {
    let end = exe.find("/steamapps/")? + "/steamapps/".len();
    let path = format!("{}appmanifest_{}.acf", &exe[..end], appid);
    let raw = std::fs::read_to_string(path).ok()?;
    for line in raw.lines() {
        if let Some(rest) = line.trim().strip_prefix("\"name\"") {
            let name = rest.trim().trim_matches('"').trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// One environment value out of a raw `/proc/<pid>/environ` blob.
fn env_value(environ: &str, var: &str) -> Option<String> {
    let prefix = format!("{var}=");
    environ
        .split('\0')
        .find_map(|e| e.strip_prefix(prefix.as_str()))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// The store codename Lutris wrote beside the game (S9c): read
/// `<dir>/.lutrisgame.json` and take its `appid`. Only itch.io installs
/// carry the file on a measured machine; a missing or malformed file is a
/// silent `None` — same precedent as [`heroic_title`].
fn lutris_marker_appid(dir: &str) -> Option<String> {
    let raw = std::fs::read_to_string(std::path::Path::new(dir).join(".lutrisgame.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let appid = match parsed.get("appid")? {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!appid.is_empty()).then_some(appid)
}

/// A stable slug of a launcher's game name (S9c stash fallback key):
/// lowercase, runs of `[a-z0-9]` joined by single hyphens, everything else
/// dropped. "Danger Scavenger" becomes "danger-scavenger".
fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut gap = false;
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

/// Which runtime a launch ran under (S9c): `proton` when umu's marker was
/// present or the raw exe is a Wine/Proton binary (the preloaders, plain
/// wine, or a Windows `.exe` under a prefix), `native` otherwise.
fn runner_of(umu_marker: bool, raw_exe: &str) -> &'static str {
    if umu_marker || is_wine_binary(raw_exe) {
        "proton"
    } else {
        "native"
    }
}

/// Is this exe a Wine/Proton process — the runtime, or a Windows binary?
fn is_wine_binary(raw_exe: &str) -> bool {
    let lower = raw_exe.to_ascii_lowercase();
    let base = std::path::Path::new(&lower)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");
    matches!(
        base,
        "wine" | "wine64" | "wine-preloader" | "wine64-preloader"
    ) || lower.ends_with(".exe")
}

fn merge_key_from_environ(environ: &str) -> Option<String> {
    let mut steam_appid = None;
    let mut lutris_uuid = None;
    let mut umu_id = None;
    let mut heroic = None;
    for entry in environ.split('\0') {
        if let Some(v) = entry.strip_prefix("SteamAppId=") {
            if valid_steam_appid(v) {
                steam_appid = Some(format!("steam:{v}"));
            }
        } else if let Some(v) = entry.strip_prefix("SteamGameId=") {
            if steam_appid.is_none() && valid_steam_appid(v) {
                steam_appid = Some(format!("steam:{v}"));
            }
        } else if let Some(v) = entry.strip_prefix("LUTRIS_GAME_UUID=") {
            if !v.is_empty() {
                lutris_uuid = Some(format!("lutris:{v}"));
            }
        } else if let Some(v) = entry.strip_prefix("UMU_ID=umu-") {
            if v != "default" && v != "0" && !v.is_empty() {
                umu_id = Some(format!("umu:{v}"));
            }
        } else if let Some(v) = entry.strip_prefix("HEROIC_APP_NAME=") {
            // Heroic (Epic/GOG/Amazon) launches carry no usable Steam or
            // Lutris identity - SteamAppId=0 and GAMEID=umu-0, both rejected
            // above - but the store codename is present in every process of
            // the tree (observed live: Control ran as HEROIC_APP_NAME=Calluna
            // with nothing else to key on).
            if !v.is_empty() {
                heroic = Some(format!("heroic:{v}"));
            }
        }
    }
    steam_appid.or(lutris_uuid).or(umu_id).or(heroic)
}

/// Does the environ carry a non-empty `LUTRIS_GAME_UUID`? Used for the
/// key-fragmentation telemetry.
fn has_lutris_uuid(environ: &str) -> bool {
    environ.split('\0').any(|e| {
        e.strip_prefix("LUTRIS_GAME_UUID=")
            .is_some_and(|v| !v.is_empty())
    })
}

/// Group-member liveness with the pid-reuse guard: the
/// pid must still resolve to the group's merge key AND still be the same
/// process (start time captured at insert). A recycled pid fails the
/// start-time check even when the new occupant carries the same key.
fn member_alive(pid: u32, key: &str, recorded_start: Option<u64>) -> bool {
    if probe_merge_key(pid).as_deref() != Some(key) {
        return false;
    }
    match (recorded_start, cache::process_start_time(pid)) {
        (Some(recorded), Some(current)) => recorded == current,
        // No start time captured at insert: existence + key is the best test.
        (None, Some(_)) => true,
        _ => false,
    }
}

/// Probe `/proc/<pid>/environ` for Steam appid variables.
///
/// Checks in order:
/// 1. `UMU_ID=umu-<N>` - umu-launcher convention; numeric N implies steam appid N
/// 2. `SteamAppId` - native Steam
/// 3. `SteamGameId` - native Steam (alternative variable)
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
/// `SteamGameId`. Only non-zero numeric values are accepted - Steam sets
/// `SteamAppId=default` for its own client processes and `SteamAppId=0`
/// for non-Steam games, neither of which is a game appid.
fn find_steam_appid(environ: &str) -> Option<String> {
    for entry in environ.split('\0') {
        if let Some(value) = entry.strip_prefix("UMU_ID=umu-") {
            // umu-launcher: numeric N implies steam appid N
            if valid_steam_appid(value) {
                return Some(value.to_string());
            }
        }
        if let Some(value) = entry.strip_prefix("SteamAppId=") {
            if valid_steam_appid(value) {
                return Some(value.to_string());
            }
        }
        if let Some(value) = entry.strip_prefix("SteamGameId=") {
            if valid_steam_appid(value) {
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
    if let Some(found) = identify_process_exe(pid, db) {
        return Some(found);
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

/// The exe-link layer of [`identify_process`] alone. Used for known wrapper
/// executables, whose cmdlines name the game they merely launch - only the
/// process's own binary may prove it IS the game.
fn identify_process_exe(pid: u32, db: &NamingDb) -> Option<(String, String)> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let exe_str = exe.to_str()?;
    let name = db.lookup_by_executable(exe_str)?;
    Some((name.to_string(), exe_str.to_string()))
}

/// Layer 4: find a `lutris-wrapper` ancestor and take the title from
/// its argv. Evidence-based: same bounded ppid chain as every other walk -
/// never a guess across trees.
/// Resolve a Heroic store codename to its display title from the
/// launcher's own install records - `installed.json` written by legendary
/// (Epic). Local files the user's launcher maintains; no network, no
/// guessing. Both the Flatpak and native config locations are tried.
fn heroic_title(app_name: &str) -> Option<String> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let candidates = [
        home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic/legendaryConfig/legendary/installed.json"),
        home.join(".config/heroic/legendaryConfig/legendary/installed.json"),
        home.join(".config/legendary/installed.json"),
    ];
    for path in candidates {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Some(title) = heroic_title_from_json(&raw, app_name) {
                return Some(title);
            }
        }
    }
    None
}

/// Pure half of [`heroic_title`], for tests: legendary's `installed.json` is
/// a map of app-name → record with a `title` field.
fn heroic_title_from_json(raw: &str, app_name: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    let title = parsed.get(app_name)?.get("title")?.as_str()?.trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn identify_via_lutris_ancestor(pid: u32) -> Option<(String, String)> {
    let mut current = pid;
    for _ in 0..MAX_ANCESTOR_DEPTH {
        let cmdline = std::fs::read_to_string(format!("/proc/{current}/cmdline")).ok();
        if let Some(cmdline) = cmdline {
            let tokens: Vec<String> = cmdline
                .split('\0')
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect();
            if let Some(name) = parse_lutris_wrapper_argv(&tokens) {
                let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                return Some((name, exe));
            }
        }
        current = read_ppid(current)?;
        if current <= 1 {
            return None;
        }
    }
    None
}

/// Extract the game title from a lutris-wrapper command line.
///
/// Shape: `… lutris-wrapper <title…> <children> <watched> <command…>` - the
/// title is every token between the wrapper and the two counters. Titles may
/// themselves end in digits ("Left 4 Dead 2"), so the counters are found from
/// the right: the last adjacent integer pair followed by a non-integer
/// command token.
fn parse_lutris_wrapper_argv(tokens: &[String]) -> Option<String> {
    let wrapper_idx = tokens.iter().position(|t| {
        std::path::Path::new(t)
            .file_name()
            .is_some_and(|f| f == "lutris-wrapper")
    })?;
    let rest = &tokens[wrapper_idx + 1..];
    if rest.len() < 3 {
        return None;
    }

    let is_int = |t: &str| t.parse::<u32>().is_ok();
    let split = (0..rest.len().saturating_sub(2))
        .rev()
        .find(|&i| is_int(&rest[i]) && is_int(&rest[i + 1]) && !is_int(&rest[i + 2]))?;
    if split == 0 {
        // No title tokens before the counters.
        return None;
    }
    let title = rest[..split].join(" ").trim().to_string();
    (!title.is_empty()).then_some(title)
}

/// Layer 1: identify a wrapper's game from its own cmdline.
///
/// Launch wrappers carry the game path at the end of their command line:
/// `... proton waitforexitandrun /path/Game.exe`. Cheap, no tree walk.
fn identify_via_cmdline(pid: u32, db: &NamingDb) -> Option<(String, String)> {
    let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).ok()?;
    let mut result = None;
    for token in cmdline.split('\0').filter(|t| !t.is_empty()) {
        if let Some(name) = db.lookup_by_executable(token) {
            // Keep scanning: the LAST match wins - the game exe is at the
            // end of the wrapper's cmdline (after proton/umu-shim paths).
            result = Some((name.to_string(), token.to_string()));
        }
    }
    result
}

/// Layer 3: find the game inside a Flatpak-portal-spawned sandbox.
///
/// When Lutris runs as a Flatpak, steam-runtime-launch-client asks
/// `org.freedesktop.portal.Flatpak` to spawn the bwrap sandbox - the
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
        // sandbox - the game is a child of pv-adverb).
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

    // Not a game - check children.
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

/// Whether an activity's name is a default (empty, or the executable stem) -
/// i.e. nothing curated has named it yet. The stem comparison mirrors the
/// wrapper-clearing logic in `apply_naming`.
fn name_is_default(activity: &Activity) -> bool {
    if activity.name.is_empty() {
        return true;
    }
    if activity.executable.is_empty() {
        return false;
    }
    std::path::Path::new(&activity.executable)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .as_deref()
        == Some(activity.name.to_lowercase().as_str())
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

    /// A nameless ungrouped record: GameMode source, wrapper exe whose stem
    /// apply_naming clears, no merge key. The withhold shape.
    fn nameless_activity(pid: u32) -> Activity {
        Activity::from_gamemode(pid as i32, "/usr/bin/bash", 1_700_000_500)
    }

    #[test]
    fn nameless_ungrouped_gamemode_is_withheld() {
        let mut e = Enricher::with_naming(None);
        let out = e.process(SourceEvent::Updated(Box::new(nameless_activity(4242))));
        assert!(out.is_empty(), "nameless record must not publish: {out:?}");
        let held = e.withheld.get(&4242).expect("record must be withheld");
        assert_eq!(held.since, 1_700_000_500, "authoritative since preserved");
    }

    #[test]
    fn withheld_publishes_on_name_hint() {
        let mut e = Enricher::with_naming(None);
        e.process(SourceEvent::Updated(Box::new(nameless_activity(4242))));
        let out = e.process(SourceEvent::NameHint {
            pid: 4242,
            name: "Cool Game".to_string(),
        });
        assert_eq!(out.len(), 1);
        let SourceEvent::Updated(a) = &out[0] else {
            panic!("expected Updated, got {out:?}");
        };
        assert_eq!(a.name, "Cool Game");
        assert_eq!(a.since, 1_700_000_500, "original since survives");
        assert!(e.withheld.is_empty());
    }

    #[test]
    fn withheld_publishes_on_reseed_once_named() {
        let mut e = Enricher::with_naming(None);
        e.process(SourceEvent::Updated(Box::new(nameless_activity(4242))));
        // A second nameless reseed stays silent.
        let out = e.process(SourceEvent::Updated(Box::new(nameless_activity(4242))));
        assert!(out.is_empty(), "still nameless, still withheld");
        // A hint arrives out of band; the next reseed publishes named.
        e.name_hints.insert(4242, "Named Now".to_string());
        let out = e.process(SourceEvent::Updated(Box::new(nameless_activity(4242))));
        assert_eq!(out.len(), 1, "named reseed must publish: {out:?}");
        let SourceEvent::Updated(a) = &out[0] else {
            panic!("expected Updated");
        };
        assert_eq!(a.name, "Named Now");
        assert!(e.withheld.is_empty());
    }

    #[test]
    fn withheld_removal_is_silent() {
        let mut e = Enricher::with_naming(None);
        e.process(SourceEvent::Updated(Box::new(nameless_activity(4242))));
        let out = e.process(SourceEvent::Removed {
            id: "pid_4242".to_string(),
            source: Source::GameMode,
        });
        assert!(out.is_empty(), "never published, never removed: {out:?}");
        assert!(e.withheld.is_empty());
        assert!(!e.active_sources.contains_key(&4242));
    }

    #[test]
    fn withheld_flushes_when_discord_joins() {
        let mut e = Enricher::with_naming(None);
        e.process(SourceEvent::Updated(Box::new(nameless_activity(4242))));

        let mut discord = Activity::new("discord_4242");
        discord.sources = vec![Source::Discord];
        discord.name = "Rich Presence Game".to_string();
        discord.process_id = 4242;
        let out = e.process(SourceEvent::Updated(Box::new(discord)));

        // GameMode record first (PublishNew pid_4242), then the Discord
        // update that joins it - never an absorb pair, never a drop.
        assert_eq!(out.len(), 2, "{out:?}");
        let SourceEvent::Updated(first) = &out[0] else {
            panic!("expected Updated");
        };
        assert_eq!(first.id, "pid_4242");
        let SourceEvent::Updated(second) = &out[1] else {
            panic!("expected Updated");
        };
        assert_eq!(second.id, "discord_4242");
        assert!(e.withheld.is_empty());
    }

    #[test]
    fn heroic_app_name_is_a_merge_key_of_last_resort() {
        // The live Control shape: zero Steam id, zero umu id, no Lutris -
        // only the Heroic codename identifies the tree.
        assert_eq!(
            merge_key_from_environ("SteamAppId=0\0GAMEID=umu-0\0HEROIC_APP_NAME=Calluna\0")
                .as_deref(),
            Some("heroic:Calluna")
        );
        // Any stronger key wins.
        assert_eq!(
            merge_key_from_environ("SteamAppId=480\0HEROIC_APP_NAME=Calluna\0").as_deref(),
            Some("steam:480")
        );
        assert_eq!(
            merge_key_from_environ("HEROIC_APP_NAME=\0"),
            None,
            "empty codename is not a key"
        );
    }

    #[test]
    fn heroic_title_resolves_from_legendary_installed_json() {
        let raw = r#"{
            "Calluna": {"app_name": "Calluna", "title": "Control",
                        "install_path": "/media/Data/Spiele/Control"},
            "Fortnite": {"app_name": "Fortnite", "title": ""}
        }"#;
        assert_eq!(
            heroic_title_from_json(raw, "Calluna").as_deref(),
            Some("Control")
        );
        // Empty titles and unknown apps resolve to nothing - the group then
        // stays unidentified rather than being named after a codename.
        assert_eq!(heroic_title_from_json(raw, "Fortnite"), None);
        assert_eq!(heroic_title_from_json(raw, "Unknown"), None);
        assert_eq!(heroic_title_from_json("not json", "Calluna"), None);
    }

    #[test]
    fn merge_key_rejects_zero_and_default() {
        // SteamAppId=0 is what Steam sets for non-Steam titles; accepting it
        // pooled unrelated games into one shared `steam:0` group.
        assert_eq!(merge_key_from_environ("SteamAppId=0\0"), None);
        assert_eq!(merge_key_from_environ("SteamGameId=0\0"), None);
        assert_eq!(merge_key_from_environ("SteamAppId=default\0"), None);
        assert_eq!(merge_key_from_environ("UMU_ID=umu-0\0"), None);
        // The fallthrough is the point of the fix: a zero Steam id must not
        // shadow the real Lutris key.
        assert_eq!(
            merge_key_from_environ("SteamAppId=0\0LUTRIS_GAME_UUID=abc\0").as_deref(),
            Some("lutris:abc")
        );
        assert_eq!(
            merge_key_from_environ("SteamAppId=480\0").as_deref(),
            Some("steam:480")
        );
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
        // its own client processes - not a game appid).
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

        // GameMode removes its record - Discord still active
        let events = e.process(SourceEvent::Removed {
            id: "pid_42".to_string(),
            source: Source::GameMode,
        });
        assert_eq!(events.len(), 1); // Only the original removal
        assert!(e.steam_ids.contains_key(&42));

        // Discord removes its record - last non-Steam source gone. The Steam
        // removal precedes the triggering removal so the record degrades in
        // place and dies with its last source (one ActivityRemoved).
        let events = e.process(SourceEvent::Removed {
            id: "discord_42".to_string(),
            source: Source::Discord,
        });
        assert_eq!(events.len(), 2); // Steam removal + original removal
        if let SourceEvent::Removed { id, source } = &events[0] {
            assert_eq!(id, "steam_42");
            assert_eq!(*source, Source::Steam);
        } else {
            panic!("expected Steam Removed first, got {events:?}");
        }
        assert_eq!(
            events[1],
            SourceEvent::Removed {
                id: "discord_42".to_string(),
                source: Source::Discord,
            }
        );
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

        // A new pid arrives - no Steam probe (no appid in /proc), no merge
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
    fn scan_adopts_lutris_keyed_processes_into_existing_groups() {
        // The Far Cry Primal shape: a group exists under a lutris key (born
        // from GameMode evidence), and the actual game process carries only
        // LUTRIS_GAME_UUID - SteamAppId=default. The scan must adopt it.
        let uuid = format!("test-{}", std::process::id());
        let key = format!("lutris:{uuid}");

        let mut game = std::process::Command::new("sleep")
            .arg("30")
            .env("LUTRIS_GAME_UUID", &uuid)
            .env("SteamAppId", "default")
            .spawn()
            .unwrap();
        let game_pid = game.id();
        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut e = Enricher::with_naming(None);
        // Group born from (simulated) GameMode evidence, rep = a fake pid
        // that is not alive; the scan pass must still adopt the live one.
        e.grouped_update(
            &key,
            game_pid,
            group_member(MemberClass::Helper),
            None,
            gamemode_activity(game_pid),
            None,
        );
        // Forget the pid mapping to force the adoption path, as if the
        // process had never registered with GameMode.
        e.pid_to_group.remove(&game_pid);
        e.groups.get_mut(&key).unwrap().members.clear();
        e.groups.get_mut(&key).unwrap().members.insert(
            game_pid + 1_000_000, // dead placeholder rep
            Member {
                gamemode: true,
                scan: false,
                class: MemberClass::Helper,
                depth: 0,
                start_time: None,
                alive: true,
            },
        );
        e.groups.get_mut(&key).unwrap().rep = game_pid + 1_000_000;

        let _ = e.reconcile_groups();
        let adopted = e
            .groups
            .get(&key)
            .is_some_and(|g| g.members.contains_key(&game_pid));
        let _ = game.kill();
        let _ = game.wait();
        assert!(
            adopted,
            "scan failed to adopt a lutris-keyed process into its existing group"
        );
    }

    #[test]
    fn lutris_wrapper_argv_parses_titles() {
        let t = |v: &[&str]| {
            parse_lutris_wrapper_argv(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        // The live Ubisoft Connect shape (Flatpak Lutris).
        assert_eq!(
            t(&[
                "python3",
                "/app/share/lutris/bin/lutris-wrapper",
                "Ubisoft",
                "Connect",
                "0",
                "0",
                "gamemoderun",
                "/x/umu-run"
            ])
            .as_deref(),
            Some("Ubisoft Connect")
        );
        // A title that ends in digits must keep them: the counters are the
        // LAST integer pair before the command.
        assert_eq!(
            t(&[
                "python3",
                "lutris-wrapper",
                "Left",
                "4",
                "Dead",
                "2",
                "0",
                "0",
                "sh",
                "-c",
                "run"
            ])
            .as_deref(),
            Some("Left 4 Dead 2")
        );
        assert_eq!(
            t(&["lutris-wrapper", "Brotato", "1", "2", "wine", "brotato.exe"]).as_deref(),
            Some("Brotato")
        );
        // Not a lutris-wrapper cmdline at all.
        assert_eq!(t(&["python3", "umu-run", "Game.exe"]), None);
        // No title before the counters.
        assert_eq!(t(&["lutris-wrapper", "0", "0", "cmd"]), None);
        // Too short to carry counters + command.
        assert_eq!(t(&["lutris-wrapper", "Solo"]), None);
    }

    #[test]
    fn lutris_ancestor_names_a_wrapped_process() {
        // Faithful tree: like the real lutris-wrapper, the script KEEPS its
        // argv (title + counters) and runs the game as a child. The walker
        // starts at the child and must find the title one hop up.
        let dir = std::env::temp_dir().join(format!("gamebus-lutris-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("lutris-wrapper");
        std::fs::write(&script, "#!/bin/sh\nsleep 30 &\nwait\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Retried: in a parallel test harness another thread can fork while
        // the just-written script's fd is briefly held, and the exec then
        // fails ETXTBSY. Transient by nature.
        let mut wrapper = None;
        for _ in 0..40 {
            match std::process::Command::new(&script)
                .args(["Test", "Game", "0", "0", "sleep", "30"])
                .spawn()
            {
                Ok(child) => {
                    wrapper = Some(child);
                    break;
                }
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(e) => panic!("wrapper spawn failed: {e}"),
            }
        }
        let mut wrapper = wrapper.expect("wrapper spawn kept hitting ETXTBSY");
        let wrapper_pid = wrapper.id();

        // Find the sleep child by scanning /proc for ppid == wrapper.
        let mut child_pid = None;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            child_pid = std::fs::read_dir("/proc").ok().and_then(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok())
                    .find(|&pid| read_ppid(pid) == Some(wrapper_pid))
            });
            if child_pid.is_some() {
                break;
            }
        }
        let child = child_pid.expect("wrapper never spawned its child");

        let found = identify_via_lutris_ancestor(child);
        let _ = wrapper.kill();
        let _ = wrapper.wait();
        let _ = std::fs::remove_dir_all(&dir);

        let (name, _exe) = found.expect("layer 4 failed to find the lutris-wrapper ancestor");
        assert_eq!(name, "Test Game");
    }

    #[test]
    fn hint_replaces_default_names_only() {
        let mut e = Enricher::with_naming(None);
        e.name_hints.insert(42, "Cool Game".to_string());

        // Empty name (stem-cleared) → hint applies.
        let mut empty = Activity::new("pid_42");
        empty.process_id = 42;
        e.apply_naming(&mut empty);
        assert_eq!(empty.name, "Cool Game");

        // The executable stem is a *default* name, not a curated one - this
        // is the Brotato.x86_64 case, and the hint must beat it.
        let mut stem = Activity::new("pid_42");
        stem.process_id = 42;
        stem.executable = "/games/Brotato.x86_64".to_string();
        stem.name = "Brotato".to_string(); // == file_stem, set by from_gamemode
        e.apply_naming(&mut stem);
        assert_eq!(stem.name, "Cool Game");

        // A curated name → hint never touches it.
        let mut named = Activity::new("pid_42");
        named.process_id = 42;
        named.executable = "/games/Brotato.x86_64".to_string();
        named.name = "Curated Name".to_string();
        e.apply_naming(&mut named);
        assert_eq!(named.name, "Curated Name");
    }

    #[test]
    fn hint_loses_to_detectable_json() {
        let json = r#"[{"name": "DB Name", "executables": [{"name": "game.exe"}], "third_party_skus": []}]"#;
        let db = NamingDb::parse(json).unwrap();
        let mut e = Enricher::with_naming(Some(db));
        e.name_hints.insert(7, "Hint Name".to_string());

        let mut activity = Activity::new("pid_7");
        activity.process_id = 7;
        activity.executable = "C:/Games/game.exe".to_string();
        e.apply_naming(&mut activity);
        // detectable.json wins; the hint is one rung below it.
        assert_eq!(activity.name, "DB Name");
    }

    #[test]
    fn hint_never_applies_to_pid_zero() {
        let mut e = Enricher::with_naming(None);
        e.name_hints.insert(0, "Ghost".to_string());
        let mut activity = Activity::new("discord_0");
        activity.process_id = 0;
        e.apply_naming(&mut activity);
        assert!(activity.name.is_empty());
    }

    #[test]
    fn name_hint_event_is_swallowed_when_no_group_matches() {
        let mut e = Enricher::with_naming(None);
        // No groups, no tracked pids: the hint is stored, nothing is emitted,
        // and the correlator never sees a NameHint.
        let out = e.process(SourceEvent::NameHint {
            pid: 999_999,
            name: "Player".to_string(),
        });
        assert!(out.is_empty());
        assert_eq!(
            e.name_hints.get(&999_999).map(String::as_str),
            Some("Player")
        );
    }

    #[test]
    fn group_member_hint_names_the_representative() {
        use crate::group::{GameGroup, Member, MemberClass};
        let mut e = Enricher::with_naming(None);

        // A group with rep 100 and member 101; only the member has a hint.
        let mut g = GameGroup::new("steam:555", 1_000);
        let member = |gm| Member {
            gamemode: gm,
            scan: false,
            class: MemberClass::Plain,
            depth: 1,
            start_time: None,
            alive: true,
        };
        g.upsert(100, member(true), false);
        g.upsert(101, member(true), false);
        e.pid_to_group.insert(100, "steam:555".to_string());
        e.pid_to_group.insert(101, "steam:555".to_string());
        e.groups.insert("steam:555".to_string(), g);
        e.name_hints.insert(101, "Member Hint".to_string());

        assert_eq!(
            e.group_hint("steam:555").as_deref(),
            Some("Member Hint"),
            "a member's hint must be reachable for the rep's record"
        );
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

    #[test]
    fn the_stash_mirrors_the_elected_identity_not_the_last_claim() {
        // The Project Hospital incident: the real game's identity is elected
        // first; the crash handler's same-confidence claim for another game
        // arrives later. The group refuses the sideways overwrite — and the
        // stash must agree with the group, not with whichever claim came
        // last. (Before the fix the stash flipped to "Spellcraft".)
        use crate::group::MemberClass;
        let key = "gog:1660194629";
        let mut e = Enricher::with_naming(None);
        e.umu_report = crate::umu_report::UmuReport::from_path(
            std::env::temp_dir().join("gamebus-test-elected-identity.json"),
        );
        e.stash_keys.insert(
            key.to_string(),
            (
                "gog".to_string(),
                Some("1660194629".to_string()),
                key.to_string(),
            ),
        );
        e.umu_report
            .note_launch("gog", Some("1660194629"), "umu-0", key);

        let identity = |name: &str, exe: &str| {
            Some((
                Identity {
                    name: name.to_string(),
                    exe: exe.to_string(),
                    class: IdentityClass::GameProcess,
                },
                IdentitySource::Curated,
            ))
        };
        e.grouped_update(
            key,
            11,
            group_member(MemberClass::GameProcess),
            identity("Project Hospital", "ProjectHospital.exe"),
            Activity::from_gamemode(11, "ProjectHospital.exe", 1_700_000_000),
            None,
        );
        e.grouped_update(
            key,
            12,
            group_member(MemberClass::GameProcess),
            identity("Spellcraft", "UnityCrashHandler64.exe"),
            Activity::from_gamemode(12, "UnityCrashHandler64.exe", 1_700_000_001),
            None,
        );

        let entry = &e.umu_report.entries()[key];
        assert_eq!(
            entry.title.as_deref(),
            Some("Project Hospital"),
            "stash disagreed with the elected identity"
        );
        assert_eq!(entry.executable.as_deref(), Some("ProjectHospital.exe"));
    }

    /// A live, gamemode-registered group member of the given class.
    fn group_member(class: MemberClass) -> Member {
        Member {
            gamemode: true,
            scan: false,
            class,
            depth: 0,
            start_time: None,
            alive: true,
        }
    }

    #[test]
    fn migrate_emits_updated_before_removed_and_moves_steam_partial() {
        // The four-event order, publish-first, and the
        // group's one Steam partial moving with the rep.
        let mut e = Enricher::with_naming(None);

        let events = e.grouped_update(
            "steam:480",
            100,
            group_member(MemberClass::Helper),
            None,
            gamemode_activity(100),
            Some(Activity::from_steam(100, "480")),
        );
        assert_eq!(
            events.len(),
            2,
            "first member publishes rep + steam partial"
        );
        assert_eq!(e.steam_ids.get(&100), Some(&"steam_100".to_string()));

        // A strictly higher class arrives: one Migrate, publish-first.
        let events = e.grouped_update(
            "steam:480",
            101,
            group_member(MemberClass::GameProcess),
            None,
            gamemode_activity(101),
            None,
        );
        assert_eq!(events.len(), 4);
        let SourceEvent::Updated(a) = &events[0] else {
            panic!("expected Updated first, got {events:?}");
        };
        assert_eq!(a.id, "pid_101");
        let SourceEvent::Updated(sa) = &events[1] else {
            panic!("expected Steam Updated second, got {events:?}");
        };
        assert_eq!(sa.id, "steam_101");
        assert_eq!(sa.app_ids.get("steam").unwrap(), "480");
        // Steam removal before GameMode removal (the old record degrades
        // in place and dies once - no transient steam_<old> flash).
        assert_eq!(
            events[2],
            SourceEvent::Removed {
                id: "steam_100".to_string(),
                source: Source::Steam,
            }
        );
        assert_eq!(
            events[3],
            SourceEvent::Removed {
                id: "pid_100".to_string(),
                source: Source::GameMode,
            }
        );
        // Steam bookkeeping moved to the new rep.
        assert_eq!(e.steam_ids.get(&101), Some(&"steam_101".to_string()));
        assert!(!e.steam_ids.contains_key(&100));
        assert!(!e.steam_appids.contains_key(&100));
    }

    #[test]
    fn absorbed_member_death_emits_nothing() {
        // An absorbed member was never on the bus, so neither
        // its arrival nor its death produces events (the Brotato kill shot).
        let mut e = Enricher::with_naming(None);
        e.grouped_update(
            "steam:480",
            100,
            group_member(MemberClass::Helper),
            None,
            gamemode_activity(100),
            None,
        );
        let events = e.grouped_update(
            "steam:480",
            101,
            group_member(MemberClass::Helper),
            None,
            gamemode_activity(101),
            None,
        );
        // The only event is the defensive cleanup for a cache-adopted
        // record under this pid (no-op on the bus when none exists) - the
        // member itself is never published.
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], SourceEvent::Removed { id, source: Source::GameMode } if id == "pid_101"),
            "absorb emits only the cache-adoption cleanup: {events:?}"
        );

        let events = e.process(SourceEvent::Removed {
            id: "pid_101".to_string(),
            source: Source::GameMode,
        });
        assert!(events.is_empty(), "absorbed member death is invisible");

        // The rep's departure with no evidence left removes the record.
        let events = e.process(SourceEvent::Removed {
            id: "pid_100".to_string(),
            source: Source::GameMode,
        });
        assert_eq!(
            events,
            vec![SourceEvent::Removed {
                id: "pid_100".to_string(),
                source: Source::GameMode,
            }]
        );
        assert!(e.groups.is_empty());
        assert!(e.pid_to_group.is_empty());
    }

    #[test]
    fn scan_gate_steamapps_and_appid_only() {
        // A /steamapps/ exe with a resolvable appid admits a
        // new group; a plain exe with a stray appid does not (sleep guard).
        let json = r#"[{"name": "Brotato", "executables": [], "third_party_skus": [{"distributor": "steam", "id": "1942280"}]}]"#;
        let db = NamingDb::parse(json).unwrap();
        let e = Enricher::with_naming(Some(db));

        assert!(e.new_group_gate(
            999_999_999,
            "/home/u/.local/share/Steam/steamapps/common/Brotato/Brotato.x86_64",
            "1942280"
        ));
        assert!(
            !e.new_group_gate(999_999_999, "/usr/bin/sleep", "1942280"),
            "a stray SteamAppId on a plain exe must stay unpublishable"
        );
        assert!(
            !e.new_group_gate(
                999_999_999,
                "/home/u/.local/share/Steam/steamapps/common/Foo/Foo",
                "99999"
            ),
            "an unresolvable appid does not pass the steamapps gate"
        );
    }

    #[test]
    fn retry_unresolved_grouped_updates_rep_in_place_no_duplicate() {
        // Late identify_wrapper success for a grouped pid sets
        // the GROUP identity and refreshes the current rep in place - it
        // never constructs a fresh activity for the wrapper pid (the latent
        // duplicate-record bug at the old enricher.rs:319-332).
        let own = std::process::id();
        let exe = std::env::current_exe().unwrap();
        let exe_name = exe.file_name().unwrap().to_str().unwrap().to_lowercase();
        let json = format!(
            r#"[{{"name": "Fake Game", "executables": [{{"name": "{exe_name}"}}], "third_party_skus": []}}]"#
        );
        let db = NamingDb::parse(&json).unwrap();
        let mut e = Enricher::with_naming(Some(db));

        e.grouped_update(
            "steam:480",
            100,
            group_member(MemberClass::Helper),
            None,
            gamemode_activity(100),
            None,
        );
        let events = e.grouped_update(
            "steam:480",
            own,
            group_member(MemberClass::Helper),
            None,
            gamemode_activity(own),
            None,
        );
        // Absorb emits exactly the defensive removal for a possible
        // cache-adopted record - bus-invisible when nothing was published.
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], SourceEvent::Removed { id, source: Source::GameMode } if *id == format!("pid_{own}")),
            "absorb must emit only the cache-adoption cleanup: {events:?}"
        );
        e.unresolved_wrappers.insert(own);

        // identify_wrapper resolves via our own cmdline (layer 1).
        let events = e.retry_unresolved();
        assert_eq!(events.len(), 1);
        let SourceEvent::Updated(a) = &events[0] else {
            panic!("expected Updated, got {events:?}");
        };
        assert_eq!(a.id, "pid_100", "the CURRENT rep is refreshed in place");
        assert_eq!(a.process_id, 100);
        assert_eq!(a.name, "Fake Game");
        assert!(e.unresolved_wrappers.is_empty());
        assert!(e.groups.get("steam:480").unwrap().identity.is_some());
    }

    #[test]
    fn wrapper_cmdline_game_token_never_classifies_game_process() {
        // A Steam wrapper's cmdline carries the full launch command, game
        // binary included (`reaper SteamLaunch ... /steamapps/.../Brotato`).
        // The cmdline identification layer must not inflate the helper to
        // GameProcess - a rep pinned at that class would block the real
        // game's strictly-greater dethrone (helpers are judged on the
        // raw exe).
        let json = r#"[{"name": "Brotato", "executables": [{"name": "brotato.x86_64"}], "third_party_skus": []}]"#;
        let db = NamingDb::parse(json).unwrap();
        let e = Enricher::with_naming(Some(db));

        // A shell posing as the wrapper, with the game binary in its argv
        // ($0). The `;:` suffix stops the shell exec-optimising itself away.
        let mut child = std::process::Command::new("sh")
            .args([
                "-c",
                "sleep 30;:",
                "/steamapps/common/Brotato/Brotato.x86_64",
            ])
            .spawn()
            .expect("spawn sh");
        let pid = child.id();
        // The cmdline appears with the exec; poll briefly.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let cmdline =
                std::fs::read_to_string(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            if cmdline.contains("Brotato.x86_64") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let raw_exe = std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .and_then(|p| p.to_str().map(str::to_string))
            .unwrap_or_default();
        assert!(
            is_wrapper_executable(&raw_exe),
            "sh resolves to a listed wrapper: {raw_exe}"
        );
        // The trap being guarded: the cmdline layer DOES identify this pid.
        assert!(identify_process(pid, e.naming.as_ref().unwrap()).is_some());

        let (class, identity) = e.classify_member(pid, &raw_exe, "steam:1942280", None);
        assert_eq!(
            class,
            MemberClass::Helper,
            "a wrapper must never inflate to GameProcess via its cmdline"
        );
        assert!(identity.is_none());

        // With the walk's identification it is an IdentifiedWrapper: the
        // record stays named, the strictly-greater dethrone stays open.
        let walked = (
            "Brotato".to_string(),
            "/steamapps/common/Brotato/Brotato.x86_64".to_string(),
            IdentitySource::Walk,
        );
        let (class, identity) = e.classify_member(pid, &raw_exe, "steam:1942280", Some(&walked));
        assert_eq!(class, MemberClass::IdentifiedWrapper);
        assert_eq!(identity.unwrap().0.class, IdentityClass::Wrapper);

        child.kill().ok();
        child.wait().ok();
    }

    #[test]
    fn a_steam_install_names_itself_from_its_own_manifest() {
        // The live case (Danger Scavenger via Steam, 2026-08-30): a real
        // Steam app absent from detectable.json published nothing at all.
        let dir = std::env::temp_dir().join(format!("gamebus-manifest-{}", std::process::id()));
        let steamapps = dir.join("SteamLibrary/steamapps");
        let game_dir = steamapps.join("common/Danger Scavenger");
        std::fs::create_dir_all(&game_dir).unwrap();
        std::fs::write(
            steamapps.join("appmanifest_1169740.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"1169740\"\n\t\"name\"\t\t\"Danger Scavenger\"\n}\n",
        )
        .unwrap();
        let exe = game_dir.join("Danger_Scavenger.x86_64");
        let exe = exe.to_str().unwrap();

        // The parser finds the name; a wrong appid finds nothing.
        assert_eq!(
            steam_manifest_name(exe, "1169740").as_deref(),
            Some("Danger Scavenger")
        );
        assert_eq!(steam_manifest_name(exe, "999"), None);
        assert_eq!(steam_manifest_name("/usr/bin/sleep", "1169740"), None);

        // classify_member: a steam-keyed non-wrapper member takes the
        // manifest name as a GameProcess-class identity, member Plain.
        let e = Enricher::with_naming(None);
        let (class, identity) = e.classify_member(std::process::id(), exe, "steam:1169740", None);
        assert_eq!(class, MemberClass::Plain);
        let (id, source) = identity.expect("the manifest names the install");
        assert_eq!(id.name, "Danger Scavenger");
        assert_eq!(id.class, IdentityClass::GameProcess);
        assert_eq!(source, IdentitySource::SteamManifest);

        // The stash arm records the gap - but only the gap. A db that
        // knows the appid means an ordinary Steam launch: no trace. No db
        // at all means "not obvious" cannot be judged: no trace either.
        let db = NamingDb::parse(r#"[{"name":"Known Game","executables":[],"id":"1","third_party_skus":[{"id":"555","distributor":"steam"}]}]"#).unwrap();
        let mut e2 = Enricher::with_naming(Some(db));
        e2.maybe_stash_launch("steam:1169740", "", exe);
        assert_eq!(
            e2.umu_report.entries()["steam:1169740"]
                .launcher_name
                .as_deref(),
            Some("Danger Scavenger"),
            "an appid detectable does not know, named by its manifest, is a recorded gap"
        );
        let m = &e2.umu_report.entries()["steam:1169740"];
        assert_eq!(m.store, "steam");
        assert_eq!(m.codename.as_deref(), Some("1169740"));
        assert_eq!(m.codename_source.as_deref(), Some("steam-manifest"));
        assert!(!m.is_umu_miss());
        e2.maybe_stash_launch("steam:555", "", exe);
        assert!(
            !e2.umu_report.entries().contains_key("steam:555"),
            "a game detectable knows leaves no trace"
        );
        let mut e3 = Enricher::with_naming(None);
        e3.maybe_stash_launch("steam:1169740", "", exe);
        assert!(
            e3.umu_report.entries().is_empty(),
            "without a database, obviousness cannot be judged - no trace"
        );

        // The scan gate accepts it with no naming database at all, and
        // still refuses anything outside a steamapps library (R5).
        assert!(e.new_group_gate(std::process::id(), exe, "1169740"));
        assert!(!e.new_group_gate(std::process::id(), "/usr/bin/sleep", "1169740"));
        assert!(!e.new_group_gate(std::process::id(), exe, "999"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_native_game_binary_takes_its_title_from_the_lutris_wrapper_ancestor() {
        // The live case (Danger Scavenger, itch.io via Lutris, 2026-08-23):
        // `lutris-wrapper Danger Scavenger 0 0 gamemoderun ./Game.x86_64`.
        // The binary registers with GameMode itself, its exe is no wrapper,
        // detectable.json does not know it - and the bus showed the stem.
        let dir = std::env::temp_dir().join(format!("gamebus-native-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("lutris-wrapper");
        std::fs::write(&script, "#!/bin/sh\nsleep 30 &\nwait\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut wrapper = None;
        for _ in 0..40 {
            match std::process::Command::new(&script)
                .args([
                    "Danger",
                    "Scavenger",
                    "0",
                    "0",
                    "gamemoderun",
                    "./Danger_Scavenger.x86_64",
                ])
                .spawn()
            {
                Ok(child) => {
                    wrapper = Some(child);
                    break;
                }
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(e) => panic!("wrapper spawn failed: {e}"),
            }
        }
        let mut wrapper = wrapper.expect("wrapper spawn kept hitting ETXTBSY");
        let wrapper_pid = wrapper.id();
        let mut child_pid = None;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            child_pid = std::fs::read_dir("/proc").ok().and_then(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok())
                    .find(|&pid| read_ppid(pid) == Some(wrapper_pid))
            });
            if child_pid.is_some() {
                break;
            }
        }
        let child = child_pid.expect("wrapper never spawned its child");

        // No naming database at all: this is the one layer that works
        // without one, and it must reach a non-wrapper member.
        let e = Enricher::with_naming(None);
        let (class, identity) = e.classify_member(
            child,
            "/games/danger-scavenger/Danger_Scavenger.x86_64",
            "lutris:test",
            None,
        );
        let _ = wrapper.kill();
        let _ = wrapper.wait();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            class,
            MemberClass::Plain,
            "the launcher title must not inflate the member's class"
        );
        let (identity, source) =
            identity.expect("the lutris-wrapper ancestor names a native game binary");
        assert_eq!(identity.name, "Danger Scavenger");
        assert_eq!(
            identity.class,
            IdentityClass::Wrapper,
            "a launcher title is launcher-class, so a curated hit can still win"
        );
        assert_eq!(
            source,
            IdentitySource::LutrisArgv,
            "the provenance tag says the argv named it"
        );
    }

    #[test]
    fn group_teardown_removes_steam_before_gamemode() {
        // Exactly-one-ActivityRemoved: the Steam partial goes first so
        // the correlator degrades the record in place and it dies with its
        // last source - no transient steam_<rep> Added+Removed flash.
        let mut e = Enricher::with_naming(None);
        e.grouped_update(
            "steam:480",
            100,
            group_member(MemberClass::Helper),
            None,
            gamemode_activity(100),
            Some(Activity::from_steam(100, "480")),
        );
        let events = e.process(SourceEvent::Removed {
            id: "pid_100".to_string(),
            source: Source::GameMode,
        });
        assert_eq!(
            events,
            vec![
                SourceEvent::Removed {
                    id: "steam_100".to_string(),
                    source: Source::Steam,
                },
                SourceEvent::Removed {
                    id: "pid_100".to_string(),
                    source: Source::GameMode,
                },
            ]
        );
        assert!(e.groups.is_empty());
    }

    #[test]
    fn grouped_pid_keeps_steam_partial_on_foreign_source_removal() {
        // A grouped pid's Steam partial is owned by the group lifecycle
        // (same guard as on_source_lost and the scan reconcile): a detaching
        // Discord partial must not reap it and kill a scan-backed record.
        let mut e = Enricher::new();
        e.pid_to_group.insert(100, "steam:480".to_string());
        e.steam_ids.insert(100, "steam_100".to_string());
        e.active_sources
            .entry(100)
            .or_default()
            .insert(Source::Discord);

        let events = e.process(SourceEvent::Removed {
            id: "discord_100".to_string(),
            source: Source::Discord,
        });
        assert_eq!(
            events,
            vec![SourceEvent::Removed {
                id: "discord_100".to_string(),
                source: Source::Discord,
            }],
            "only the Discord removal is forwarded"
        );
        assert!(
            e.steam_ids.contains_key(&100),
            "the group keeps its Steam partial"
        );
        assert!(!e.active_sources.contains_key(&100));
    }

    #[test]
    fn scan_created_group_adopts_first_gamemode_since() {
        // A scan-created group is born blind (since 0); the first GameMode
        // evidence supplies the authoritative timestamp, set-once - a later
        // event can never move it (Since never jumps).
        let mut e = Enricher::with_naming(None);
        let mut group = GameGroup::new("steam:480", 0);
        group.steam_appid = Some("480".to_string());
        group.upsert(
            100,
            Member {
                gamemode: false,
                scan: true,
                class: MemberClass::GameProcess,
                depth: 3,
                start_time: None,
                alive: true,
            },
            false,
        );
        e.groups.insert("steam:480".to_string(), group);
        e.steam_only_groups.insert("steam:480".to_string());
        e.pid_to_group.insert(100, "steam:480".to_string());

        let events = e.grouped_update(
            "steam:480",
            100,
            group_member(MemberClass::GameProcess),
            None,
            gamemode_activity(100),
            None,
        );
        let SourceEvent::Updated(a) = &events[0] else {
            panic!("expected Updated, got {events:?}");
        };
        assert_eq!(a.since, 1_700_000_100, "GameMode's real timestamp is kept");
        assert_eq!(e.groups["steam:480"].since, 1_700_000_100);

        // Set-once: a later GameMode event cannot move it.
        let mut later = gamemode_activity(100);
        later.since = 1_700_000_999;
        let events = e.grouped_update(
            "steam:480",
            100,
            group_member(MemberClass::GameProcess),
            None,
            later,
            None,
        );
        let SourceEvent::Updated(a) = &events[0] else {
            panic!("expected Updated, got {events:?}");
        };
        assert_eq!(a.since, 1_700_000_100);
    }

    #[test]
    fn liveness_start_time_mismatch_is_dead() {
        // The pid-reuse guard - same pid, same key, different
        // start time means a recycled pid, and the member is dead.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .env("SteamAppId", "90001")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        let start = cache::process_start_time(pid).expect("child start time");

        // The environ appears with the exec; poll briefly.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !member_alive(pid, "steam:90001", Some(start)) && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(member_alive(pid, "steam:90001", Some(start)));
        assert!(
            !member_alive(pid, "steam:90001", Some(start + 1)),
            "start-time mismatch is a recycled pid"
        );
        assert!(
            !member_alive(pid, "steam:42", Some(start)),
            "key mismatch is dead"
        );

        child.kill().ok();
        child.wait().ok();
        assert!(
            !member_alive(pid, "steam:90001", Some(start)),
            "a vanished process is dead"
        );
    }

    /// An enricher with a stash on a unique scratch path, removed first so a
    /// previous run can never leak entries into the asserts.
    fn with_scratch_stash(name: &str) -> Enricher {
        let mut e = Enricher::with_naming(None);
        let path =
            std::env::temp_dir().join(format!("gamebus-test-{name}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        e.umu_report = UmuReport::from_path(path);
        e
    }

    #[test]
    fn a_native_lutris_launch_records_launcher_facts_and_marker_codename() {
        // The live Danger Scavenger shape (itch.io via Lutris, 2026-08-23):
        // no umu marker, but Lutris hands us the store, the name, the
        // directory — and wrote the codename beside the game.
        let mut e = with_scratch_stash("ds-facts");
        let dir = std::env::temp_dir().join(format!("gamebus-ds-marker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".lutrisgame.json"),
            r#"{"slug":"danger-scavenger","runner":"linux","appid":"926077","upload":"4665094","service":"itchio","date":1787522302}"#,
        )
        .unwrap();
        let exe = "/media/Data/Spiele/itchio/danger-scavenger/Danger_Scavenger.x86_64";
        let environ = format!(
            "LUTRIS_GAME_UUID=uuid-1\0STORE=itchio\0GAME_NAME=Danger Scavenger\0GAME_DIRECTORY={}\0",
            dir.display()
        );
        e.maybe_stash_launch("lutris:uuid-1", &environ, exe);

        let entry = e.umu_report.entries()["itchio:926077"].clone();
        assert_eq!(entry.umu_id, "", "never went through umu");
        assert!(!entry.is_umu_miss());
        assert_eq!(entry.store, "itchio");
        assert_eq!(entry.codename.as_deref(), Some("926077"));
        assert_eq!(entry.codename_source.as_deref(), Some("lutris-config"));
        assert_eq!(entry.launcher.as_deref(), Some("lutris"));
        assert_eq!(entry.launcher_name.as_deref(), Some("Danger Scavenger"));
        assert_eq!(entry.launcher_dir.as_deref(), dir.to_str());
        assert_eq!(entry.runner.as_deref(), Some("native"));
        assert!(entry.title.is_none(), "no identity elected yet");

        // The group identity arrives off the lutris-wrapper argv: the stash
        // entry gains the title under its own provenance label.
        e.grouped_update(
            "lutris:uuid-1",
            11,
            group_member(MemberClass::Plain),
            Some((
                Identity {
                    name: "Danger Scavenger".to_string(),
                    exe: exe.to_string(),
                    class: IdentityClass::Wrapper,
                },
                IdentitySource::LutrisArgv,
            )),
            Activity::from_gamemode(11, exe, 1_700_000_000),
            None,
        );
        let entry = &e.umu_report.entries()["itchio:926077"];
        assert_eq!(entry.title.as_deref(), Some("Danger Scavenger"));
        assert_eq!(entry.title_source.as_deref(), Some("lutris-wrapper"));
        assert_eq!(entry.confidence, Some(Confidence::Medium));
        assert_eq!(entry.executable.as_deref(), Some(exe));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_heroic_umu_miss_records_the_same_entry_plus_launcher_facts() {
        // The pre-S9c Heroic umu-0 shape must produce the entry it always
        // did — same key, same store guess, same codename, same umu id —
        // with the launcher facts added beside it.
        let mut e = with_scratch_stash("heroic-facts");
        let environ =
            "SteamAppId=0\0GAMEID=umu-0\0HEROIC_APP_SOURCE=epic\0HEROIC_APP_NAME=Calluna\0";
        let key = merge_key_from_environ(environ).expect("heroic key");
        assert_eq!(key, "heroic:Calluna");
        e.maybe_stash_launch(&key, environ, "/usr/bin/wine64-preloader");

        let entry = &e.umu_report.entries()["egs:Calluna"];
        // The entry as the pre-widening daemon wrote it.
        assert_eq!(entry.store, "egs");
        assert_eq!(entry.codename.as_deref(), Some("Calluna"));
        assert_eq!(entry.umu_id, "umu-0");
        assert!(entry.is_umu_miss());
        assert!(entry.title.is_none());
        assert!(entry.title_source.is_none());
        assert!(entry.confidence.is_none());
        assert!(entry.executable.is_none());
        // The new facts beside it. launcher_name is a live lookup into the
        // machine's Heroic library and is deliberately not asserted.
        assert_eq!(entry.launcher.as_deref(), Some("heroic"));
        assert_eq!(entry.codename_source.as_deref(), Some("heroic-env"));
        assert_eq!(entry.runner.as_deref(), Some("proton"));
        assert_eq!(
            e.stash_keys["heroic:Calluna"],
            (
                "egs".to_string(),
                Some("Calluna".to_string()),
                "heroic:Calluna".to_string()
            )
        );
    }

    #[test]
    fn repeated_lutris_umu_launches_collapse_onto_the_game_name_slug() {
        // A Lutris Wine launch (Control through umu-default): the merge key
        // carries a fresh uuid every launch, but the stash must keep ONE
        // entry — keyed by the GAME_NAME slug when no codename exists.
        let mut e = with_scratch_stash("control-collapse");
        let exe = "/usr/bin/wine64-preloader";
        for uuid in ["uuid-1", "uuid-2"] {
            let environ = format!(
                "LUTRIS_GAME_UUID={uuid}\0UMU_ID=umu-default\0STORE=egs\0GAME_NAME=Control\0"
            );
            e.maybe_stash_launch(&format!("lutris:{uuid}"), &environ, exe);
        }

        assert_eq!(e.umu_report.entries().len(), 1, "one game, one entry");
        let entry = &e.umu_report.entries()["lutris:control"];
        assert_eq!(entry.umu_id, "umu-default");
        assert!(entry.is_umu_miss());
        assert_eq!(entry.store, "egs");
        assert_eq!(entry.codename, None, "no marker file, no codename");
        assert_eq!(entry.codename_source, None);
        assert_eq!(entry.launcher.as_deref(), Some("lutris"));
        assert_eq!(entry.launcher_name.as_deref(), Some("Control"));
        assert_eq!(entry.runner.as_deref(), Some("proton"));
        // Both per-launch merge keys resolve to the same stash entry, so a
        // later title lands on it whichever launch the group came from.
        assert_eq!(e.stash_keys["lutris:uuid-1"].2, "lutris:control");
        assert_eq!(e.stash_keys["lutris:uuid-2"].2, "lutris:control");
    }

    #[test]
    fn steam_and_curated_umu_launches_never_enter_the_stash() {
        // Authoritative identities: a Steam appid, or a real umu id without
        // the miss marker. Neither is a gap; neither is recorded.
        let mut e = with_scratch_stash("no-stash");
        e.maybe_stash_launch(
            "steam:480",
            "SteamAppId=480\0",
            "/steamapps/common/Game/game.exe",
        );
        e.maybe_stash_launch(
            "umu:testgame",
            "UMU_ID=umu-testgame\0STORE=egs\0",
            "/usr/bin/wine64-preloader",
        );
        assert!(e.umu_report.entries().is_empty());
        assert!(e.stash_keys.is_empty());
    }

    #[test]
    fn a_malformed_lutris_marker_yields_no_codename_and_no_panic() {
        let mut e = with_scratch_stash("bad-marker");
        let dir = std::env::temp_dir().join(format!("gamebus-bad-marker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".lutrisgame.json"), "not json {").unwrap();
        let environ = format!(
            "LUTRIS_GAME_UUID=uuid-3\0STORE=itchio\0GAME_NAME=Danger Scavenger\0GAME_DIRECTORY={}\0",
            dir.display()
        );
        e.maybe_stash_launch(
            "lutris:uuid-3",
            &environ,
            "/media/Data/Spiele/itchio/danger-scavenger/Danger_Scavenger.x86_64",
        );

        // No codename means the slug fallback keys the entry.
        let entry = &e.umu_report.entries()["lutris:danger-scavenger"];
        assert_eq!(entry.codename, None);
        assert_eq!(entry.codename_source, None);
        assert_eq!(entry.store, "itchio");
        assert_eq!(entry.runner.as_deref(), Some("native"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lutris_marker_reads_appid_and_swallows_malformed_files() {
        let dir = std::env::temp_dir().join(format!("gamebus-marker-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir_str = dir.to_str().unwrap();

        // The live Danger Scavenger marker.
        std::fs::write(
            dir.join(".lutrisgame.json"),
            r#"{"slug":"danger-scavenger","runner":"linux","appid":"926077","upload":"4665094","service":"itchio","date":1787522302}"#,
        )
        .unwrap();
        assert_eq!(lutris_marker_appid(dir_str).as_deref(), Some("926077"));

        // A numeric appid is accepted too.
        std::fs::write(dir.join(".lutrisgame.json"), r#"{"appid":926077}"#).unwrap();
        assert_eq!(lutris_marker_appid(dir_str).as_deref(), Some("926077"));

        // Malformed JSON, a missing appid, and a missing file are silent.
        std::fs::write(dir.join(".lutrisgame.json"), "not json {").unwrap();
        assert_eq!(lutris_marker_appid(dir_str), None);
        std::fs::write(dir.join(".lutrisgame.json"), r#"{"slug":"x"}"#).unwrap();
        assert_eq!(lutris_marker_appid(dir_str), None);
        std::fs::write(dir.join(".lutrisgame.json"), r#"{"appid":""}"#).unwrap();
        assert_eq!(lutris_marker_appid(dir_str), None);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(lutris_marker_appid(dir_str), None);
    }

    #[test]
    fn runner_classification_table() {
        // The umu marker settles it regardless of the exe.
        assert_eq!(runner_of(true, "/games/Game.x86_64"), "proton");
        // Wine runtime binaries.
        assert_eq!(runner_of(false, "/usr/bin/wine64-preloader"), "proton");
        assert_eq!(runner_of(false, "/opt/wine/bin/wine-preloader"), "proton");
        assert_eq!(runner_of(false, "/runners/wine/bin/wine64"), "proton");
        assert_eq!(runner_of(false, "/runners/wine/bin/wine"), "proton");
        // A Windows binary under a prefix.
        assert_eq!(
            runner_of(false, "/prefix/drive_c/Games/Game/Game.exe"),
            "proton"
        );
        // Native Linux binaries.
        assert_eq!(
            runner_of(
                false,
                "/media/Data/Spiele/itchio/danger-scavenger/Danger_Scavenger.x86_64"
            ),
            "native"
        );
        assert_eq!(runner_of(false, "/usr/bin/python3"), "native");
        assert_eq!(runner_of(false, ""), "native");
    }

    #[test]
    fn slug_lowercases_and_joins_alnum_runs() {
        assert_eq!(slug("Danger Scavenger"), "danger-scavenger");
        assert_eq!(slug("Control"), "control");
        assert_eq!(slug("Left 4 Dead 2"), "left-4-dead-2");
        assert_eq!(slug("Amnesia: The Bunker"), "amnesia-the-bunker");
        assert_eq!(slug("  !! "), "");
    }
}
