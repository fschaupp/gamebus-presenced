//! Review the compat findings beisl handed over, and mark what was submitted.
//!
//! beisl detects a wall and fires the trigger; this side houses the finding
//! (owner policy, 2026-09-05). Two intakes, and which one can work depends on
//! the wall.
//!
//! **Push**, the detector's own trigger, is a drop directory: beisl writes
//! one JSON file per finding into the inbox (`compat --inbox` prints it) and
//! is done - no gamebus binary on its detection path, nothing to wait for.
//! Every `compat` invocation drains it first, so a spooled finding shows up
//! in a list or an export without anyone running an intake command. See
//! [`crate::setup::inbox`] for the writer's half of the contract.
//!
//! `compat --record <file|->` takes the same payload synchronously, for a
//! sender that wants an exit code, and for pasting one by hand.
//!
//! Either way the payload names beisl's own evidence (`kernel-driver`), not
//! this side's vocabulary, and [`wall_for`] does the translating.
//!
//! **Pull**, `compat --scan`, reads beisl's structured output over MCP.
//!
//! Push is not the redundant one. Two of the four walls are transient by
//! construction: a Wine stub and a volume-GUID failure exist only on the
//! launch that broke, they vanish the moment the underlying bug is fixed, and
//! Proton overwrites its log every run (established across the beisl and
//! SpritzWine sides, 2026-09-05). A scan can only record what is still
//! observable when the user happens to run it, so for those walls the push
//! has to fire at detection time or the evidence is simply gone. The stash is
//! the durable copy: once a finding is in, it survives the run that produced
//! it.
//!
//! Neither intake is load-bearing yet, and the honest reason is worth
//! carrying here: as of 2026-09-05 beisl cannot detect the anti-cheat wall
//! this pipeline was built for. Its loader declines before calling any Wine
//! primitive, so no rule over Wine channels reaches it, and the Wine log it
//! would read exists only when someone has hand-added PROTON_LOG to a game's
//! Steam launch options. That is beisl's problem to solve, not this module's,
//! but a reader should not mistake a quiet stash for a quiet machine.
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
//!   "wine": "SpritzWine-Prater",
//!   "gpu_vendor": "amd",
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
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;
use serde_json::Value;

use serde_json::json;

use crate::compat::{CompatFinding, CompatStash, Incoming, Observation, WallKind};
use crate::endpoints::Endpoints;
use crate::naming::NamingDb;
use crate::setup::inbox;
use crate::setup::mcp::{Client, ServerSpec};
use crate::setup::protonfix;
use crate::setup::reports;
use crate::setup::specs;
use crate::umu_report::today;
use gamebus_coupler::{pending_targets, FindingChange, FindingVerb, TARGETS};

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
    trace_dir: Option<String>,
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
            // Through the same seam a scan uses: a sender names the evidence
            // it saw, this side names the wall (see `wall_for`).
            wall: wall_for(&self.wall),
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
                trace_dir: self.trace_dir,
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
        let delivery: Delivery =
            serde_json::from_value(item).map_err(|e| format!("delivery {}: {e}", i + 1))?;
        let key = delivery
            .key()
            .map_err(|e| format!("delivery {}: {e}", i + 1))?;
        staged.push((key, delivery.into_incoming()));
    }

    let mut keys = Vec::new();
    for (key, incoming) in staged {
        stash.record(&key, incoming);
        keys.push(key);
    }
    Ok(keys)
}

/// What one pass over the inbox did.
#[derive(Default)]
pub(crate) struct Drained {
    /// Keys taken into the stash.
    pub(crate) recorded: Vec<String>,
    /// Drops this side refused, and where each one was parked.
    pub(crate) rejected: Vec<(PathBuf, String)>,
    /// Findings were ingested but the stash did not reach disk, so the drops
    /// were left where they are. They are still the only copy.
    pub(crate) held: bool,
}

/// Take everything beisl has dropped into the stash.
///
/// The order is what makes this safe to interrupt: every drop is ingested
/// into the in-memory stash, the stash is written once, and only then are the
/// files removed. A crash anywhere before the write leaves the drops in place
/// and the next run re-ingests them, which is a no-op - `record` dedups an
/// observation on `(source, signature, wine)`. Removing a file first would
/// trade a duplicate for a lost finding.
///
/// Runs on every `compat` invocation, not just `--scan`: a spooled finding
/// should show up in a list or an export without the user knowing an intake
/// exists (beisl's request, 2026-09-05). Its notices go to stderr so
/// `--export` stdout stays pasteable.
fn drain(stash: &mut CompatStash) -> Drained {
    match inbox::dir() {
        Some(dir) => drain_dir(stash, &dir),
        None => Drained::default(),
    }
}

pub(crate) fn drain_dir(stash: &mut CompatStash, dir: &Path) -> Drained {
    let mut out = Drained::default();
    let mut taken = Vec::new();
    for path in inbox::pending(dir) {
        match inbox::read(&path).and_then(|raw| ingest(stash, &raw)) {
            Ok(keys) => {
                out.recorded.extend(keys);
                taken.push(path);
            }
            Err(why) => {
                let landed = inbox::reject(&path).unwrap_or(path);
                out.rejected.push((landed, why));
            }
        }
    }

    if !taken.is_empty() {
        stash.save();
        if stash.unsaved() {
            out.held = true;
            return out;
        }
        for path in &taken {
            inbox::remove(path);
        }
    }
    out
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
/// identity - that gap is the whole reason this integration exists. Three
/// ways to close it, strongest evidence first:
///
/// 1. The appid beisl read off `org.gamebus.Presence.v1` at trace time. The
///    authoritative answer, and it needs nothing from us.
/// 2. ProtonFixes' own announcement in the run's game log, which names the
///    title and appid together. This is Proton's resolution of the actual
///    launch, so it outranks inferring an identity from a filename.
/// 3. The naming database: executable to title to Steam appid. Weakest of
///    the three - it matches on a basename and falls back to the first entry
///    in an ambiguous bucket - but it covers launches ProtonFixes was not
///    involved in.
///
/// Rung 2 is not a nicety. The five analyzed WARDOGS runs carry no appid
/// (they predate beisl reading presence), and neither the launcher
/// executable nor the playtest appid is in detectable.json - so rung 3 fails
/// on them and rung 2 is the only thing that keys the first payload we
/// actually want to file.
///
/// None of the three is a guess. A run that satisfies none is reported and
/// skipped rather than filed under an invented key, which would poison a key
/// space the identity-miss stash shares.
fn resolve_key(
    event: &WallEvent,
    naming: Option<&NamingDb>,
    trace_dir: Option<&Path>,
) -> Result<(String, Option<String>), String> {
    if let Some(appid) = event.steam_appid.as_deref().filter(|a| !a.is_empty()) {
        let title = naming
            .and_then(|db| db.lookup_by_steam_appid(appid))
            .map(str::to_string)
            .or_else(|| event.game.clone());
        return Ok((format!("steam:{appid}"), title));
    }

    if let Some(id) = trace_dir.and_then(protonfix::read_from_dir) {
        return Ok((format!("steam:{}", id.steam_appid), Some(id.title)));
    }

    let exe = event
        .game
        .as_deref()
        .ok_or_else(|| "the run names no executable".to_string())?;
    let db = naming
        .ok_or_else(|| format!("no appid on the run and no naming database to resolve {exe}"))?;
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

    // One extra call, for traces_root only: the run directory is where the
    // machine specs live, and this side reads them itself at submit time
    // rather than asking beisl to relay a machine fingerprint (that side's
    // call, 2026-09-05, and the right one).
    let traces_root = client
        .call_json("list_traces", json!({}))
        .ok()
        .and_then(|v| {
            v.get("traces_root")
                .and_then(Value::as_str)
                .map(str::to_string)
        });

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
        let run_dir = traces_root
            .as_ref()
            .map(|root| PathBuf::from(root).join(&event.run_id));
        let (key, title) = match resolve_key(&event, naming.as_ref(), run_dir.as_deref()) {
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
                    trace_dir: run_dir.as_ref().map(|p| p.display().to_string()),
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

    // Answered before the stash is touched: beisl asks for this path so it
    // can spool a finding, and a stash it cannot read is no reason to refuse.
    if rest.iter().any(|a| a == "--inbox") {
        let Some(dir) = inbox::dir() else {
            eprintln!("Cannot resolve the inbox path (no HOME).");
            return ExitCode::FAILURE;
        };
        if let Err(e) = inbox::ensure(&dir) {
            eprintln!("Cannot create {}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
        println!("{}", dir.display());
        return ExitCode::SUCCESS;
    }

    let record = flag_value(rest, "--record");
    let reported = flag_value(rest, "--reported");
    let target = flag_value(rest, "--target");
    let scanning = rest.iter().any(|a| a == "--scan");
    let export = flag_value(rest, "--export");
    let as_json = rest.iter().any(|a| a == "--json");
    let dismiss = flag_value(rest, "--dismiss");
    let undismiss = flag_value(rest, "--undismiss");
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

    let drained = drain(&mut stash);
    for key in &drained.recorded {
        eprintln!("Took {key} from the inbox.");
    }
    for (path, why) in &drained.rejected {
        eprintln!("Refused {}: {why}", path.display());
        eprintln!("It was kept, not deleted; nothing else was written.");
    }
    if drained.held {
        eprintln!("Ingested findings did not reach the stash file.");
        eprintln!("The drops are still in the inbox; nothing was lost.");
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
                if as_json {
                    let out = json!({
                        "recorded": outcome.recorded,
                        "skipped": outcome.skipped,
                        "truncated": outcome.truncated,
                    });
                    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
                    return ExitCode::SUCCESS;
                }
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

    if let Some(target) = export {
        let key = flag_value(rest, "--key");
        return run_export(&stash, &target, key.as_deref(), as_json);
    }

    // Dismissing is the review flow's "not worth filing": the finding stays,
    // with its evidence, and stops being offered. The stash has always had
    // the field; only the CLI could not set it (beisl's request, it drives
    // this from its window).
    if let Some(key) = dismiss.as_ref().or(undismiss.as_ref()) {
        if !stash.findings().contains_key(key) {
            eprintln!("No finding under {key}.");
            return ExitCode::FAILURE;
        }
        let clearing = undismiss.is_some();
        let verb = if clearing {
            FindingVerb::Undismiss
        } else {
            FindingVerb::Dismiss
        };
        let prior = json!(stash.findings()[key]);
        if let Some(Ok(change)) = stash.apply(key, &verb) {
            if let Err(e) = record_cli(key, verb, change, prior) {
                eprintln!("{e} Nothing was written.");
                return ExitCode::FAILURE;
            }
        }
        stash.save();
        if clearing {
            println!("Restored {key} to the review list.");
        } else {
            println!("Dismissed {key}.");
        }
        return ExitCode::SUCCESS;
    }

    if let Some(key) = reported {
        let Some(target) = target else {
            eprintln!("--reported needs --target ({}).", TARGETS.join(", "));
            return ExitCode::FAILURE;
        };
        let prior = stash.findings().get(&key).map_or(Value::Null, |f| json!(f));
        match stash.mark_reported(&key, &target) {
            Some(Ok(())) => {
                let verb = FindingVerb::MarkReported {
                    target: target.clone(),
                };
                if let Err(e) = record_cli(&key, verb, FindingChange::Reported, prior) {
                    eprintln!("{e} Nothing was written.");
                    return ExitCode::FAILURE;
                }
            }
            Some(Err(_)) => {
                eprintln!(
                    "Unknown target {target} - one of {}. Nothing was written.",
                    TARGETS.join(", ")
                );
                return ExitCode::FAILURE;
            }
            None => {
                eprintln!("No finding under {key}.");
                return ExitCode::FAILURE;
            }
        }
        stash.save();
        println!("Marked {key} reported to {target}.");
        return ExitCode::SUCCESS;
    }

    if as_json {
        return list_json(&stash, &drained);
    }
    list(&stash);
    ExitCode::SUCCESS
}

/// Record a CLI edit in the ledger when auth is set up. The stash is only
/// written after this succeeds, so a refused edit never lands.
fn record_cli(
    key: &str,
    verb: FindingVerb,
    change: FindingChange,
    prior: Value,
) -> Result<(), super::auth::AppendError> {
    let action = super::auth::Action::EditFinding {
        key: key.to_string(),
        verb,
        change,
    };
    super::auth::record_local(super::auth::Origin::Cli, action, prior)
}

/// Draft a submission for one finding, or for every finding a target still
/// wants. Prints; never submits.
fn run_export(stash: &CompatStash, target: &str, key: Option<&str>, as_json: bool) -> ExitCode {
    let endpoints = Endpoints::load();
    let mut keys: Vec<&String> = match key {
        Some(k) => match stash.findings().get_key_value(k) {
            Some((k, _)) => vec![k],
            None => {
                eprintln!("No finding under {k}.");
                return ExitCode::FAILURE;
            }
        },
        None => stash
            .findings()
            .iter()
            .filter(|(k, _)| pending_targets(&stash.findings()[*k]).contains(&target))
            .map(|(k, _)| k)
            .collect(),
    };
    keys.sort();

    if keys.is_empty() {
        if as_json {
            println!("{}", json!({"reports": []}));
        } else {
            println!("Nothing to submit to {target}.");
        }
        return ExitCode::SUCCESS;
    }

    let mut drafts = Vec::new();

    for k in keys {
        let f = &stash.findings()[k];
        let report = match target {
            "awacy" => reports::awacy_issue(f, &endpoints),
            "protondb" => {
                // Specs are read HERE, at the moment of drafting a public
                // report, and not before: see setup::specs.
                let trace = f
                    .latest()
                    .and_then(|o| o.trace_dir.as_deref())
                    .map(Path::new)
                    .and_then(specs::read_from_dir);
                let probed;
                let specs = match trace {
                    Some(s) => {
                        probed = s;
                        Some(&probed)
                    }
                    None => {
                        probed = specs::read_system();
                        Some(&probed)
                    }
                };
                reports::protondb_report(f, specs, &endpoints)
            }
            other => {
                eprintln!("Unknown target {other}. Expected awacy or protondb.");
                return ExitCode::FAILURE;
            }
        };

        if as_json {
            drafts.push(json!({
                "key": k,
                "target": target,
                "url": report.url,
                "body": report.body,
                // Never empty for a draft worth filing by hand: these are the
                // fields no trace can supply (see reports::Report).
                "missing": report.missing,
            }));
            continue;
        }

        println!("=== {k} ===");
        println!("{}", report.body);
        println!("File it at: {}", report.url);
        if !report.missing.is_empty() {
            println!();
            println!("Before filing, this draft still needs:");
            for m in &report.missing {
                println!("  - {m}");
            }
        }
        println!();
    }

    if as_json {
        let out = json!({"reports": drafts});
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return ExitCode::SUCCESS;
    }
    println!("Nothing was submitted. Review, complete, and file it yourself.");
    ExitCode::SUCCESS
}

/// The stash as one JSON object.
///
/// An object rather than a bare array so `--json` has the same shape here, on
/// `--scan` and on `--export`, and so it can gain a field without breaking a
/// reader. `key` and `needs` are computed onto each finding; everything else
/// is the stored record, unknown fields included.
/// One finding as every JSON surface shows it (`compat --json`, the MCP
/// server): the stored record plus its `key` and `needs`. Shared so the two
/// cannot drift.
pub(crate) fn finding_json(key: &str, f: &CompatFinding) -> Value {
    let mut value = serde_json::to_value(f).unwrap_or_else(|_| json!({}));
    if let Some(map) = value.as_object_mut() {
        map.insert("key".into(), json!(key));
        map.insert("needs".into(), json!(pending_targets(f)));
        // The stored record omits an empty field to keep the file small. A
        // reader should not have to tell "absent" from "empty", so every
        // documented key is present here even when the stash left it out.
        for (field, empty) in [
            ("title", Value::Null),
            ("steam_appid", Value::Null),
            ("awacy_slug", Value::Null),
            ("dismissed", Value::Null),
            ("reported", json!({})),
        ] {
            map.entry(field).or_insert(empty);
        }
    }
    value
}

fn list_json(stash: &CompatStash, drained: &Drained) -> ExitCode {
    let mut keys: Vec<&String> = stash.findings().keys().collect();
    keys.sort();

    let findings: Vec<Value> = keys
        .into_iter()
        .map(|key| finding_json(key, &stash.findings()[key]))
        .collect();

    let refused: Vec<Value> = drained
        .rejected
        .iter()
        .map(|(path, why)| json!({"path": path.display().to_string(), "why": why}))
        .collect();

    let out = json!({
        "findings": findings,
        // What this very run took off the inbox, so a UI can say what
        // arrived without diffing against its own last view.
        "inbox": {"took": drained.recorded, "refused": refused},
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    ExitCode::SUCCESS
}

fn list(stash: &CompatStash) {
    if stash.findings().is_empty() {
        println!("No compat findings stashed.");
        println!("beisl drops them in: gamebus-setup compat --inbox");
        return;
    }

    let mut keys: Vec<&String> = stash.findings().keys().collect();
    keys.sort();
    for key in keys {
        let f = &stash.findings()[key];
        let title = f.title.as_deref().unwrap_or("(untitled)");
        let mark = if f.dismissed.is_some() { "-" } else { "*" };
        println!("{mark} {title}  [{key}]");
        println!(
            "    wall: {}  seen {} .. {}",
            f.wall.as_str(),
            f.first_seen,
            f.last_seen
        );
        if let Some(o) = f.latest() {
            let wine = o.wine.as_deref().unwrap_or("unknown wine");
            let gpu = o.gpu.as_deref().unwrap_or("unknown GPU");
            println!("    latest: {wine} on {gpu} (via {})", o.source);
            if let Some(sig) = &o.signature {
                println!("    signature: {sig}");
            }
            // Read late and never stored: see setup::specs. A game stopped
            // by a wall renders no frame, so MangoHud logs nothing and the
            // trace usually cannot answer - which is exactly when the
            // machine is asked instead, labelled so nobody reads a probe as
            // a statement about the run.
            match o
                .trace_dir
                .as_deref()
                .map(Path::new)
                .and_then(specs::read_from_dir)
            {
                Some(s) => println!("    specs (this run): {}", s.summary()),
                None => {
                    let s = specs::read_system();
                    if !s.summary().is_empty() {
                        println!("    specs (this machine now): {}", s.summary());
                    }
                }
            }
        }
        let pending = pending_targets(f);
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
        let (key, title) =
            resolve_key(&event("eldenring.exe", Some("1245620")), Some(&db), None).unwrap();
        assert_eq!(key, "steam:1245620");
        assert_eq!(title.as_deref(), Some("Elden Ring"));
    }

    #[test]
    fn an_old_run_without_an_appid_is_resolved_through_the_naming_database() {
        let db = NamingDb::parse(NAMING).unwrap();
        let (key, title) = resolve_key(&event("eldenring.exe", None), Some(&db), None).unwrap();
        assert_eq!(key, "steam:1245620", "exe -> title -> appid");
        assert_eq!(title.as_deref(), Some("Elden Ring"));
    }

    #[test]
    fn an_unresolvable_run_is_skipped_rather_than_given_an_invented_key() {
        let db = NamingDb::parse(NAMING).unwrap();
        // Known exe, but no Steam sku to key on.
        let e = resolve_key(&event("nosku.exe", None), Some(&db), None).unwrap_err();
        assert!(e.contains("no Steam appid"), "{e}");
        // Unknown exe entirely.
        let e = resolve_key(&event("mystery.exe", None), Some(&db), None).unwrap_err();
        assert!(e.contains("no naming database entry"), "{e}");
        // No database at all.
        let e = resolve_key(&event("eldenring.exe", None), None, None).unwrap_err();
        assert!(e.contains("no naming database"), "{e}");
    }

    #[test]
    fn an_empty_appid_is_treated_as_absent() {
        let db = NamingDb::parse(NAMING).unwrap();
        let (key, _) = resolve_key(&event("eldenring.exe", Some("")), Some(&db), None).unwrap();
        assert_eq!(key, "steam:1245620", "fell through to the naming database");
    }

    /// A run dir holding a ProtonFixes announcement and nothing else.
    fn wardogs_run_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pfix-key-{tag}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("game.log"),
            "noise\nUsing early stage global defaults for \"WARDOGS Playtest\" (4809930)\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn the_game_log_keys_a_run_the_naming_database_cannot() {
        let db = NamingDb::parse(NAMING).unwrap();
        let dir = wardogs_run_dir("wins");
        // The real case: launcher exe in no database, no appid on the event.
        let (key, title) = resolve_key(
            &event("WardogsLauncher-Shipping.exe", None),
            Some(&db),
            Some(&dir),
        )
        .expect("keyed off the game log");
        assert_eq!(key, "steam:4809930");
        assert_eq!(title.as_deref(), Some("WARDOGS Playtest"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_appid_from_the_bus_still_outranks_the_game_log() {
        let db = NamingDb::parse(NAMING).unwrap();
        let dir = wardogs_run_dir("outranked");
        let (key, _) =
            resolve_key(&event("whatever.exe", Some("111")), Some(&db), Some(&dir)).expect("keyed");
        assert_eq!(key, "steam:111", "presence beats a log line");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_dir_with_no_announcement_falls_through_to_the_database() {
        let db = NamingDb::parse(NAMING).unwrap();
        let dir = std::env::temp_dir().join(format!("pfix-empty-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("game.log"), "just noise\n").unwrap();
        let (key, _) = resolve_key(&event("eldenring.exe", None), Some(&db), Some(&dir)).unwrap();
        assert_eq!(key, "steam:1245620");
        let _ = std::fs::remove_dir_all(&dir);
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
        assert_eq!(
            o.extra.get("shader_stalls").and_then(|v| v.as_i64()),
            Some(17)
        );
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

    fn inbox_scratch(tag: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("gamebus-drain-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let inbox = root.join("compat-inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        (root, inbox)
    }

    fn drop_file(inbox: &Path, name: &str, body: &str) {
        // The writer's contract: publish by rename, never by open-and-write.
        let tmp = inbox.join(format!("{name}.tmp"));
        std::fs::write(&tmp, body).unwrap();
        std::fs::rename(&tmp, inbox.join(name)).unwrap();
    }

    #[test]
    fn a_spooled_finding_lands_in_the_stash_and_the_drop_is_taken() {
        let (root, inbox) = inbox_scratch("basic");
        drop_file(
            &inbox,
            "0001-run-42.json",
            r#"{"wall":"kernel-driver","steam_appid":"4809930","title":"WARDOGS Playtest",
                "signature":"lighthouse_driver.sys"}"#,
        );

        let mut stash = CompatStash::from_path(root.join("compat-findings.json"));
        let out = drain_dir(&mut stash, &inbox);

        assert_eq!(out.recorded, vec!["steam:4809930"]);
        assert!(!out.held);
        // beisl's own evidence name, translated on this side.
        assert_eq!(
            stash.findings()["steam:4809930"].wall,
            WallKind::KernelAntiCheat
        );
        assert!(inbox::pending(&inbox).is_empty(), "the drop was taken");
        assert!(
            root.join("compat-findings.json").exists(),
            "and it reached disk"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_partial_write_is_invisible_until_it_is_renamed() {
        let (root, inbox) = inbox_scratch("partial");
        std::fs::write(inbox.join("0001-run-42.json.tmp"), r#"{"wall":"wine-s"#).unwrap();

        let mut stash = CompatStash::from_path(root.join("compat-findings.json"));
        let out = drain_dir(&mut stash, &inbox);

        assert!(out.recorded.is_empty());
        assert!(
            out.rejected.is_empty(),
            "a half-written file is not a refusal"
        );
        assert!(
            inbox.join("0001-run-42.json.tmp").exists(),
            "and it is left alone"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn one_bad_drop_does_not_block_the_others() {
        let (root, inbox) = inbox_scratch("mixed");
        drop_file(&inbox, "0001-bad.json", "{ not json");
        drop_file(
            &inbox,
            "0002-good.json",
            r#"{"wall":"wine-stub","steam_appid":"1867240"}"#,
        );
        // Parsed, but nothing names the game: refused rather than invented.
        drop_file(&inbox, "0003-keyless.json", r#"{"wall":"wine-stub"}"#);

        let mut stash = CompatStash::from_path(root.join("compat-findings.json"));
        let out = drain_dir(&mut stash, &inbox);

        assert_eq!(out.recorded, vec!["steam:1867240"]);
        assert_eq!(out.rejected.len(), 2);
        // Refused evidence is parked, not deleted - the writer gets to see it.
        assert!(inbox.join("0001-bad.json.rejected").exists());
        assert!(inbox.join("0003-keyless.json.rejected").exists());
        assert!(inbox::pending(&inbox).is_empty(), "and neither is retried");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_stash_that_cannot_be_written_keeps_the_drops() {
        let (root, inbox) = inbox_scratch("held");
        drop_file(
            &inbox,
            "0001-run-42.json",
            r#"{"wall":"kernel-driver","steam_appid":"4809930"}"#,
        );

        // A corrupt stash refuses to be overwritten, which is exactly the
        // case where deleting the drop would destroy the only other copy.
        let path = root.join("compat-findings.json");
        std::fs::write(&path, "{ not json").unwrap();
        let mut stash = CompatStash::from_path(path);
        let out = drain_dir(&mut stash, &inbox);

        assert!(out.held);
        assert_eq!(
            inbox::pending(&inbox).len(),
            1,
            "the finding is still spooled"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn draining_the_same_drop_twice_adds_one_observation() {
        let (root, inbox) = inbox_scratch("idempotent");
        let body = r#"{"wall":"kernel-driver","steam_appid":"4809930",
                       "signature":"lighthouse_driver.sys","wine":"SpritzWine-Prater"}"#;
        let mut stash = CompatStash::from_path(root.join("compat-findings.json"));

        drop_file(&inbox, "0001-run-42.json", body);
        drain_dir(&mut stash, &inbox);
        // What a crash between the save and the unlink would leave behind.
        drop_file(&inbox, "0001-run-42.json", body);
        drain_dir(&mut stash, &inbox);

        assert_eq!(stash.findings()["steam:4809930"].observations.len(), 1);

        std::fs::remove_dir_all(&root).unwrap();
    }
}
