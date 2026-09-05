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

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::umu_report::today;

/// How many observations one finding keeps. A wall reproduces on every
/// launch; the submission wants the evidence, not the tally. Oldest are
/// dropped first, so the newest wine/GPU combination always survives.
const MAX_OBSERVATIONS: usize = 20;

/// What stopped the game.
///
/// Serialised as a plain kebab-case string, and an unrecognised one round-
/// trips through [`WallKind::Other`] rather than failing the parse or being
/// dropped: beisl may learn a new wall before this side is rebuilt, and a
/// finding it takes days to reproduce must not be lost to a version skew.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WallKind {
    /// `EXCEPTION_WINE_STUB unimplemented function` - a missing Wine export.
    WineStub,
    /// `ZwLoadDriver` on a non-builtin `.sys` - a vendor kernel anti-cheat,
    /// which no Wine can honour. The AreWeAntiCheatYet signal.
    KernelAntiCheat,
    /// `c00000cb` on a `\??\Volume{...}` path - the volume-GUID path bug.
    VolumeGuidPath,
    /// An EAC or BattlEye game with no Proton runtime present. Reserved:
    /// beisl does not emit this yet and deliberately will not until a real
    /// log line is captured, because a guessed substring would wreck the
    /// exactness the detection depends on (that side, 2026-09-05).
    MissingRuntime,
    /// A kind this build does not know. Preserved verbatim.
    Other(String),
}

impl WallKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::WineStub => "wine-stub",
            Self::KernelAntiCheat => "kernel-anti-cheat",
            Self::VolumeGuidPath => "volume-guid-path",
            Self::MissingRuntime => "missing-runtime",
            Self::Other(s) => s,
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "wine-stub" => Self::WineStub,
            "kernel-anti-cheat" => Self::KernelAntiCheat,
            "volume-guid-path" => Self::VolumeGuidPath,
            "missing-runtime" => Self::MissingRuntime,
            other => Self::Other(other.to_string()),
        }
    }

    /// Whether this wall is one AreWeAntiCheatYet collects. Their data is
    /// anti-cheat behaviour; a missing Wine export is a Wine bug, not theirs.
    pub fn is_anti_cheat(&self) -> bool {
        matches!(self, Self::KernelAntiCheat | Self::MissingRuntime)
    }
}

impl Serialize for WallKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WallKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::parse(&String::deserialize(d)?))
    }
}

/// One run's evidence for a wall: the facts only a trace has.
///
/// Every field past `source` is optional. A source that cannot supply one
/// contributes `None`, and `None` never clears what an earlier observation
/// established - the same additive rule the identity-miss stash uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// Which tool observed this (`beisl`). Named so a later source is
    /// distinguishable without a schema change.
    pub source: String,
    /// `YYYY-MM-DD` the observation was last made.
    pub observed: String,
    /// Exact wine flavor and version the run used - the single most
    /// load-bearing fact in a ProtonDB report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wine: Option<String>,
    /// The GPU device name. beisl does NOT supply this - its artifacts carry
    /// a vendor tag only (see `gpu_vendor`) - so this is filled from this
    /// side or left empty rather than guessed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
    /// GPU vendor tag as beisl detected it: `intel`, `amd`, `nvidia`,
    /// `qualcomm`. A vendor, never a device - kept separate from `gpu` so a
    /// report generator cannot print "intel" where a device name belongs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_vendor: Option<String>,
    /// System specs, as the source rendered them. Nothing supplies this yet:
    /// beisl does not record specs at trace time (confirmed with that side,
    /// 2026-09-05), so it stays empty until something does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specs: Option<String>,
    /// What identifies this wall: the missing export, the `.sys` names, the
    /// failing path. Also the dedup key - see [`CompatStash::record`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Where the full evidence lives (a beisl run id or log path). A
    /// pointer, never a copy: this stash does not mirror trace data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    /// The run directory this observation came from, so machine specs can be
    /// read at SUBMIT time rather than copied here at scan time.
    ///
    /// Deliberately a pointer. A CPU/GPU/RAM/kernel tuple is a decent machine
    /// fingerprint, and the user's consent to publish one belongs at the
    /// submit step in front of them, not pre-collected into a stash months
    /// earlier. See `setup::specs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_dir: Option<String>,
    /// Layer split of who ate the frame (`kernel`, `driver`, `translation`,
    /// `wine`, `game`, `other`), when the source measured one.
    ///
    /// Read it with `attributed_pct` and `record_mode` or it misleads: the
    /// percentages are over ATTRIBUTED samples only, and a `degraded`
    /// recording is userspace-only, so a missing `kernel` layer there is a
    /// limit of the recording, not a quiet kernel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_split: Option<BTreeMap<String, f64>>,
    /// What fraction of the trace `layer_split` actually covers. The
    /// unattributed remainder is a measurement gap and is deliberately not
    /// folded into any named layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributed_pct: Option<f64>,
    /// How the trace was recorded (`degraded` means userspace-only). Carried
    /// so nothing downstream reads a layer split without its caveat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_mode: Option<String>,
    /// Anything a newer source sent that this build has no field for.
    /// Round-tripped untouched so an older gamebus-setup cannot silently
    /// drop a newer beisl's facts.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One game's wall, and every run that evidenced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatFinding {
    /// Display title when anything resolved one. The identity-miss stash is
    /// the better source; this is a convenience copy for a standalone read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub wall: WallKind,
    /// Steam appid, which is also gamedb's canonical id (`steam-<appid>`)
    /// and the only key ProtonDB needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_appid: Option<String>,
    /// AreWeAntiCheatYet slug (`apex-legends`). A key, never a URL: URLs
    /// derive from keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awacy_slug: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
    /// Newest last, capped at [`MAX_OBSERVATIONS`].
    #[serde(default)]
    pub observations: Vec<Observation>,
    /// Target name (`protondb`, `awacy`, `gamedb`) to the date it was
    /// submitted. A map rather than a bool per target so a new target needs
    /// no schema change - the `reported` flag gamedb carries reads off this.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reported: BTreeMap<String, String>,
    /// Set (to the date) when the user dismissed this finding: not wrong,
    /// just not wanted. A flag rather than a deletion, because the next run
    /// would re-record it anyway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed: Option<String>,
    /// Forward-compatibility catch-all, as on [`Observation`].
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl CompatFinding {
    /// Whether this finding is worth offering for submission.
    pub fn submittable(&self) -> bool {
        self.dismissed.is_none() && !self.observations.is_empty()
    }

    /// Whether a given target still wants this finding.
    pub fn needs(&self, target: &str) -> bool {
        self.submittable() && !self.reported.contains_key(target)
    }

    /// The most recent observation, which carries the freshest wine/GPU pair.
    pub fn latest(&self) -> Option<&Observation> {
        self.observations.last()
    }
}

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

    /// Take a finding for `key`, creating or refreshing it. Does not write -
    /// batch, then [`save`](Self::save).
    ///
    /// Facts only ever ADD: a field this delivery leaves empty keeps whatever
    /// an earlier one established. The observation is deduped on
    /// `(source, signature, wine)` - the same wall on the same build is one
    /// piece of evidence seen twice, not two - and a repeat only refreshes
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
        let same = entry.observations.iter().position(|o| {
            o.source == obs.source && o.signature == obs.signature && o.wine == obs.wine
        });
        match same {
            Some(i) => {
                let existing = &mut entry.observations[i];
                existing.observed = obs.observed;
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

    /// Mark a finding submitted to `target`, dated today.
    pub fn mark_reported(&mut self, key: &str, target: &str) {
        let day = today();
        self.update(key, |f| {
            f.reported.insert(target.to_string(), day);
        });
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
        newer.observation = obs(
            "beisl",
            "lighthouse_driver.sys",
            Some("spritzwine-10.1"),
        );
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
        assert!(back.contains("shader_stalls"), "observation-level extra kept");
    }

    #[test]
    fn marking_reported_takes_a_finding_out_of_that_targets_queue() {
        let mut s = stash();
        s.record("steam:4809930", wardogs());
        assert!(s.findings["steam:4809930"].needs("awacy"));
        s.mark_reported("steam:4809930", "awacy");
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
}
