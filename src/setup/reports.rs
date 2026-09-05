//! Submission drafts for the compat findings: ProtonDB and AreWeAntiCheatYet.
//!
//! Both targets are drafted here and submitted by the user, never by this
//! tool. Neither has an unauthenticated submission API, and neither should be
//! written to by a program on a user's behalf: a compatibility report carries
//! their machine's specs and their name on a public claim.
//!
//! # What was verified, and when
//!
//! AreWeAntiCheatYet takes new games and status changes as **GitHub issues
//! with a form template**, not pull requests against `games.json` (their
//! README and `.github/ISSUE_TEMPLATE/0-new-game.yml`, read 2026-09-05). An
//! earlier design note here assumed a pull request; it was wrong, and this
//! module follows the template. Their issue forms accept prefilled values as
//! query parameters keyed by field id, so the draft is handed over as a URL
//! that opens the form already filled, plus the same content as text.
//!
//! ProtonDB reports are filed on the game's own page behind a Steam login, so
//! the draft is a report body to paste.
//!
//! # What is deliberately left blank
//!
//! The AreWeAntiCheatYet form REQUIRES a "Proof of mention" - a reputable
//! public source for the claim. A local trace is not one, and inventing a
//! citation would be worse than leaving it empty. Anything required that this
//! side cannot honestly supply is reported in [`Report::missing`] so the user
//! is told what to add rather than discovering it in the form.

#![allow(dead_code)]

use crate::compat::{CompatFinding, WallKind};
use crate::endpoints::Endpoints;
use crate::setup::specs::{SpecSource, Specs};

/// A drafted submission: what to say, where to say it, and what is still
/// missing.
#[derive(Debug, Clone)]
pub struct Report {
    /// Where the user goes to file it. For AreWeAntiCheatYet this opens the
    /// issue form with the known fields already filled.
    pub url: String,
    /// The same content as plain text, for review before filing and for the
    /// targets where pasting is the only route.
    pub body: String,
    /// Required fields this side could not honestly fill.
    pub missing: Vec<String>,
}

/// The anti-cheat dropdown's own vocabulary, matched from the driver names a
/// trace saw.
///
/// Conservative on purpose: only anti-cheats whose driver filenames are
/// unmistakable are named. Anything else is "Other" with the observed
/// filenames quoted as evidence and the naming left to the user - a trace
/// proves a kernel driver loaded, not whose product it is.
fn anticheat_for(signature: &str) -> (&'static str, Option<String>) {
    let lower = signature.to_ascii_lowercase();
    if lower.contains("easyanticheat") || lower.contains("eac_") {
        return ("Easy Anti-Cheat (EAC)", None);
    }
    if lower.contains("bedaisy") || lower.contains("battleye") {
        return ("BattlEye", None);
    }
    ("Other (please specify below)", None)
}

/// The status vocabulary, from the form's dropdown.
///
/// A wall means the game does not run, which is their "Broken". Never
/// "Denied": that records a vendor REFUSING to enable Linux support, which is
/// a statement about the vendor, and no trace can establish it.
fn status_for(wall: &WallKind) -> &'static str {
    match wall {
        WallKind::KernelAntiCheat | WallKind::MissingRuntime => "Broken",
        _ => "Broken",
    }
}

/// Draft an AreWeAntiCheatYet issue for a finding.
pub fn awacy_issue(f: &CompatFinding, e: &Endpoints) -> Report {
    let title = f.title.as_deref().unwrap_or("(unknown title)");
    let store_url = f
        .steam_appid
        .as_deref()
        .map(|id| format!("https://store.steampowered.com/app/{id}/"));
    let observed = f.latest();
    let signature = observed.and_then(|o| o.signature.as_deref()).unwrap_or("");
    let (anticheat, name) = anticheat_for(signature);
    let status = status_for(&f.wall);

    let mut missing = Vec::new();
    if store_url.is_none() {
        missing.push("Game main page (required): no Steam appid on this finding".into());
    }
    missing.push(
        "Proof of mention (required): a reputable public source. A local trace is not one; \
         link a vendor statement, a news item or a ProtonDB report."
            .into(),
    );
    if anticheat.starts_with("Other") {
        missing.push(
            "Anti-cheat name: the trace shows which driver loaded, not whose product it is. \
             Name it if you know."
                .into(),
        );
    }

    let mut tinkering = String::new();
    if let Some(o) = observed {
        if let Some(wine) = &o.wine {
            tinkering.push_str(&format!("Tried under {wine}.\n"));
        }
        if let Some(sig) = &o.signature {
            tinkering.push_str(&format!("Observed: {sig}\n"));
        }
        tinkering.push_str("No configuration made the game run.\n");
    }

    let comments = format!(
        "Detected by beisl and stashed by gamebus-setup on {}.\nWall kind: {}.\n\
         This draft was generated from a local trace; nothing was submitted automatically.",
        f.last_seen,
        f.wall.as_str()
    );

    let body = format!(
        "Game title: {title}\n\
         Game main page: {}\n\
         Anti-cheat software: {anticheat}\n\
         Anti-cheat name: {}\n\
         Status of the game anti-cheat: {status}\n\
         Tinkering steps:\n{tinkering}\
         Proof of mention: <REQUIRED - add a reputable source>\n\
         Additional comments:\n{comments}\n",
        store_url
            .as_deref()
            .unwrap_or("<REQUIRED - no Steam appid known>"),
        name.as_deref().unwrap_or(""),
    );

    let mut url = format!(
        "{}&title={}",
        e.awacy_new_game_issue,
        encode(&format!("Request: {title}"))
    );
    url.push_str(&format!("&game={}", encode(title)));
    if let Some(store) = &store_url {
        url.push_str(&format!("&game-url={}", encode(store)));
    }
    url.push_str(&format!("&anti-cheat={}", encode(anticheat)));
    url.push_str(&format!("&anti-cheat-status={}", encode(status)));
    if !tinkering.is_empty() {
        url.push_str(&format!("&tinkering-steps={}", encode(tinkering.trim())));
    }
    url.push_str(&format!("&additional-comments={}", encode(&comments)));

    Report { url, body, missing }
}

/// Draft a ProtonDB report for a finding.
///
/// `specs` is whatever could be read at submit time. Its [`SpecSource`] is
/// printed, because "the GPU that ran this session" and "a GPU present on
/// this machine" are different claims and a public report should not blur
/// them.
pub fn protondb_report(f: &CompatFinding, specs: Option<&Specs>, e: &Endpoints) -> Report {
    let title = f.title.as_deref().unwrap_or("(unknown title)");
    let observed = f.latest();

    let mut missing = Vec::new();
    let url = match f.steam_appid.as_deref() {
        Some(id) => format!("{}/{id}", e.protondb_app),
        None => {
            missing.push("Steam appid (required): ProtonDB reports are per app id".into());
            e.protondb_app.clone()
        }
    };

    let mut body = format!("{title}\n\n");
    if let Some(o) = observed {
        body.push_str(&format!(
            "Does not run: {}.\n",
            o.signature.as_deref().unwrap_or(f.wall.as_str())
        ));
        if let Some(wine) = &o.wine {
            body.push_str(&format!("Proton/Wine: {wine}\n"));
        }
        if let Some(mode) = &o.record_mode {
            if mode == "degraded" {
                body.push_str(
                    "Recording was userspace-only, so no kernel-side timing is included.\n",
                );
            }
        }
    }

    match specs {
        Some(s) if s.source == SpecSource::Trace => {
            body.push_str(&format!("\nSystem (this run): {}\n", s.summary()));
        }
        Some(s) => {
            body.push_str(&format!("\nSystem (this machine): {}\n", s.summary()));
            body.push_str(
                "The game rendered no frame, so no per-session hardware record exists; \
                 the above is the machine, not proof of which GPU ran it.\n",
            );
            missing.push(
                "GPU actually used: unknown. A game stopped by a wall logs no frame, so \
                 nothing recorded which GPU ran it."
                    .into(),
            );
        }
        None => missing.push("System specs: none readable".into()),
    }

    body.push_str("\nDrafted from a local trace by gamebus-setup; filed by hand.\n");
    Report { url, body, missing }
}

/// Percent-encode for a query parameter. Unreserved characters pass through,
/// spaces become `+`, everything else is escaped - enough for a GitHub issue
/// form prefill and no more.
fn encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::{CompatStash, Incoming, Observation};
    use std::collections::BTreeMap;

    fn endpoints() -> Endpoints {
        Endpoints::load()
    }

    fn finding(wall: WallKind, signature: &str, appid: Option<&str>) -> CompatFinding {
        let mut s = CompatStash::default();
        s.record(
            "k",
            Incoming {
                wall,
                title: Some("WARDOGS Playtest".into()),
                steam_appid: appid.map(str::to_string),
                awacy_slug: None,
                observation: Observation {
                    source: "beisl".into(),
                    observed: "2026-09-05".into(),
                    wine: Some("Proton - Experimental".into()),
                    gpu: None,
                    gpu_vendor: Some("amd".into()),
                    specs: None,
                    signature: Some(signature.into()),
                    log: None,
                    trace_dir: None,
                    layer_split: None,
                    attributed_pct: None,
                    record_mode: Some("degraded".into()),
                    extra: BTreeMap::new(),
                },
            },
        );
        s.findings()["k"].clone()
    }

    #[test]
    fn the_awacy_draft_fills_what_the_trace_knows() {
        let f = finding(
            WallKind::KernelAntiCheat,
            "ZwLoadDriver foo.sys",
            Some("4809930"),
        );
        let r = awacy_issue(&f, &endpoints());
        assert!(r.body.contains("Game title: WARDOGS Playtest"));
        assert!(r.body.contains("store.steampowered.com/app/4809930/"));
        assert!(r.body.contains("Status of the game anti-cheat: Broken"));
        assert!(r.url.contains("template=0-new-game.yml"));
        assert!(r.url.contains("game=WARDOGS+Playtest"));
    }

    #[test]
    fn proof_of_mention_is_always_left_to_the_user() {
        let f = finding(WallKind::KernelAntiCheat, "ZwLoadDriver foo.sys", Some("1"));
        let r = awacy_issue(&f, &endpoints());
        assert!(
            r.missing.iter().any(|m| m.starts_with("Proof of mention")),
            "a local trace is not a reputable public source"
        );
        assert!(
            !r.url.contains("anti-cheat-proof"),
            "never prefill a citation we do not have"
        );
    }

    #[test]
    fn denied_is_never_claimed_from_a_trace() {
        for wall in [
            WallKind::KernelAntiCheat,
            WallKind::MissingRuntime,
            WallKind::WineStub,
        ] {
            assert_eq!(status_for(&wall), "Broken");
        }
    }

    #[test]
    fn only_unmistakable_anticheats_are_named() {
        assert_eq!(
            anticheat_for("EasyAntiCheat.sys loaded").0,
            "Easy Anti-Cheat (EAC)"
        );
        assert_eq!(anticheat_for("BEDaisy.sys loaded").0, "BattlEye");
        // A vendor driver a trace cannot attribute stays "Other".
        let (name, _) = anticheat_for("ZwLoadDriver lighthouse_driver.sys");
        assert_eq!(name, "Other (please specify below)");
    }

    #[test]
    fn an_unattributable_anticheat_asks_the_user_to_name_it() {
        let f = finding(
            WallKind::KernelAntiCheat,
            "lighthouse_driver.sys",
            Some("1"),
        );
        let r = awacy_issue(&f, &endpoints());
        assert!(r.missing.iter().any(|m| m.starts_with("Anti-cheat name")));
    }

    #[test]
    fn a_finding_with_no_appid_says_which_required_field_is_missing() {
        let f = finding(WallKind::KernelAntiCheat, "foo.sys", None);
        let awacy = awacy_issue(&f, &endpoints());
        assert!(awacy
            .missing
            .iter()
            .any(|m| m.starts_with("Game main page")));
        let pdb = protondb_report(&f, None, &endpoints());
        assert!(pdb.missing.iter().any(|m| m.starts_with("Steam appid")));
    }

    #[test]
    fn the_protondb_draft_distinguishes_trace_specs_from_a_probe() {
        let f = finding(
            WallKind::WineStub,
            "unimplemented function",
            Some("4809930"),
        );
        let traced = Specs {
            source: SpecSource::Trace,
            gpu: Some("Intel Arc A770".into()),
            ..Default::default()
        };
        let r = protondb_report(&f, Some(&traced), &endpoints());
        assert!(r.body.contains("System (this run)"));
        assert!(
            !r.missing.iter().any(|m| m.starts_with("GPU actually used")),
            "a trace DOES know which GPU ran it"
        );

        let probed = Specs {
            source: SpecSource::System,
            gpus: vec!["Intel Arc A770".into(), "Navi 22".into()],
            ..Default::default()
        };
        let r = protondb_report(&f, Some(&probed), &endpoints());
        assert!(r.body.contains("System (this machine)"));
        assert!(
            r.body.contains("not proof of which GPU ran it"),
            "a probe must not be read as a per-session record"
        );
        assert!(r.missing.iter().any(|m| m.starts_with("GPU actually used")));
    }

    #[test]
    fn the_protondb_url_is_the_games_own_page() {
        let f = finding(WallKind::WineStub, "x", Some("4809930"));
        let r = protondb_report(&f, None, &endpoints());
        assert!(r.url.ends_with("/4809930"), "{}", r.url);
    }

    #[test]
    fn query_encoding_survives_the_characters_a_title_actually_has() {
        assert_eq!(encode("WARDOGS Playtest"), "WARDOGS+Playtest");
        assert_eq!(encode("Tom Clancy's"), "Tom+Clancy%27s");
        assert_eq!(encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode("line\nbreak"), "line%0Abreak");
    }
}
