//! Verification: the database checks, the API lookups, and id drafting.

use std::path::PathBuf;

use serde::Deserialize;

use crate::naming::NamingDb;
use crate::umu_report::{
    self, draft_umu_id, DraftOutcome, DraftedId, FixCheck, Miss, UmuDb, UmuReport, Verification,
    VerificationState,
};

use super::export::csv_field;
use super::fixes;
use super::{
    api_base, basis_label, endpoints, plural_y, scope_line, urlencode, HTTP_TIMEOUT, USER_AGENT,
};

/// `--fetch`: one request to the bare API endpoint (the full dump), validated
/// by parsing before it replaces the cache. Atomic, like every write here.
pub(super) fn fetch_full_dump(api: &str) -> Result<(PathBuf, usize), String> {
    let body = ureq::get(api)
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|e| format!("{api}: {e}"))?
        .into_string()
        .map_err(|e| format!("reading the response body: {e}"))?;
    let db = UmuDb::parse(&body).map_err(|e| format!("response is not the database: {e}"))?;
    let path = UmuDb::cache_path().ok_or("cannot resolve the cache path (no HOME)")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, &body)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{}: {e}", path.display())
        })?;
    Ok((path, db.len()))
}

/// One entry's settled verdict, staged in memory; the stash is written once,
/// at the end, and not at all if verification could not run.
struct Verdict {
    state: VerificationState,
    umu_id: Option<String>,
    note: Option<String>,
    drafted: Option<DraftedId>,
}

/// The umu id a submission for this entry would carry - the database's own
/// (verified or cross-store) or the drafted one. Without an id there is
/// nothing to look a protonfix up under.
fn scope_id(verdict: &Verdict) -> Option<String> {
    verdict
        .umu_id
        .clone()
        .or_else(|| verdict.drafted.as_ref().map(|d| d.id.clone()))
}

/// Returns the human-readable summary as lines rather than printing: the
/// CLI prints them, the TUI logs them into its output pane (a `println!`
/// under raw mode would tear the screen).
pub(super) fn verify(report: &mut UmuReport, db: Option<&UmuDb>) -> Result<Vec<String>, String> {
    let api = api_base();
    let naming = NamingDb::load();
    let today = umu_report::today();
    let mut lines = Vec::new();
    // The launchers' own records first: a codename the Lutris library knows
    // saves every later step a guess. Local files only, never fatal.
    match crate::setup::lutris_library::load() {
        Ok(games) if !games.is_empty() => {
            let filled = crate::setup::lutris_library::fill_codenames(report, &games);
            for line in &filled {
                lines.push(format!("Lutris library: {line}"));
            }
        }
        Ok(_) => {}
        Err(e) => lines.push(format!(
            "Lutris library unreadable ({e}) - codenames stay as they are."
        )),
    }
    if db.is_none() {
        lines.push("No local database (--db / GAMEBUS_UMU_DB / --fetch cache) - every entry goes to the API, and id drafting is skipped: collisions cannot be checked without the full database.".to_string());
    }

    // Only umu misses are checked: a launcher launch (empty umu id) is a
    // gamedb identity record, never a umu-database gap - it is not looked
    // up, not drafted an id, and not fix-checked.
    let mut keys: Vec<String> = report
        .entries()
        .iter()
        .filter(|(_, m)| m.is_umu_miss())
        .map(|(k, _)| k.clone())
        .collect();
    keys.sort();
    let skipped = report.entries().len() - keys.len();
    if skipped > 0 {
        lines.push(format!(
            "Skipped {skipped} launcher launch{} (no umu id) - identity records for gamedb, never checked against the umu database.",
            if skipped == 1 { "" } else { "es" }
        ));
    }
    let mut verdicts: Vec<(String, Verdict)> = Vec::new();
    let mut api_down: Option<String> = None;

    for key in &keys {
        let m = &report.entries()[key];
        // Local pass: the exact launch first, then the title.
        if let Some(db) = db {
            if let Some(code) = m.effective_codename() {
                if let Some(hit) = db.find_store_codename(m.effective_store(), code) {
                    verdicts.push((
                        key.clone(),
                        Verdict {
                            state: VerificationState::AlreadyInDatabase,
                            umu_id: Some(hit.umu_id.clone()),
                            note: None,
                            drafted: None,
                        },
                    ));
                    continue;
                }
            }
            if let Some(title) = m.effective_title() {
                if let Some(hit) = db.find_title(title).first() {
                    verdicts.push((
                        key.clone(),
                        Verdict {
                            state: VerificationState::CrossStoreId,
                            umu_id: Some(hit.umu_id.clone()),
                            note: Some(format!("id held by the {} entry", hit.store)),
                            drafted: None,
                        },
                    ));
                    continue;
                }
            }
        }
        // The local copy settled nothing - ask the live API, once per entry,
        // until the first transport failure (no hammering a dead endpoint).
        if api_down.is_none() {
            match api_check(&api, m) {
                Ok(Some(verdict)) => {
                    verdicts.push((key.clone(), verdict));
                    continue;
                }
                Ok(None) => {
                    verdicts.push((
                        key.clone(),
                        Verdict {
                            state: VerificationState::ConfirmedMissing,
                            umu_id: None,
                            note: None,
                            drafted: None,
                        },
                    ));
                    continue;
                }
                Err(e) => api_down = Some(e),
            }
        }
        // API unreachable. With a local copy the verdict stands on that copy
        // alone - said so honestly. Without one there is nothing to verify
        // against: error out with the stash untouched.
        match db {
            Some(_) => verdicts.push((
                key.clone(),
                Verdict {
                    state: VerificationState::ConfirmedMissing,
                    umu_id: None,
                    note: Some("API unreachable - checked against the local copy only".into()),
                    drafted: None,
                },
            )),
            None => {
                return Err(format!(
                    "no local database and the API is unreachable ({})",
                    api_down.as_deref().unwrap_or("unknown error")
                ));
            }
        }
    }

    // Draft ids for the confirmed-missing entries. Needs the full database -
    // the collision check is mandatory, so no database means no drafts.
    if let Some(db) = db {
        for (key, verdict) in &mut verdicts {
            if verdict.state != VerificationState::ConfirmedMissing {
                continue;
            }
            let m = &report.entries()[key.as_str()];
            let Some(title) = m.effective_title() else {
                continue;
            };
            let appid = naming.as_ref().and_then(|n| n.steam_appid_for_title(title));
            match draft_umu_id(db, title, m.effective_codename(), appid) {
                DraftOutcome::Drafted { id, basis } => {
                    verdict.drafted = Some(DraftedId {
                        id,
                        basis,
                        collision_checked: today.clone(),
                    });
                }
                DraftOutcome::ExistingId { id } => {
                    // The would-be id already names this very game: that IS
                    // the cross-store id, not a fresh draft.
                    verdict.state = VerificationState::CrossStoreId;
                    verdict.umu_id = Some(id);
                    verdict.note = Some("id found via the drafting rules".into());
                }
                DraftOutcome::Collision { id, existing_title } => {
                    let collision = format!(
                        "draft {id} collides with the existing entry '{existing_title}' - left as umu-FIXME"
                    );
                    verdict.note = Some(match verdict.note.take() {
                        Some(prior) => format!("{prior}; {collision}"),
                        None => collision,
                    });
                }
                DraftOutcome::NoBasis => {}
            }
        }
    }

    // Does the game need umu at all? The database takes entries for games
    // that require a fix in Proton and says so in its opening paragraph, so
    // an entry without a fix upstream is one the maintainers do not want.
    // Unreachable list: say so and leave every earlier verdict standing -
    // "unchecked" and "runs fine" are not the same claim.
    let fix_list = match fixes::load_for_verify() {
        Ok(list) => {
            lines.push(format!(
                "Read the protonfix list: {} game{} need umu's help.",
                list.len(),
                if list.len() == 1 { "" } else { "s" }
            ));
            Some(list)
        }
        Err(e) => {
            lines.push(format!(
                "Could not read the protonfix list ({e}) - whether these games need umu stays unchecked."
            ));
            None
        }
    };

    // Single write-back, then the human-readable summary.
    for (key, verdict) in &verdicts {
        let scope = fix_list.as_ref().map(|list| {
            scope_id(verdict).map(|id| FixCheck {
                fixes: list.fixes_for(&id).to_vec(),
                umu_id: id,
                checked: today.clone(),
            })
        });
        report.update(key, |m| {
            m.verification = Some(Verification {
                state: verdict.state,
                umu_id: verdict.umu_id.clone(),
                checked: today.clone(),
                note: verdict.note.clone(),
            });
            m.drafted_id = verdict.drafted.clone();
            // An id that changed since the last run makes the old check
            // meaningless; so does losing the id entirely.
            if let Some(scope) = &scope {
                m.fix = scope.clone();
            }
        });
    }
    report.save();

    lines.push(format!(
        "Verified {} entr{} against the database:",
        verdicts.len(),
        plural_y(verdicts.len())
    ));
    for (key, verdict) in &verdicts {
        let m = &report.entries()[key.as_str()];
        let title = m.effective_title().unwrap_or("(unresolved)");
        let line = match (&verdict.state, &verdict.drafted) {
            (VerificationState::AlreadyInDatabase, _) => format!(
                "already in the database as {} - the launcher missed, not the database",
                verdict.umu_id.as_deref().unwrap_or("?")
            ),
            (VerificationState::CrossStoreId, _) => format!(
                "known under another store as {} - likely the id to submit",
                verdict.umu_id.as_deref().unwrap_or("?")
            ),
            (VerificationState::ConfirmedMissing, Some(d)) => format!(
                "missing from the database - drafted {} ({}, collision-checked)",
                d.id,
                basis_label(d.basis)
            ),
            (VerificationState::ConfirmedMissing, None) => "missing from the database".to_string(),
        };
        lines.push(format!("  {title:<28} {line}"));
        if let Some(note) = &verdict.note {
            lines.push(format!("  {:<28} note: {note}", ""));
        }
        if let Some(scope) = scope_line(m) {
            lines.push(format!("  {:<28} {scope}", ""));
        }
    }
    Ok(lines)
}

/// The pure half of a manual assignment: shape rules, then the same
/// mandatory collision check every draft gets.
pub(super) fn check_assignment(
    db: &UmuDb,
    title: Option<&str>,
    id: &str,
) -> Result<String, String> {
    let Some(suffix) = id.strip_prefix("umu-").filter(|s| !s.is_empty()) else {
        return Err(format!(
            "'{id}' is not a umu id - the database wants umu-<something>."
        ));
    };
    if !suffix
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "'{id}' carries characters the database's ids never use."
        ));
    }
    let holders = db.find_umu_id(id);
    if holders.is_empty() {
        let steam_note = if suffix.chars().all(|c| c.is_ascii_digit()) {
            " - numeric, so Proton will treat it as the Steam appid; make sure it is one"
        } else {
            ""
        };
        Ok(format!(
            "free in the database, collision-checked{steam_note}."
        ))
    } else if title.is_some_and(|t| holders.iter().any(|e| e.title.eq_ignore_ascii_case(t))) {
        Ok(format!(
            "already names this very game ({}) - the cross-store id.",
            holders[0].title
        ))
    } else {
        Err(format!(
            "{id} already names '{}' in the database - not saved.",
            holders[0].title
        ))
    }
}

/// The API's two lookups for one entry. Only the store+codename lookup is
/// authoritative - verified live 2026-08-07, it matches exactly. The title
/// lookup does substring matching (`?title=Control` returns Ground
/// Control's ids) and its rows carry no title to compare against, so a hit
/// there is advisory only: it lands in the note for the human, never in the
/// verdict's id. `Ok(None)` means the API answered and found nothing.
fn api_check(api: &str, m: &Miss) -> Result<Option<Verdict>, String> {
    if let Some(code) = m
        .effective_codename()
        .filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("none"))
    {
        if m.effective_store() != "none" {
            let url = format!(
                "{api}?store={}&codename={}",
                urlencode(m.effective_store()),
                urlencode(code)
            );
            if let Some(umu_id) = api_umu_ids(&url)?.into_iter().next() {
                return Ok(Some(Verdict {
                    state: VerificationState::AlreadyInDatabase,
                    umu_id: Some(umu_id),
                    note: None,
                    drafted: None,
                }));
            }
        }
    }
    if let Some(title) = m.effective_title() {
        let url = format!("{api}?title={}", urlencode(title));
        let mut ids = api_umu_ids(&url)?;
        ids.dedup();
        if !ids.is_empty() {
            return Ok(Some(Verdict {
                state: VerificationState::ConfirmedMissing,
                umu_id: None,
                note: Some(format!(
                    "live title lookup matched {} (substring match - check whether one is this game before submitting)",
                    ids.join(", ")
                )),
                drafted: None,
            }));
        }
    }
    Ok(None)
}

/// One API query. Both lookup shapes answer `[{"umu_id": …, …}, …]`; a miss
/// is the empty array.
fn api_umu_ids(url: &str) -> Result<Vec<String>, String> {
    #[derive(Deserialize)]
    struct Row {
        umu_id: Option<String>,
    }
    let rows: Vec<Row> = ureq::get(url)
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|e| format!("{url}: {e}"))?
        .into_json()
        .map_err(|e| format!("{url}: unexpected response: {e}"))?;
    Ok(rows.into_iter().filter_map(|r| r.umu_id).collect())
}

/// `--check-prs`: scan open upstream merge requests for rows that look like
/// our entries. Substring evidence only - a hit demotes the entry from the
/// exports and names the PR, nothing more.
pub(super) fn check_open_prs(report: &mut UmuReport) -> Result<(), String> {
    #[derive(Deserialize)]
    struct OpenPr {
        number: u64,
        title: String,
        diff_url: String,
    }
    let open_prs_url = &endpoints().umu_open_prs;
    let prs: Vec<OpenPr> = ureq::get(open_prs_url)
        .set("User-Agent", USER_AGENT)
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|e| format!("{open_prs_url}: {e}"))?
        .into_json()
        .map_err(|e| format!("unexpected GitHub response: {e}"))?;
    println!(
        "Checking {} open merge request(s) (best-effort)…",
        prs.len()
    );

    // Every run re-evaluates from scratch: a PR that closed unmerged, or a
    // substring false positive, must not hold an entry out of the exports
    // forever. Cleared only once the PR list actually arrived (above).
    let stale: Vec<String> = report
        .entries()
        .iter()
        .filter(|(_, m)| m.possible_pr.is_some())
        .map(|(k, _)| k.clone())
        .collect();
    for key in &stale {
        report.update(key, |m| m.possible_pr = None);
    }

    let mut hits: Vec<(String, String)> = Vec::new();
    for pr in &prs {
        let diff = ureq::get(&pr.diff_url)
            .set("User-Agent", USER_AGENT)
            .timeout(HTTP_TIMEOUT)
            .call()
            .map_err(|e| e.to_string())
            .and_then(|r| r.into_string().map_err(|e| e.to_string()));
        let diff = match diff {
            Ok(diff) => diff.to_lowercase(),
            Err(e) => {
                eprintln!("  PR #{}: diff unavailable ({e}) - skipped", pr.number);
                continue;
            }
        };
        for (key, m) in report.entries() {
            if hits.iter().any(|(k, _)| k == key) {
                continue;
            }
            // Only candidates can be in a submission of ours; matching the
            // rest would annotate entries the pipeline never exports.
            if !super::umu_candidate(m) {
                continue;
            }
            if diff_mentions(&diff, m) {
                hits.push((key.clone(), format!("PR #{} - {}", pr.number, pr.title)));
            }
        }
    }

    if hits.is_empty() {
        println!("  No open merge request seems to contain these entries.");
        if !stale.is_empty() {
            println!(
                "  ({} earlier possibly-submitted annotation(s) no longer match and were cleared.)",
                stale.len()
            );
        }
    }
    for (key, pr) in hits {
        let title = report.entries()[&key]
            .effective_title()
            .map(str::to_string)
            .unwrap_or_else(|| key.clone());
        println!("  {title}: possibly already submitted ({pr})");
        report.update(&key, |m| m.possible_pr = Some(pr.clone()));
    }
    report.save();
    Ok(())
}

/// Does this (lowercased) PR diff add a CSV row that looks like this miss?
/// Cells, not free text - titles are ordinary words and a bare substring
/// match would flag half the tracker. Titles match both the plain and the
/// quoted form: the upstream CSV quotes comma-carrying titles
/// (`"Warhammer 40,000: Space Marine",gog,…`).
fn diff_mentions(diff_lower: &str, m: &Miss) -> bool {
    let codename_hit = m
        .effective_codename()
        .filter(|c| c.len() >= 4 && !c.eq_ignore_ascii_case("none"))
        .is_some_and(|c| diff_lower.contains(&format!(",{},", c.to_lowercase())));
    // The effective title is what a submission would carry - the resolver's
    // guess matters to nobody once the user corrected it.
    let title_hit = m.effective_title().is_some_and(|t| {
        let plain = format!("\n+{},", t.to_lowercase());
        let quoted = format!("\n+{},", csv_field(t).to_lowercase());
        diff_lower.contains(&plain) || diff_lower.contains(&quoted)
    });
    codename_hit || title_hit
}

#[cfg(test)]
mod tests {
    use super::super::pick_report;
    use super::*;

    fn miss(title: Option<&str>, codename: Option<&str>) -> Miss {
        Miss {
            title: title.map(str::to_string),
            store: "egs".into(),
            codename: codename.map(str::to_string),
            umu_id: "umu-0".into(),
            title_source: None,
            confidence: None,
            executable: None,
            first_seen: "2026-08-07".into(),
            last_seen: "2026-08-07".into(),
            launcher: None,
            launcher_name: None,
            launcher_dir: None,
            codename_source: None,
            runner: None,
            verification: None,
            drafted_id: None,
            possible_pr: None,
            fix: None,
            store_override: None,
            codename_override: None,
            codename_override_source: None,
            title_override: None,
            dismissed: None,
            umu_promoted: None,
        }
    }

    #[test]
    fn diff_matching_handles_quoted_and_plain_titles() {
        // The upstream CSV quotes comma-carrying titles; a PR adding such a
        // row must still match.
        let diff =
            "\n+\"warhammer 40,000: space marine\",gog,1668484481,umu-55150,,,\n".to_string();
        assert!(diff_mentions(
            &diff,
            &miss(Some("Warhammer 40,000: Space Marine"), None)
        ));

        let plain = "\n+borderlands 3,egs,catnip,umu-397540,bl3,,\n".to_string();
        assert!(diff_mentions(&plain, &miss(Some("Borderlands 3"), None)));
        // A title appearing as free text (not a row start) is NOT a hit.
        let prose = "this pr improves borderlands 3, the entry\n+something,else,x,umu-1a,,,\n";
        assert!(!diff_mentions(prose, &miss(Some("Borderlands 3"), None)));
    }

    #[test]
    fn diff_matching_uses_codenames_only_when_meaningful() {
        let diff = ",calluna,".to_string();
        assert!(diff_mentions(&diff, &miss(None, Some("Calluna"))));
        // Short or placeholder codenames would match everything.
        assert!(!diff_mentions(&diff, &miss(None, Some("none"))));
        assert!(!diff_mentions("...,abc,...", &miss(None, Some("abc"))));
    }

    #[test]
    fn manual_ids_are_shape_checked_and_collision_checked() {
        let db = UmuDb::parse(concat!(
            "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)\n",
            "Borderlands 3,egs,Catnip,umu-397540,bl3,,\n",
        ))
        .unwrap();
        // Shape rules.
        assert!(check_assignment(&db, Some("X"), "397540").is_err());
        assert!(check_assignment(&db, Some("X"), "umu-").is_err());
        assert!(check_assignment(&db, Some("X"), "umu-a b").is_err());
        // Free id passes; a numeric one warns about the Proton appid rule.
        assert!(check_assignment(&db, Some("Control"), "umu-870780")
            .unwrap()
            .contains("Steam appid"));
        assert!(check_assignment(&db, Some("Control"), "umu-controlgame").is_ok());
        // Same-title holder is the cross-store id; different title rejects.
        assert!(check_assignment(&db, Some("Borderlands 3"), "umu-397540").is_ok());
        assert!(check_assignment(&db, Some("Not Borderlands"), "umu-397540").is_err());
    }

    // ---- codename_override threading: everything that reads a codename
    // must read the effective one.

    #[test]
    fn verify_looks_up_store_and_codename_with_the_override() {
        let db = UmuDb::parse(concat!(
            "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)\n",
            "Witchery,gog,1423049311,umu-witchery1,,,\n",
        ))
        .unwrap();
        let (mut report, key) = pick_report("gog", "witchery");
        report.update(&key, |m| {
            m.title = Some("Witchery".into());
            m.codename_override = Some("1423049311".into());
        });
        // The overridden codename hits locally - the API is never reached
        // (there is nothing serving it here, so a request would error out).
        let lines = verify(&mut report, Some(&db)).expect("local verify");
        assert!(
            lines.iter().any(|l| l.contains("already in the database")),
            "{lines:?}"
        );
        let v = report.entries()[&key].verification.as_ref().unwrap();
        assert_eq!(v.state, VerificationState::AlreadyInDatabase);
        assert_eq!(v.umu_id.as_deref(), Some("umu-witchery1"));
    }

    #[test]
    fn verify_skips_launcher_launches_entirely() {
        let db = UmuDb::parse(concat!(
            "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)\n",
            "Borderlands 3,egs,Catnip,umu-397540,bl3,,\n",
        ))
        .unwrap();
        // One umu miss that settles locally, plus one launcher launch
        // (empty umu id). The latter must never be verified, drafted, or
        // fix-checked - and must never send verify to the API (nothing
        // serves it here, so a request would error the whole run out).
        let (mut report, key) = pick_report("egs", "Catnip");
        report.note_launch("itchio", Some("926077"), "", "itchio:926077");
        let lines = verify(&mut report, Some(&db)).expect("local verify");

        let launcher = &report.entries()["itchio:926077"];
        assert!(
            launcher.verification.is_none(),
            "a launcher launch was verified"
        );
        assert!(
            launcher.drafted_id.is_none(),
            "a launcher launch was drafted an id"
        );
        assert!(launcher.fix.is_none(), "a launcher launch was fix-checked");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Skipped 1 launcher launch")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("Verified 1 entry")),
            "the count includes the skipped entry: {lines:?}"
        );
        // The umu miss itself was verified as before.
        assert!(report.entries()[&key].verification.is_some());
    }

    #[test]
    fn verify_looks_up_the_title_with_the_override() {
        let db = UmuDb::parse(concat!(
            "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)\n",
            "Project Hospital,gog,1660194629,umu-project1,,,\n",
        ))
        .unwrap();
        let (mut report, key) = pick_report("gog", "someothercode");
        // The resolver's title is wrong (the Spellcraft incident); the
        // user's correction is what must reach the title lookup - a hit
        // here keeps the API out of the picture entirely.
        report.update(&key, |m| {
            m.title = Some("Spellcraft".into());
            m.title_override = Some("Project Hospital".into());
        });
        let lines = verify(&mut report, Some(&db)).expect("local verify");
        assert!(
            lines.iter().any(|l| l.contains("Project Hospital")),
            "{lines:?}"
        );
        let v = report.entries()[&key].verification.as_ref().unwrap();
        assert_eq!(v.state, VerificationState::CrossStoreId);
        assert_eq!(v.umu_id.as_deref(), Some("umu-project1"));
    }
}
