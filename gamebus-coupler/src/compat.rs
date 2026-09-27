//! The compat-finding record: a wall that stopped a game, and every run
//! that evidenced it. Persisted by the engine in `compat-findings.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// failing path. Also the engine's dedup key, compared through
    /// [`canonical_signature`].
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
    /// Newest last, capped by the engine so a reproducing wall cannot grow
    /// the file.
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

/// A signature with any per-launch nonce collapsed, for comparison only.
///
/// Some anti-cheats register their service under a name generated per launch:
/// WARDOGS' Elytra produced `elytra_tu7-ELBU27khJhcC` on one run and
/// `elytra_r7tC-LhUH4-egxUm` on the next (both real, 2026-09-05). Compared
/// literally, every launch of the same game is a fresh observation, and
/// twenty of those evict the evidence that actually differs.
///
/// The test is deliberately narrow: a tail after `_` counts as generated only
/// when it is at least eight characters and mixes lower, upper and digit. The
/// two risks are not symmetric. Missing a nonce costs one duplicate
/// observation; matching too eagerly silently fuses two different walls, and
/// the same Proton log carries `elytraldrfs_driver` and `elytraldrfs_shared`
/// side by side - a rule that cut at the first `_` would merge them.
pub fn canonical_signature(signature: &str) -> String {
    let mut out = String::with_capacity(signature.len());
    let mut segment = String::new();
    for ch in signature.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            segment.push(ch);
        } else {
            push_canonical(&mut out, &segment);
            segment.clear();
            out.push(ch);
        }
    }
    push_canonical(&mut out, &segment);
    out
}

fn push_canonical(out: &mut String, segment: &str) {
    match segment.rsplit_once('_') {
        Some((stem, tail)) if looks_generated(tail) => {
            out.push_str(stem);
            out.push_str("_*");
        }
        _ => out.push_str(segment),
    }
}

fn looks_generated(tail: &str) -> bool {
    tail.len() >= 8
        && tail.chars().any(|c| c.is_ascii_lowercase())
        && tail.chars().any(|c| c.is_ascii_uppercase())
        && tail.chars().any(|c| c.is_ascii_digit())
}

/// Every target a finding can be filed with. `reported` stays an open map, so
/// a new target needs no schema change: it is added here, and only here.
pub const TARGETS: &[&str] = &["protondb", "awacy", "gamedb"];

/// Targets that still want this finding.
///
/// The AreWeAntiCheatYet rule lives here rather than in each caller:
/// AWACY collects anti-cheat behaviour, and a Wine stub is a Wine bug, not
/// theirs to carry. Every front-end asks this rather than keeping a copy
/// (beisl's request, 2026-09-05).
pub fn pending_targets(f: &CompatFinding) -> Vec<&'static str> {
    TARGETS
        .iter()
        .copied()
        .filter(|t| f.needs(t))
        .filter(|t| *t != "awacy" || f.wall.is_anti_cheat())
        .collect()
}
