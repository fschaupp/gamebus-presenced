//! Review the compat findings beisl handed over, and mark what was submitted.
//!
//! beisl detects a wall and fires the trigger; this side houses the finding
//! (owner policy, 2026-09-05). The intake is a push: beisl runs
//!
//! ```text
//! gamebus-setup compat --record -   < finding.json
//! ```
//!
//! which keeps gamebus free of any knowledge of beisl's on-disk layout, and
//! keeps this tool the stash's only writer. A `--scan` backfill from beisl's
//! existing artifacts is the other half of the agreed intake and is not
//! built yet: it needs beisl's artifact layout pinned first, which is being
//! settled in that repo.
//!
//! The delivery is one JSON object, or an array of them:
//!
//! ```json
//! {
//!   "key": "steam:4809930",
//!   "wall": "kernel-anti-cheat",
//!   "title": "WARDOGS Playtest",
//!   "steam_appid": "4809930",
//!   "source": "beisl",
//!   "wine": "spritzwine-10.0",
//!   "gpu": "AMD Radeon RX 7900 XT",
//!   "signature": "lighthouse_driver.sys, elytraldrfs_driver.sys",
//!   "log": "beisl:run-42"
//! }
//! ```
//!
//! `key` may be omitted when `steam_appid` is present - `steam:<appid>` is
//! the identity-miss stash's own key for a Steam launch, and the two stashes
//! join on it. Every field this build does not know is kept verbatim on the
//! observation rather than dropped, so a newer beisl can send more than this
//! version understands without losing it.

use std::collections::BTreeMap;
use std::process::ExitCode;

use serde::Deserialize;
use serde_json::Value;

use serde_json::json;

use crate::compat::{CompatStash, Incoming, Observation, WallKind};
use crate::naming::NamingDb;
use crate::setup::mcp::{Client, ServerSpec};
use crate::umu_report::today;

/// One finding as a source delivers it. Flat on purpose: the run facts and
/// the game keys arrive together, and a sender should not have to model this
/// side's split between a finding and its observations.
#[derive(Debug, Deserialize)]
struct Delivery {
    /// The stash key. Derived from `steam_appid` when absent.
    #[serde(default)]
    key: Option<String>,
    wall: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    steam_appid: Option<String>,
    #[serde(default)]
    awacy_slug: Option<String>,
    /// Which tool observed this. Defaults to `beisl`, the only sender today.
    #[serde(default = "default_source")]
    source: String,
    #[serde(default)]
    wine: Option<String>,
    #[serde(default)]
    gpu: Option<String>,
    #[serde(default)]
    gpu_vendor: Option<String>,
    #[serde(default)]
    specs: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    log: Option<String>,
    #[serde(default)]
    layer_split: Option<BTreeMap<String, f64>>,
    #[serde(default)]
    attributed_pct: Option<f64>,
    #[serde(default)]
    record_mode: Option<String>,
    /// Everything this build has no field for, kept on the observation.
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

fn default_source() -> String {
    "beisl".to_string()
}

impl Delivery {
    /// The stash key this finding belongs under, or why it has none.
    fn key(&self) -> Result<String, String> {
        if let Some(k) = self.key.as_deref().filter(|k| !k.is_empty()) {
            return Ok(k.to_string());
        }
        match self.steam_appid.as_deref().filter(|a| !a.is_empty()) {
            Some(appid) => Ok(format!("steam:{appid}")),
            None => Err("delivery has neither a key nor a steam_appid".into()),
        }
    }

    fn into_incoming(self) -> Incoming {
        Incoming {
            wall: WallKind::parse(&self.wall),
            title: self.title,
            steam_appid: self.steam_appid,
            awacy_slug: self.awacy_slug,
            observation: Observation {
                source: self.source,
                observed: today(),
                wine: self.wine,
                gpu: self.gpu,
                gpu_vendor: self.gpu_vendor,
                specs: self.specs,
                signature: self.signature,
                log: self.log,
                layer_split: self.layer_split,
                attributed_pct: self.attributed_pct,
                record_mode: self.record_mode,
                extra: self.extra,
            },
        }
    }
}

/// Accept one or many deliveries from `raw`.
///
/// Returns the recorded keys, or the first reason the payload was refused.
/// Nothing is written unless every delivery parsed: a half-applied batch is
/// harder to reason about than a rejected one.
fn ingest(stash: &mut CompatStash, raw: &str) -> Result<Vec<String>, String> {
    let value: Value = serde_json::from_str(raw).map_err(|e| format!("not JSON: {e}"))?;
    let items = match value {
        Value::Array(items) => items,
        object => vec![object],
    };

    let mut staged = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        let delivery: Delivery = serde_json::from_value(item)
            .map_err(|e| format!("delivery {}: {e}", i + 1))?;
        let key = delivery.key().map_err(|e| format!("delivery {}: {e}", i + 1))?;
        staged.push((key, delivery.into_incoming()));
    }

    let mut keys = Vec::new();
    for (key, incoming) in staged {
        stash.record(&key, incoming);
        keys.push(key);
    }
    Ok(keys)
}

/// Translate beisl's signal name into this side's finding name.
///
/// The two vocabularies are deliberately different and this is the seam where
/// the difference lives. beisl names the EVIDENCE it saw: `kernel-driver` is
/// "a .sys that Wine does not ship was loaded", which is almost always an
/// anti-cheat but is not beisl's call to make. Naming it `kernel-anti-cheat`
/// is an interpretation, so it happens here, on the side that submits reports
/// and has to stand behind them (agreed with that side, 2026-09-05).
///
/// An unrecognised signal keeps its own name rather than being dropped: a
/// newer beisl may detect a wall this build has never heard of.
fn wall_for(signal: &str) -> WallKind {
    match signal {
        "kernel-driver" => WallKind::KernelAntiCheat,
        "wine-stub" => WallKind::WineStub,
        "volume-guid" => WallKind::VolumeGuidPath,
        other => WallKind::parse(other),
    }
}

/// One compat-wall event, as `list_events` renders it.
#[derive(Debug, Deserialize)]
struct WallEvent {
    run_id: String,
    #[serde(default)]
    game: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    signatures: Vec<String>,
    #[serde(default)]
    detail: Option<String>,
    /// Present since beisl learned to read presence at trace time. Null on
    /// every run traced before that, which is most of them, so this is
    /// resolved the long way when it is missing (see [`resolve_key`]).
    #[serde(default)]
    steam_appid: Option<String>,
}

/// The artifact fields worth carrying onto an observation. Every one is
/// optional: an unanalyzed run has no artifact at all, and a finding without
/// one is still a finding.
#[derive(Debug, Default, Deserialize)]
struct Artifact {
    #[serde(default)]
    wine_flavor: Option<String>,
    #[serde(default)]
    gpu_vendor: Option<String>,
    #[serde(default)]
    record_mode: Option<String>,
    #[serde(default)]
    attributed_pct: Option<f64>,
    #[serde(default)]
    layers: Vec<Layer>,
}

#[derive(Debug, Deserialize)]
struct Layer {
    layer: String,
    pct: f64,
}

/// Work out which game a wall belongs to.
///
/// beisl names a run after its executable, and an executable is not an
/// identity - that gap is the whole reason this integration exists. Two ways
/// to close it, best first:
///
/// 1. The appid beisl read off `org.gamebus.Presence.v1` at trace time. This
///    is the authoritative answer and needs nothing from us.
/// 2. Failing that, the naming database: executable to title to Steam appid.
///    This is how a run traced before beisl learned to read presence still
///    lands under the right key, and it is exactly the identity work this
///    side owns.
///
/// Neither is a guess. A run that satisfies neither is reported and skipped
/// rather than filed under an invented key, which would poison a key space
/// the identity-miss stash shares.
fn resolve_key(event: &WallEvent, naming: Option<&NamingDb>) -> Result<(String, Option<String>), String> {
    if let Some(appid) = event.steam_appid.as_deref().filter(|a| !a.is_empty()) {
        let title = naming
            .and_then(|db| db.lookup_by_steam_appid(appid))
            .map(str::to_string)
            .or_else(|| event.game.clone());
        return Ok((format!("steam:{appid}"), title));
    }

    let exe = event
        .game
        .as_deref()
        .ok_or_else(|| "the run names no executable".to_string())?;
    let db = naming.ok_or_else(|| {
        format!("no appid on the run and no naming database to resolve {exe}")
    })?;
    let title = db
        .lookup_by_executable(exe)
        .ok_or_else(|| format!("{exe} is in no naming database entry"))?
        .to_string();
    let appid = db
        .steam_appid_for_title(&title)
        .ok_or_else(|| format!("{title} has no Steam appid to key on"))?;
    Ok((format!("steam:{appid}"), Some(title)))
}

/// Pull findings from a server's structured output.
///
/// Idempotent by construction: an observation dedups on
/// `(source, signature, wine)`, so rescanning the same traces refreshes dates
/// rather than piling up evidence. That is why there is no cursor to keep -
/// a full scan is always safe, and `--since` is an optimisation, not a
/// correctness requirement.
fn scan(stash: &mut CompatStash, spec: ServerSpec, since: u64) -> Result<Scan, String> {
    let mut client = Client::start(spec)?;
    let source = client.name().to_string();

    let tools = client.tool_names()?;
    if !tools.iter().any(|t| t == "list_events") {
        return Err(format!(
            "{source} offers no list_events tool; nothing to scan"
        ));
    }

    let events = client.call_json("list_events", json!({ "since_unix": since }))?;
    let truncated = events
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let list = events
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let naming = NamingDb::load();
    let mut outcome = Scan {
        truncated,
        ..Scan::default()
    };

    for raw in list {
        if raw.get("kind").and_then(Value::as_str) != Some("compat-wall") {
            continue;
        }
        let event: WallEvent = match serde_json::from_value(raw) {
            Ok(e) => e,
            Err(e) => {
                outcome.skipped.push(format!("unreadable event: {e}"));
                continue;
            }
        };
        let (key, title) = match resolve_key(&event, naming.as_ref()) {
            Ok(pair) => pair,
            Err(why) => {
                outcome.skipped.push(format!("{}: {why}", event.run_id));
                continue;
            }
        };

        // Best effort: an unanalyzed run has no artifact, and the wall is
        // worth recording without one.
        let artifact: Artifact = client
            .call_json("get_artifact", json!({ "run_id": event.run_id }))
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();

        let layer_split: BTreeMap<String, f64> = artifact
            .layers
            .iter()
            .map(|l| (l.layer.clone(), l.pct))
            .collect();

        let signal = event
            .signature
            .clone()
            .or_else(|| event.signatures.first().cloned())
            .unwrap_or_default();

        stash.record(
            &key,
            Incoming {
                wall: wall_for(&signal),
                title,
                steam_appid: key.strip_prefix("steam:").map(str::to_string),
                awacy_slug: None,
                observation: Observation {
                    source: source.clone(),
                    observed: today(),
                    wine: artifact.wine_flavor,
                    gpu: None,
                    gpu_vendor: artifact.gpu_vendor.filter(|v| !v.is_empty()),
                    specs: None,
                    // The detail line is the evidence a human reads; the
                    // signal name alone would not survive review.
                    signature: event.detail.or(Some(signal)),
                    log: Some(format!("{source}:{}", event.run_id)),
                    layer_split: (!layer_split.is_empty()).then_some(layer_split),
                    attributed_pct: artifact.attributed_pct,
                    record_mode: artifact.record_mode,
                    extra: BTreeMap::new(),
                },
            },
        );
        outcome.recorded.push(key);
    }

    outcome.recorded.sort();
    outcome.recorded.dedup();
    Ok(outcome)
}

#[derive(Debug, Default)]
struct Scan {
    recorded: Vec<String>,
    skipped: Vec<String>,
    truncated: bool,
}

pub fn run(args: &[String]) -> ExitCode {
    let rest = &args[1..];
    let record = flag_value(rest, "--record");
    let reported = flag_value(rest, "--reported");
    let target = flag_value(rest, "--target");
    let scanning = rest.iter().any(|a| a == "--scan");
    let since = flag_value(rest, "--since")
        .map(|v| v.parse::<u64>())
        .transpose();
    let since = match since {
        Ok(v) => v.unwrap_or(0),
        Err(e) => {
            eprintln!("--since wants a unix timestamp: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut stash = CompatStash::load();
    if let Some(e) = stash.load_error() {
        // Never show "no findings" over a file that failed to parse - that
        // reads as data loss. Nothing writes to it either.
        eprintln!("{e}");
        eprintln!("Fix or move the file; nothing has overwritten it.");
        return ExitCode::FAILURE;
    }
    if stash.path().is_none() {
        eprintln!("Cannot resolve the stash path (no HOME).");
        return ExitCode::FAILURE;
    }

    if let Some(source) = record {
        let raw = match read_payload(&source) {
            Ok(raw) => raw,
            Err(e) => {
                eprintln!("Cannot read {source}: {e}");
                return ExitCode::FAILURE;
            }
        };
        return match ingest(&mut stash, &raw) {
            Ok(keys) => {
                stash.save();
                for key in &keys {
                    println!("Recorded {key}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("Refused the payload: {e}");
                eprintln!("Nothing was written.");
                ExitCode::FAILURE
            }
        };
    }

    if scanning {
        return match scan(&mut stash, ServerSpec::beisl(), since) {
            Ok(outcome) => {
                stash.save();
                for key in &outcome.recorded {
                    println!("Recorded {key}");
                }
                for why in &outcome.skipped {
                    // Loud on purpose: a wall we could not key is a finding
                    // the user has and does not know about.
                    eprintln!("Skipped {why}");
                }
                if outcome.truncated {
                    eprintln!("The event list was truncated; rerun with --since to page.");
                }
                if outcome.recorded.is_empty() && outcome.skipped.is_empty() {
                    println!("No compat walls in beisl's traces.");
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("Scan failed: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if let Some(key) = reported {
        let Some(target) = target else {
            eprintln!("--reported needs --target (protondb, awacy, gamedb).");
            return ExitCode::FAILURE;
        };
        if !stash.findings().contains_key(&key) {
            eprintln!("No finding under {key}.");
            return ExitCode::FAILURE;
        }
        stash.mark_reported(&key, &target);
        stash.save();
        println!("Marked {key} reported to {target}.");
        return ExitCode::SUCCESS;
    }

    list(&stash);
    ExitCode::SUCCESS
}

fn list(stash: &CompatStash) {
    if stash.findings().is_empty() {
        println!("No compat findings stashed.");
        println!("beisl records them with: gamebus-setup compat --record -");
        return;
    }

    let mut keys: Vec<&String> = stash.findings().keys().collect();
    keys.sort();
    for key in keys {
        let f = &stash.findings()[key];
        let title = f.title.as_deref().unwrap_or("(untitled)");
        let mark = if f.dismissed.is_some() { "-" } else { "*" };
        println!("{mark} {title}  [{key}]");
        println!("    wall: {}  seen {} .. {}", f.wall.as_str(), f.first_seen, f.last_seen);
        if let Some(o) = f.latest() {
            let wine = o.wine.as_deref().unwrap_or("unknown wine");
            let gpu = o.gpu.as_deref().unwrap_or("unknown GPU");
            println!("    latest: {wine} on {gpu} (via {})", o.source);
            if let Some(sig) = &o.signature {
                println!("    signature: {sig}");
            }
        }
        let pending: Vec<&str> = ["protondb", "awacy", "gamedb"]
            .into_iter()
            .filter(|t| f.needs(t))
            // AreWeAntiCheatYet collects anti-cheat behaviour; a Wine stub is
            // a Wine bug and not theirs to carry.
            .filter(|t| *t != "awacy" || f.wall.is_anti_cheat())
            .collect();
        if !pending.is_empty() {
            println!("    not yet reported to: {}", pending.join(", "));
        }
        if !f.reported.is_empty() {
            let done: Vec<String> = f
                .reported
                .iter()
                .map(|(t, when)| format!("{t} ({when})"))
                .collect();
            println!("    reported: {}", done.join(", "));
        }
    }
    println!();
    println!("{} finding(s).", stash.findings().len());
}

/// `-` means stdin; anything else is a path.
fn read_payload(source: &str) -> std::io::Result<String> {
    if source == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        Ok(buf)
    } else {
        std::fs::read_to_string(source)
    }
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .filter(|v| !v.starts_with("--"))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMING: &str = r#"[
        {
            "name": "Elden Ring",
            "executables": [{"name": "eldenring.exe", "os": "win32", "is_launcher": false}],
            "third_party_skus": [{"distributor": "steam", "id": "1245620"}]
        },
        {
            "name": "No Sku Game",
            "executables": [{"name": "nosku.exe", "os": "win32", "is_launcher": false}],
            "third_party_skus": []
        }
    ]"#;

    fn event(game: &str, appid: Option<&str>) -> WallEvent {
        WallEvent {
            run_id: "run-1".into(),
            game: Some(game.into()),
            signature: Some("kernel-driver".into()),
            signatures: vec!["kernel-driver".into()],
            detail: Some("ZwLoadDriver lighthouse_driver.sys".into()),
            steam_appid: appid.map(str::to_string),
        }
    }

    #[test]
    fn beisls_evidence_names_become_this_sides_finding_names() {
        assert_eq!(wall_for("kernel-driver"), WallKind::KernelAntiCheat);
        assert_eq!(wall_for("wine-stub"), WallKind::WineStub);
        assert_eq!(wall_for("volume-guid"), WallKind::VolumeGuidPath);
    }

    #[test]
    fn a_signal_this_build_does_not_know_keeps_its_own_name() {
        assert_eq!(
            wall_for("gpu-fault-loop"),
            WallKind::Other("gpu-fault-loop".into())
        );
    }

    #[test]
    fn the_appid_beisl_read_off_the_bus_wins() {
        let db = NamingDb::parse(NAMING).unwrap();
        let (key, title) = resolve_key(&event("eldenring.exe", Some("1245620")), Some(&db)).unwrap();
        assert_eq!(key, "steam:1245620");
        assert_eq!(title.as_deref(), Some("Elden Ring"));
    }

    #[test]
    fn an_old_run_without_an_appid_is_resolved_through_the_naming_database() {
        let db = NamingDb::parse(NAMING).unwrap();
        let (key, title) = resolve_key(&event("eldenring.exe", None), Some(&db)).unwrap();
        assert_eq!(key, "steam:1245620", "exe -> title -> appid");
        assert_eq!(title.as_deref(), Some("Elden Ring"));
    }

    #[test]
    fn an_unresolvable_run_is_skipped_rather_than_given_an_invented_key() {
        let db = NamingDb::parse(NAMING).unwrap();
        // Known exe, but no Steam sku to key on.
        let e = resolve_key(&event("nosku.exe", None), Some(&db)).unwrap_err();
        assert!(e.contains("no Steam appid"), "{e}");
        // Unknown exe entirely.
        let e = resolve_key(&event("mystery.exe", None), Some(&db)).unwrap_err();
        assert!(e.contains("no naming database entry"), "{e}");
        // No database at all.
        let e = resolve_key(&event("eldenring.exe", None), None).unwrap_err();
        assert!(e.contains("no naming database"), "{e}");
    }

    #[test]
    fn an_empty_appid_is_treated_as_absent() {
        let db = NamingDb::parse(NAMING).unwrap();
        let (key, _) = resolve_key(&event("eldenring.exe", Some("")), Some(&db)).unwrap();
        assert_eq!(key, "steam:1245620", "fell through to the naming database");
    }

    #[test]
    fn a_steam_delivery_needs_no_explicit_key() {
        let mut s = CompatStash::default();
        let keys = ingest(
            &mut s,
            r#"{"wall":"kernel-anti-cheat","steam_appid":"4809930","source":"beisl"}"#,
        )
        .expect("accepted");
        assert_eq!(keys, vec!["steam:4809930"]);
        assert!(s.findings().contains_key("steam:4809930"));
    }

    #[test]
    fn a_delivery_with_no_identity_is_refused() {
        let mut s = CompatStash::default();
        let e = ingest(&mut s, r#"{"wall":"wine-stub"}"#).unwrap_err();
        assert!(e.contains("neither a key nor a steam_appid"), "{e}");
        assert!(s.findings().is_empty());
    }

    #[test]
    fn a_batch_is_all_or_nothing() {
        let mut s = CompatStash::default();
        let e = ingest(
            &mut s,
            r#"[{"wall":"wine-stub","key":"steam:1"},{"wall":"wine-stub"}]"#,
        )
        .unwrap_err();
        assert!(e.contains("delivery 2"), "{e}");
        assert!(
            s.findings().is_empty(),
            "the good first delivery is not half-applied"
        );
    }

    #[test]
    fn unknown_delivery_fields_land_on_the_observation() {
        let mut s = CompatStash::default();
        ingest(
            &mut s,
            r#"{"wall":"wine-stub","key":"steam:1","shader_stalls":17}"#,
        )
        .expect("accepted");
        let o = s.findings()["steam:1"].latest().expect("an observation");
        assert_eq!(o.extra.get("shader_stalls").and_then(|v| v.as_i64()), Some(17));
    }

    #[test]
    fn an_array_and_a_bare_object_are_both_accepted() {
        let mut s = CompatStash::default();
        let one = ingest(&mut s, r#"{"wall":"wine-stub","key":"a"}"#).unwrap();
        let many = ingest(
            &mut s,
            r#"[{"wall":"wine-stub","key":"b"},{"wall":"wine-stub","key":"c"}]"#,
        )
        .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(many.len(), 2);
        assert_eq!(s.findings().len(), 3);
    }

    #[test]
    fn the_source_defaults_to_beisl() {
        let mut s = CompatStash::default();
        ingest(&mut s, r#"{"wall":"wine-stub","key":"steam:1"}"#).unwrap();
        assert_eq!(s.findings()["steam:1"].latest().unwrap().source, "beisl");
    }
}
