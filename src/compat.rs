//! Compat findings: the walls that stop a game, stashed for submission.
//!
//! [`beisl`](https://codeberg.org/fschaupp/beisl) detects that a game hit a
//! wall - a missing Wine export, a vendor kernel anti-cheat, a volume-GUID
//! path bug, an absent EAC/BattlEye runtime - and hands the finding here.
//! This side houses it: the stash below is the raw material for a
//! user-reviewed submission to ProtonDB and AreWeAntiCheatYet, and for the
//! per-target keys gamebus-gamedb carries. beisl enhances the data and
//! fires the trigger; gamebus houses it (owner policy, 2026-09-05).
//!
//! Persisted at `$XDG_DATA_HOME/gamebus-presenced/compat-findings.json` -
//! data, not cache. Keyed exactly like the identity-miss stash
//! (`store:codename`, else the launch merge key), so the two join on the key
//! without either owning the other's file.
//!
//! # Why its own file
//!
//! `umu-misses.json` has two writers that each own half of every entry, and
//! every persist re-reads the file to adopt the other half. A compat finding
//! has a THIRD origin, and putting it in that file would be unsafe in a way
//! that is easy to miss: `Miss` has no catch-all field, so an older
//! `gamebus-presenced` would deserialize an entry without the new field and
//! write the whole map straight back out - silently dropping every finding.
//! The daemon persists on every stashed launch, so one run of a stale daemon
//! would wipe the lot. A separate file an old binary never opens cannot lose
//! this way, and each further source can take its own single-writer file
//! rather than growing an N-way merge.
//!
//! Only `gamebus-setup` reads or writes this file. The daemon never touches
//! it and stays network-free; submitting remains the user's act, not ours.

// Compiled into gamebus-setup only. The daemon has no compat half at all,
// which is the point: an untouched hot path cannot regress.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use crate::umu_report::today;

pub use gamebus_coupler::compat::{canonical_signature, CompatFinding, Observation, WallKind};
use gamebus_coupler::{apply_finding, FindingChange, FindingVerb, Refusal};

/// How many observations one finding keeps. A wall reproduces on every
/// launch; the submission wants the evidence, not the tally. Oldest are
/// dropped first, so the newest wine/GPU combination always survives.
const MAX_OBSERVATIONS: usize = 20;

/// What a source hands over for one finding. Everything optional except the
/// wall itself: a source that knows only "this game hit a kernel anti-cheat"
/// still has something worth stashing.
#[derive(Debug, Clone)]
pub struct Incoming {
    pub wall: WallKind,
    pub title: Option<String>,
    pub steam_appid: Option<String>,
    pub awacy_slug: Option<String>,
    pub observation: Observation,
}

/// The compat-finding stash: keyed like the identity-miss stash, one
/// writer, written through on change.
#[derive(Debug, Default)]
pub struct CompatStash {
    findings: HashMap<String, CompatFinding>,
    path: Option<PathBuf>,
    dirty: bool,
    /// Set when the file exists but did not parse. The path is dropped in
    /// that case so no write can flatten a file we failed to read.
    load_error: Option<String>,
}

impl CompatStash {
    pub fn load() -> Self {
        match Self::default_path() {
            Some(path) => Self::from_path(path),
            None => Self::default(),
        }
    }

    pub(crate) fn from_path(path: PathBuf) -> Self {
        match std::fs::read_to_string(&path) {
            Err(_) => Self {
                path: Some(path),
                ..Self::default()
            },
            Ok(raw) => match serde_json::from_str(&raw) {
                Ok(findings) => Self {
                    findings,
                    path: Some(path),
                    ..Self::default()
                },
                Err(e) => Self {
                    load_error: Some(format!(
                        "{} exists but failed to parse: {e}",
                        path.display()
                    )),
                    ..Self::default()
                },
            },
        }
    }

    /// Why the stash refused to load, if it did. Callers surface this:
    /// showing "no findings" over a corrupt file hides data loss.
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    pub fn default_path() -> Option<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(data.join("gamebus-presenced").join("compat-findings.json"))
    }

    pub fn findings(&self) -> &HashMap<String, CompatFinding> {
        &self.findings
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// True when a change has not reached disk. Checked by callers that
    /// destroy their only other copy of the data once it is saved.
    pub fn unsaved(&self) -> bool {
        self.dirty
    }

    /// Take a finding for `key`, creating or refreshing it. Does not write -
    /// batch, then [`save`](Self::save).
    ///
    /// Facts only ever ADD: a field this delivery leaves empty keeps whatever
    /// an earlier one established. The observation is deduped on
    /// `(source, wine, canonical signature)` - the same wall on the same
    /// build is one piece of evidence seen twice, not two - and a repeat only refreshes
    /// its date, which keeps a wall that reproduces on every launch from
    /// growing the file without bound.
    pub fn record(&mut self, key: &str, incoming: Incoming) {
        let today = today();
        let entry = self
            .findings
            .entry(key.to_string())
            .or_insert_with(|| CompatFinding {
                title: None,
                wall: incoming.wall.clone(),
                steam_appid: None,
                awacy_slug: None,
                first_seen: today.clone(),
                last_seen: today.clone(),
                observations: Vec::new(),
                reported: BTreeMap::new(),
                dismissed: None,
                extra: BTreeMap::new(),
            });

        entry.last_seen = today;
        // A later delivery may sharpen the wall (a generic kind resolving to
        // a specific one), but never blanks it.
        entry.wall = incoming.wall;
        fill(&mut entry.title, incoming.title);
        fill(&mut entry.steam_appid, incoming.steam_appid);
        fill(&mut entry.awacy_slug, incoming.awacy_slug);

        let obs = incoming.observation;
        let canonical = obs.signature.as_deref().map(canonical_signature);
        let same = entry.observations.iter().position(|o| {
            o.source == obs.source
                && o.wine == obs.wine
                && o.signature.as_deref().map(canonical_signature) == canonical
        });
        match same {
            Some(i) => {
                let existing = &mut entry.observations[i];
                existing.observed = obs.observed;
                // Two raw lines that canonicalise the same ARE the proof that
                // the tail is generated, so the stored evidence becomes the
                // form a reader can match. Until then the raw line stands:
                // one sighting is no reason to claim a pattern.
                if existing.signature != obs.signature {
                    existing.signature = canonical;
                }
                // A repeat may carry facts the first sighting lacked.
                fill(&mut existing.gpu, obs.gpu);
                fill(&mut existing.gpu_vendor, obs.gpu_vendor);
                fill(&mut existing.specs, obs.specs);
                fill(&mut existing.log, obs.log);
                fill(&mut existing.trace_dir, obs.trace_dir);
                fill(&mut existing.record_mode, obs.record_mode);
                if existing.layer_split.is_none() {
                    existing.layer_split = obs.layer_split;
                    existing.attributed_pct = obs.attributed_pct;
                }
                for (k, v) in obs.extra {
                    existing.extra.entry(k).or_insert(v);
                }
            }
            None => {
                entry.observations.push(obs);
                let len = entry.observations.len();
                if len > MAX_OBSERVATIONS {
                    entry.observations.drain(..len - MAX_OBSERVATIONS);
                }
            }
        }
        self.dirty = true;
    }

    /// Mutate one finding in place (the review flow). Marks dirty; does not
    /// write.
    pub fn update(&mut self, key: &str, f: impl FnOnce(&mut CompatFinding)) {
        if let Some(entry) = self.findings.get_mut(key) {
            f(entry);
            self.dirty = true;
        }
    }

    /// Mark a finding submitted to `target`, dated today. `None` when no
    /// finding lives under `key`, never a quiet success.
    pub fn mark_reported(&mut self, key: &str, target: &str) -> Option<Result<(), Refusal>> {
        let verb = FindingVerb::MarkReported {
            target: target.to_string(),
        };
        self.apply(key, &verb).map(|r| r.map(|_| ()))
    }

    /// Apply one coupler verb to a finding, dated today. `None` when no
    /// finding lives under `key`; a refusal writes nothing.
    pub fn apply(
        &mut self,
        key: &str,
        verb: &FindingVerb,
    ) -> Option<Result<FindingChange, Refusal>> {
        let day = today();
        let entry = self.findings.get_mut(key)?;
        let result = apply_finding(entry, verb, &day);
        self.dirty |= result.is_ok();
        Some(result)
    }

    /// Persist. Atomic, and adopts any finding only the file knows first, so
    /// a second gamebus-setup running concurrently cannot lose one.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        self.adopt_unseen();
        let Some(path) = &self.path else { return };
        let Ok(json) = serde_json::to_string_pretty(&self.findings) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, path).is_ok() {
            self.dirty = false;
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Keep whole any finding the file has and memory does not. Nobody ever
    /// deletes a finding, so a key we do not hold is one someone else wrote.
    fn adopt_unseen(&mut self) {
        let Some(path) = &self.path else { return };
        let Ok(raw) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(disk) = serde_json::from_str::<HashMap<String, CompatFinding>>(&raw) else {
            return; // unreadable file: memory is the best surviving copy
        };
        for (key, theirs) in disk {
            self.findings.entry(key).or_insert(theirs);
        }
    }
}

/// Set `slot` when it is empty and `value` has something to say. Never
/// clears: a thinner delivery must not blank a fact.
fn fill(slot: &mut Option<String>, value: Option<String>) {
    if slot.is_none() {
        if let Some(v) = value {
            if !v.is_empty() {
                *slot = Some(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stash() -> CompatStash {
        CompatStash::default() // no path: save() no-ops, pure in-memory
    }

    fn obs(source: &str, signature: &str, wine: Option<&str>) -> Observation {
        Observation {
            source: source.to_string(),
            observed: today(),
            wine: wine.map(str::to_string),
            gpu: None,
            gpu_vendor: None,
            specs: None,
            signature: Some(signature.to_string()),
            log: None,
            trace_dir: None,
            layer_split: None,
            attributed_pct: None,
            record_mode: None,
            extra: BTreeMap::new(),
        }
    }

    fn wardogs() -> Incoming {
        Incoming {
            wall: WallKind::KernelAntiCheat,
            title: Some("WARDOGS Playtest".into()),
            steam_appid: Some("4809930".into()),
            awacy_slug: None,
            observation: obs("beisl", "lighthouse_driver.sys", Some("spritzwine-10.0")),
        }
    }

    #[test]
    fn a_finding_lands_under_the_miss_stash_key() {
        let mut s = stash();
        s.record("steam:4809930", wardogs());
        let f = s.findings.get("steam:4809930").expect("finding exists");
        assert_eq!(f.wall, WallKind::KernelAntiCheat);
        assert_eq!(f.steam_appid.as_deref(), Some("4809930"));
        assert_eq!(f.observations.len(), 1);
        assert!(f.submittable());
        assert!(f.needs("awacy"));
    }

    #[test]
    fn the_same_wall_on_the_same_build_is_one_observation() {
        let mut s = stash();
        s.record("steam:4809930", wardogs());
        s.record("steam:4809930", wardogs());
        s.record("steam:4809930", wardogs());
        let f = &s.findings["steam:4809930"];
        assert_eq!(f.observations.len(), 1, "a repeat refreshes, never appends");
    }

    #[test]
    fn a_new_wine_build_is_new_evidence() {
        let mut s = stash();
        s.record("steam:4809930", wardogs());
        let mut newer = wardogs();
        newer.observation = obs("beisl", "lighthouse_driver.sys", Some("spritzwine-10.1"));
        s.record("steam:4809930", newer);
        let f = &s.findings["steam:4809930"];
        assert_eq!(f.observations.len(), 2);
        assert_eq!(
            f.latest().and_then(|o| o.wine.as_deref()),
            Some("spritzwine-10.1")
        );
    }

    #[test]
    fn a_repeat_can_add_facts_but_never_blank_them() {
        let mut s = stash();
        let mut first = wardogs();
        first.observation.gpu = Some("AMD Radeon RX 7900 XT".into());
        s.record("steam:4809930", first);

        // A thinner delivery: same wall, same build, no GPU, no title.
        let mut thin = wardogs();
        thin.title = None;
        thin.observation.log = Some("beisl:run-42".into());
        s.record("steam:4809930", thin);

        let f = &s.findings["steam:4809930"];
        assert_eq!(f.title.as_deref(), Some("WARDOGS Playtest"));
        let o = f.latest().unwrap();
        assert_eq!(o.gpu.as_deref(), Some("AMD Radeon RX 7900 XT"));
        assert_eq!(o.log.as_deref(), Some("beisl:run-42"), "new fact adopted");
    }

    #[test]
    fn observations_are_capped_so_a_reproducing_wall_cannot_grow_the_file() {
        let mut s = stash();
        for i in 0..MAX_OBSERVATIONS + 10 {
            let mut inc = wardogs();
            inc.observation = obs("beisl", "lighthouse_driver.sys", Some(&format!("wine-{i}")));
            s.record("steam:4809930", inc);
        }
        let f = &s.findings["steam:4809930"];
        assert_eq!(f.observations.len(), MAX_OBSERVATIONS);
        assert_eq!(
            f.latest().and_then(|o| o.wine.as_deref()),
            Some(format!("wine-{}", MAX_OBSERVATIONS + 9).as_str()),
            "the newest build survives; the oldest are dropped"
        );
    }

    #[test]
    fn an_unknown_wall_kind_round_trips_instead_of_being_lost() {
        let raw = r#"{"steam:1":{"wall":"gpu-fault-loop","first_seen":"2026-09-05",
                      "last_seen":"2026-09-05","observations":[]}}"#;
        let parsed: HashMap<String, CompatFinding> =
            serde_json::from_str(raw).expect("unknown kind parses");
        let f = &parsed["steam:1"];
        assert_eq!(f.wall, WallKind::Other("gpu-fault-loop".into()));
        let back = serde_json::to_string(&parsed).unwrap();
        assert!(
            back.contains("gpu-fault-loop"),
            "a kind this build does not know survives the round trip"
        );
    }

    #[test]
    fn a_newer_sources_extra_fields_survive_this_build() {
        let raw = r#"{"steam:1":{"wall":"wine-stub","first_seen":"2026-09-05",
                      "last_seen":"2026-09-05","future_field":"keep me",
                      "observations":[{"source":"beisl","observed":"2026-09-05",
                      "shader_stalls":17}]}}"#;
        let parsed: HashMap<String, CompatFinding> = serde_json::from_str(raw).unwrap();
        let back = serde_json::to_string(&parsed).unwrap();
        assert!(back.contains("future_field"), "finding-level extra kept");
        assert!(
            back.contains("shader_stalls"),
            "observation-level extra kept"
        );
    }

    #[test]
    fn marking_reported_takes_a_finding_out_of_that_targets_queue() {
        let mut s = stash();
        s.record("steam:4809930", wardogs());
        assert!(s.findings["steam:4809930"].needs("awacy"));
        assert_eq!(s.mark_reported("steam:4809930", "awacy"), Some(Ok(())));
        assert_eq!(s.mark_reported("steam:nope", "awacy"), None);
        assert!(!s.findings["steam:4809930"].needs("awacy"));
        assert!(
            s.findings["steam:4809930"].needs("protondb"),
            "one target reported does not report the others"
        );
    }

    #[test]
    fn a_dismissed_finding_is_offered_to_nobody() {
        let mut s = stash();
        s.record("steam:4809930", wardogs());
        s.update("steam:4809930", |f| f.dismissed = Some(today()));
        let f = &s.findings["steam:4809930"];
        assert!(!f.submittable());
        assert!(!f.needs("awacy"));
    }

    #[test]
    fn only_anti_cheat_walls_are_awacys_business() {
        assert!(WallKind::KernelAntiCheat.is_anti_cheat());
        assert!(WallKind::MissingRuntime.is_anti_cheat());
        assert!(!WallKind::WineStub.is_anti_cheat());
        assert!(!WallKind::VolumeGuidPath.is_anti_cheat());
    }

    #[test]
    fn a_corrupt_file_is_never_written_over() {
        let dir = std::env::temp_dir().join(format!("compat-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("compat-findings.json");
        std::fs::write(&path, "{ not json").unwrap();

        let mut s = CompatStash::from_path(path.clone());
        assert!(s.load_error().is_some(), "the parse failure is reported");
        s.record("steam:4809930", wardogs());
        s.save();

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "{ not json", "accumulated knowledge beats a session");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_concurrent_writers_finding_is_not_lost() {
        let dir = std::env::temp_dir().join(format!("compat-concurrent-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("compat-findings.json");

        let mut ours = CompatStash::from_path(path.clone());
        ours.record("steam:4809930", wardogs());

        // Someone else wrote a different game while we held ours in memory.
        let mut theirs = CompatStash::from_path(path.clone());
        theirs.record("steam:1867240", wardogs());
        theirs.save();

        ours.save();
        let disk: HashMap<String, CompatFinding> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(disk.contains_key("steam:4809930"), "ours survived");
        assert!(disk.contains_key("steam:1867240"), "theirs survived");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_service_name_generated_per_launch_is_one_observation() {
        // Both lines are real, from two WARDOGS launches on 2026-09-05.
        let mut s = CompatStash::from_path(std::env::temp_dir().join("unused-nonce.json"));
        for sig in [
            r"Services\elytra_tu7-ELBU27khJhcC",
            r"Services\elytra_r7tC-LhUH4-egxUm",
        ] {
            let mut incoming = wardogs();
            incoming.observation.signature = Some(sig.to_string());
            s.record("steam:4809930", incoming);
        }

        let f = &s.findings["steam:4809930"];
        assert_eq!(f.observations.len(), 1, "one wall, seen twice");
        // Two raw lines that agree once collapsed ARE the proof the tail is
        // generated, so the evidence becomes what a reader can match.
        assert_eq!(
            f.observations[0].signature.as_deref(),
            Some(r"Services\elytra_*")
        );
    }

    #[test]
    fn one_sighting_keeps_its_exact_line() {
        let mut s = CompatStash::from_path(std::env::temp_dir().join("unused-single.json"));
        let mut incoming = wardogs();
        incoming.observation.signature = Some(r"Services\elytra_tu7-ELBU27khJhcC".into());
        s.record("steam:4809930", incoming);

        assert_eq!(
            s.findings["steam:4809930"].observations[0]
                .signature
                .as_deref(),
            Some(r"Services\elytra_tu7-ELBU27khJhcC"),
            "a pattern is not claimed from a single observation"
        );
    }

    #[test]
    fn two_drivers_sharing_a_stem_stay_apart() {
        // The same Proton log carries both. Fusing them would lose a driver.
        assert_ne!(
            canonical_signature("elytraldrfs_driver.sys"),
            canonical_signature("elytraldrfs_shared.sys")
        );
        assert_eq!(
            canonical_signature("elytraldrfs_driver.sys"),
            "elytraldrfs_driver.sys"
        );
    }

    #[test]
    fn only_a_mixed_case_digit_bearing_tail_counts_as_generated() {
        // Long enough, and mixes all three classes.
        assert_eq!(canonical_signature("elytra_tu7-ELBU27khJhcC"), "elytra_*");
        // Too short.
        assert_eq!(
            canonical_signature("EasyAntiCheat_x64"),
            "EasyAntiCheat_x64"
        );
        // No digit.
        assert_eq!(canonical_signature("wardogs_Shipping"), "wardogs_Shipping");
        // No uppercase.
        assert_eq!(canonical_signature("mod_a7f3c9d2"), "mod_a7f3c9d2");
        // Nothing to cut.
        assert_eq!(canonical_signature("BEDaisy.sys"), "BEDaisy.sys");
        // Separators and list shape survive intact.
        assert_eq!(
            canonical_signature("lighthouse_driver.sys, elytra_tu7-ELBU27khJhcC"),
            "lighthouse_driver.sys, elytra_*"
        );
    }
}
