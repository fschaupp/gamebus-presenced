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

use crate::compat::{CompatStash, Incoming, Observation, WallKind};
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
    specs: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    log: Option<String>,
    #[serde(default)]
    layer_split: Option<BTreeMap<String, f64>>,
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
                specs: self.specs,
                signature: self.signature,
                log: self.log,
                layer_split: self.layer_split,
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

pub fn run(args: &[String]) -> ExitCode {
    let rest = &args[1..];
    let record = flag_value(rest, "--record");
    let reported = flag_value(rest, "--reported");
    let target = flag_value(rest, "--target");

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
