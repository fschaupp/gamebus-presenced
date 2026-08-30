//! The exports: submission partitioning, the CSV, and the merge-request text.

use std::io::Write;

use crate::umu_report::{Confidence, DraftBasis, Miss, UmuReport, VerificationState};

use super::{basis_label, confidence_label, endpoints, plural_y, urlencode, CSV_HEADER};

/// One submission-ready row, shared by both exports.
struct SubmissionRow<'a> {
    miss: &'a Miss,
    title: &'a str,
    umu_id: String,
    /// Where the id came from, for the NOTE column and the evidence list.
    id_provenance: Option<String>,
    /// The CODENAME column value. The README's standalone rule pairs
    /// store `none` with codename `none` - a codename without a store
    /// namespace is meaningless upstream, so it moves to the NOTE instead.
    codename: String,
    /// True when the id provably honors the "Steam appid when on Steam"
    /// rule: verified cross-store from the database, or drafted from a
    /// detectable.json Steam sku. Slug/codename drafts only prove Discord's
    /// file had no entry - NOT that the game is absent from Steam.
    steam_rule_certain: bool,
    /// The protonfixes this row's id is served by - why the entry belongs in
    /// the database at all. Empty only for a candidate that earned its place
    /// another way: promoted by the user, or already active in the database
    /// under another store; every other fix-less entry is held back before
    /// it becomes a row.
    fixes: &'a [String],
}

/// Split the stash into rows worth submitting and entries listed after the
/// block with the reason they were held back.
///
/// Only [`super::umu_candidate`] entries can become rows (owner policy
/// 2026-08-24): a launcher launch never went through umu, and a umu miss
/// without a promotion, a cross-store match, or a protonfix stays a
/// gamedb-only identity record until the user promotes it in the TUI.
fn partition(report: &UmuReport) -> (Vec<SubmissionRow<'_>>, Vec<(&Miss, String)>) {
    let mut rows = Vec::new();
    let mut held = Vec::new();
    let mut entries: Vec<&Miss> = report.entries().values().collect();
    entries.sort_by(|a, b| a.last_seen.cmp(&b.last_seen).reverse());

    for m in entries {
        // The outermost gate: a launch that never went through umu cannot
        // be a umu-database gap, whatever its annotations say.
        if !m.is_umu_miss() {
            held.push((
                m,
                "a launcher launch (no umu id) - a gamedb identity record, nothing for the umu database"
                    .to_string(),
            ));
            continue;
        }
        if m.dismissed.is_some() {
            held.push((m, "dismissed by you (d in the TUI restores it)".to_string()));
            continue;
        }
        if let Some(v) = &m.verification {
            if v.state == VerificationState::AlreadyInDatabase {
                held.push((
                    m,
                    format!(
                        "already in the database as {} - a launcher-side miss, not a gap",
                        v.umu_id.as_deref().unwrap_or("?")
                    ),
                ));
                continue;
            }
        }
        if let Some(pr) = &m.possible_pr {
            held.push((m, format!("possibly already submitted: {pr}")));
            continue;
        }
        // A title the user typed outranks any resolver guess - an override
        // is confident by definition; the resolver gate stays for the rest.
        let confident = m.title_override.is_some()
            || matches!(
                m.confidence,
                Some(Confidence::High) | Some(Confidence::Medium)
            );
        let Some(title) = m.effective_title().filter(|_| confident) else {
            held.push((m, "unresolved or low-confidence title".to_string()));
            continue;
        };
        let (umu_id, id_provenance, steam_rule_certain) = match (&m.verification, &m.drafted_id) {
            (Some(v), _) if v.state == VerificationState::CrossStoreId && v.umu_id.is_some() => (
                v.umu_id.clone().expect("checked"),
                Some(format!(
                    "id shared from the database's existing entry (verified {})",
                    v.checked
                )),
                true,
            ),
            (_, Some(d)) => (
                d.id.clone(),
                Some(format!(
                    "id drafted from {}, collision-checked {}",
                    basis_label(d.basis),
                    d.collision_checked
                )),
                d.basis == DraftBasis::SteamSku,
            ),
            _ => ("umu-FIXME".to_string(), None, false),
        };
        // The database's scope rule, straight from its opening paragraph:
        // it collects games that require fixes in Proton, and games that run
        // out of the box "have no need be added". A row for a game with no
        // protonfix is review work for the maintainers and nothing for the
        // player, so it never reaches the submission.
        if umu_id == "umu-FIXME" {
            held.push((
                m,
                "no id could be drafted, so there is nothing to look a protonfix up under"
                    .to_string(),
            ));
            continue;
        }
        // The candidacy gate (owner policy 2026-08-24): a row needs a
        // promotion, a cross-store match, or a protonfix under this very id
        // (a fix checked under a stale id proves nothing about this row).
        // Promotion and the cross-store match carry an entry even without a
        // fix - the user said the database should have it, or the database
        // already actively serves the game under another store.
        let scope = m
            .fix
            .as_ref()
            .filter(|f| f.umu_id.eq_ignore_ascii_case(&umu_id));
        let cross_store = m
            .verification
            .as_ref()
            .is_some_and(|v| v.state == VerificationState::CrossStoreId);
        if m.umu_promoted.is_none() && !cross_store && !scope.is_some_and(|f| f.has_fix()) {
            held.push((
                m,
                match scope {
                    None => format!(
                        "whether {umu_id} needs a protonfix is unchecked - run --verify \
                         (network), or promote it in the TUI (u)"
                    ),
                    // Same distinction the review list draws: a firm id
                    // proves the game runs without umu's help, a guessed one
                    // only means we could not show that it needs any.
                    Some(scope) => match super::id_is_firm(m) {
                        true => format!(
                            "no protonfix for {umu_id} - the game runs out of the box, and the \
                             database only wants games that need a fix (checked {}); not \
                             promoted and no cross-store match - u in the TUI promotes it if \
                             the database should carry it anyway",
                            scope.checked
                        ),
                        false => format!(
                            "no protonfix for {umu_id}, and that id is our own guess - nothing \
                             shows this game needs umu (checked {}); not promoted and no \
                             cross-store match - u in the TUI promotes it if the database \
                             should carry it anyway",
                            scope.checked
                        ),
                    },
                },
            ));
            continue;
        }
        let codename = if m.effective_store() == "none" {
            "none".to_string()
        } else {
            m.effective_codename()
                .map(str::to_string)
                .unwrap_or_else(|| "none".into())
        };
        // The database's GOG rule: the codename is the numeric gogdb.org
        // product id. Heroic launches satisfy it by construction (Heroic's
        // GOG app name IS that id); anything else needs a human lookup and
        // is held out of the submission until it gets one.
        let is_gogdb_id = !codename.is_empty() && codename.bytes().all(|b| b.is_ascii_digit());
        if m.effective_store() == "gog" && !is_gogdb_id {
            held.push((
                m,
                format!(
                    "gog codename must be the numeric gogdb.org product id \
                     (launcher reported '{codename}')"
                ),
            ));
            continue;
        }
        rows.push(SubmissionRow {
            miss: m,
            title,
            umu_id,
            id_provenance,
            codename,
            steam_rule_certain,
            fixes: scope.map(|s| s.fixes.as_slice()).unwrap_or(&[]),
        });
    }
    (rows, held)
}

/// The submission wants executable names, not this machine's absolute
/// paths - those embed `/home/<user>` and whatever else the install layout
/// leaks. Basename only, either separator (Wine paths carry backslashes).
fn exe_basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn csv_line(row: &SubmissionRow<'_>) -> String {
    let m = row.miss;
    // The NOTE column is the database's, not ours: it carries game-related
    // remarks only (which of two standalone versions a row means, per the
    // README's Genshin example). Provenance - who resolved the title, how
    // the id was drafted - belongs in the merge request's evidence text,
    // never in the submitted CSV. The column stays empty.
    format!(
        "{},{},{},{},,,{}",
        csv_field(row.title),
        m.effective_store().to_lowercase(),
        // Codename comes verbatim from an untrusted process's environment
        // (HEROIC_APP_NAME) - escaped like every other external field, or a
        // comma in it would shift the columns and forge the UMU_ID cell.
        csv_field(&row.codename),
        row.umu_id,
        m.executable
            .as_deref()
            .map(exe_basename)
            .map(csv_field)
            .unwrap_or_default(),
    )
}

/// `--export`: the submission CSV on stdout, held-back entries on stderr.
pub(super) fn export_csv(report: &UmuReport) {
    let (rows, held) = partition(report);
    println!("# umu-database submission draft - review before submitting!");
    println!("# Rules: {}#readme", endpoints().umu_repository);
    println!(
        "# Only umu candidates are listed: an upstream protonfix, an active cross-store entry, or your own promotion."
    );
    println!("{CSV_HEADER}");
    for row in &rows {
        println!("{}", csv_line(row));
    }
    for (m, reason) in &held {
        eprintln!(
            "(held back: {} - {reason})",
            m.effective_title().unwrap_or("(unresolved)")
        );
    }
}

/// `--export-md`: the slim merge request - title line, one honest paragraph,
/// the CSV in a fenced block, per-entry evidence, and the README's own rules
/// as a checklist.
pub(super) fn export_markdown(
    report: &UmuReport,
    dest: Option<&std::path::Path>,
) -> Result<(), String> {
    let (rows, held) = partition(report);
    if rows.is_empty() {
        // The common case now that the scope rule is enforced: every game
        // ran fine. That is a result, not a failure - say which it was.
        let out_of_scope = held
            .iter()
            .filter(|(_, reason)| reason.contains("protonfix"))
            .count();
        return Err(match out_of_scope {
            0 => "Nothing to export: no verified, submission-ready entries. Run --verify first, or check the held-back reasons in --export.".to_string(),
            n => format!(
                "Nothing to export: {n} of {} entries need no protonfix, so the database does not want them - it collects games that require a fix in Proton. The rest is in --export's held-back list.",
                held.len()
            ),
        });
    }
    let titles: Vec<&str> = rows.iter().map(|r| r.title).collect();
    // Only tick the Steam rule when every id provably honors it; a slug or
    // codename draft leaves "is this on Steam?" a genuinely open question
    // for the human submitter.
    let steam_rule = rows.iter().all(|r| r.steam_rule_certain);

    let mut md = String::new();
    md.push_str(&format!(
        "# Add {} game{}: {}\n\n",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" },
        titles.join(", ")
    ));
    // The honest paragraph: when every row rests on a protonfix, say
    // exactly that; when a promoted or cross-store row carries none, the
    // blanket fix claim would be a lie.
    let all_fixed = rows.iter().all(|r| !r.fixes.is_empty());
    if all_fixed {
        md.push_str(&format!(
            "Each game below already has a protonfix, and these store copies launch \
             through umu with `GAMEID=umu-0`, so the fix never reaches them. The rows \
             map each copy onto the id its fix is filed under. \
             [gamebus-presenced]({}) resolved the titles on a live system from the \
             launchers' own install records; per-entry evidence, fix included, below.\n\n",
            "https://github.com/fschaupp/gamebus-presenced"
        ));
    } else {
        md.push_str(&format!(
            "These store copies launch through umu with `GAMEID=umu-0`. Rows with a \
             protonfix map the copy onto the id the fix is filed under; the rest map \
             a copy onto an id the database already serves for another store, or were \
             reviewed and submitted deliberately. \
             [gamebus-presenced]({}) resolved the titles on a live system from the \
             launchers' own install records; per-entry evidence below.\n\n",
            "https://github.com/fschaupp/gamebus-presenced"
        ));
    }
    md.push_str("Rows for `umu-database.csv`:\n\n```csv\n");
    md.push_str(CSV_HEADER);
    md.push('\n');
    for row in &rows {
        md.push_str(&csv_line(row));
        md.push('\n');
    }
    md.push_str("```\n\n## Evidence\n\n");
    for row in &rows {
        let m = row.miss;
        let store = m.effective_store().to_lowercase();
        // The database's per-store codename authorities, linked so the
        // reviewer can check without hunting: gogdb.org product page for
        // GOG ids; egdata.app for the EGS Builds App Name.
        let codename_ref = match store.as_str() {
            "gog" => format!(
                " ([gogdb.org product]({}/{}))",
                endpoints().gog_gogdb_product,
                urlencode(&row.codename)
            ),
            "egs" => " (egdata.app Builds App Name, via Heroic)".to_string(),
            _ => String::new(),
        };
        let advisory = m
            .verification
            .as_ref()
            .and_then(|v| v.note.as_deref())
            .map(|n| format!("; {n}"))
            .unwrap_or_default();
        // Why the row exists, stated per entry: the fix, linked so a
        // reviewer sees in one click that this game needs umu - or, for a
        // fix-less candidate, the promotion or cross-store entry that
        // earned it the place instead.
        let why = if row.fixes.is_empty() {
            match &m.umu_promoted {
                Some(date) => format!(
                    "no protonfix found; promoted for submission by the reviewing user {date}"
                ),
                None => "no protonfix found; the database already serves this game under \
                         another store's entry"
                    .to_string(),
            }
        } else {
            format!(
                "fix: {}",
                row.fixes
                    .iter()
                    .map(|p| format!("[`{p}`]({})", super::fixes::fix_url(p)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        md.push_str(&format!(
            "- **{}** - store `{store}`, codename `{}`{codename_ref}{}; {}; {}{advisory}; {why}.\n",
            row.title,
            row.codename,
            m.executable
                .as_deref()
                .map(|e| format!(", exe `{}`", exe_basename(e)))
                .unwrap_or_default(),
            title_provenance(m),
            row.id_provenance
                .as_deref()
                .unwrap_or("id carried over from the database"),
        ));
    }
    md.push_str("\n## Checklist\n\n");
    // The checklist is for what a human must verify, not for what the code
    // guarantees mechanically - lowercase STORE entries and the empty NOTE
    // column are enforced at the emit site, and a self-ticked box for them
    // would be a box nobody actually checked.
    md.push_str("- [ ] Titles match the store's spelling and capitalization\n");
    md.push_str(&format!(
        "- [{}] Every id follows the database rules (Steam appid when the game is on Steam)\n",
        if steam_rule { 'x' } else { ' ' }
    ));
    md.push_str(
        "- [x] Non-Steam ids contain a letter (a numeric id would be parsed as a SteamAppId)\n",
    );
    md.push_str(
        "- [x] GOG codenames are numeric gogdb.org product ids (non-conforming entries are held back)\n",
    );
    // Provable only when the daemon itself derived the store AND the
    // codename: an egs guess straight from HEROIC_APP_NAME IS the egdata
    // Builds App Name. A store the user overrode to egs, or a codename
    // override (whose origin the stash cannot prove), leaves the box open
    // for the human.
    let egs_appname = rows
        .iter()
        .filter(|r| r.miss.effective_store() == "egs")
        .all(|r| {
            r.miss.store == "egs"
                && r.miss.store_override.is_none()
                && r.miss.codename_override.is_none()
        });
    md.push_str(&format!(
        "- [{}] EGS codenames are the egdata.app Builds \"App Name\" (Heroic reports it verbatim)\n",
        if egs_appname { 'x' } else { ' ' }
    ));
    if !held.is_empty() {
        md.push_str("\n<!-- Held back, not part of this submission:\n");
        for (m, reason) in &held {
            md.push_str(&format!(
                "  {} - {reason}\n",
                m.effective_title().unwrap_or("(unresolved)")
            ));
        }
        md.push_str("-->\n");
    }

    match dest {
        Some(path) => {
            std::fs::write(path, &md).map_err(|e| format!("{}: {e}", path.display()))?;
            println!(
                "Wrote the merge-request text for {} entr{} to {}",
                rows.len(),
                plural_y(rows.len()),
                path.display()
            );
            println!("Submit at: {}", endpoints().umu_repository);
        }
        None => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(md.as_bytes());
        }
    }
    Ok(())
}

/// The evidence list's title-provenance phrase. An overridden title is the
/// user's word, not the resolver's - saying "high confidence" over it would
/// dress a human correction up as machine evidence.
fn title_provenance(m: &Miss) -> String {
    match (&m.title_override, m.title.as_deref()) {
        (Some(_), Some(resolved)) => format!("title set by you (resolver said '{resolved}')"),
        (Some(_), None) => "title set by you".to_string(),
        (None, _) => format!(
            "title from {} ({} confidence)",
            m.title_source.as_deref().unwrap_or("unknown"),
            confidence_label(m.confidence)
        ),
    }
}

/// Quote a CSV field when it needs it.
pub(super) fn csv_field(v: &str) -> String {
    if v.contains(',') || v.contains('"') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::super::pick_report;
    use super::*;
    use crate::umu_report::{DraftedId, FixCheck, UmuReport};

    /// A drafted id plus the protonfix that justifies submitting it - the
    /// state an entry reaches after `--verify` when the game does need umu.
    fn in_scope(report: &mut UmuReport, key: &str, id: &str) {
        report.update(key, |m| {
            m.drafted_id = Some(DraftedId {
                id: id.to_string(),
                basis: DraftBasis::SteamSku,
                collision_checked: "2026-08-22".into(),
            });
            m.fix = Some(FixCheck {
                umu_id: id.to_string(),
                fixes: vec![format!(
                    "gamefixes-steam/{}.py",
                    id.trim_start_matches("umu-")
                )],
                checked: "2026-08-22".into(),
            });
        });
    }

    #[test]
    fn only_games_that_need_a_protonfix_are_submitted() {
        // The upstream rule: the database collects games that require fixes
        // in Proton. A game that runs out of the box is held back with that
        // reason, however well-resolved and collision-checked its id is.
        let (mut report, key) = pick_report("egs", "Catnip");
        in_scope(&mut report, &key, "umu-397540");
        let (rows, held) = partition(&report);
        assert_eq!(rows.len(), 1, "a game with a fix belongs in the submission");
        assert_eq!(rows[0].fixes, ["gamefixes-steam/397540.py"]);
        assert!(held.is_empty());

        report.update(&key, |m| {
            m.fix.as_mut().expect("set above").fixes.clear();
        });
        let (rows, held) = partition(&report);
        assert!(rows.is_empty(), "a game with no protonfix reached a row");
        assert!(held[0].1.contains("no protonfix"), "{}", held[0].1);
        assert!(held[0].1.contains("runs out of the box"), "{}", held[0].1);
    }

    #[test]
    fn a_missing_fix_under_a_guessed_id_claims_less_than_one_under_a_real_id() {
        // A Steam-sku draft is the id the database itself would assign, so
        // "no fix for it" really does mean the game runs out of the box.
        let (mut report, key) = pick_report("egs", "Catnip");
        in_scope(&mut report, &key, "umu-397540");
        report.update(&key, |m| m.fix.as_mut().expect("set above").fixes.clear());
        let (_, held) = partition(&report);
        assert!(held[0].1.contains("runs out of the box"), "{}", held[0].1);

        // A made-up slug proves nothing about Steam: a fix could sit under
        // an appid we never learned, so the reason must not claim otherwise.
        report.update(&key, |m| {
            m.drafted_id.as_mut().expect("set above").basis = DraftBasis::TitleSlug;
        });
        let (_, held) = partition(&report);
        assert!(held[0].1.contains("our own guess"), "{}", held[0].1);
        assert!(!held[0].1.contains("runs out of the box"), "{}", held[0].1);
    }

    #[test]
    fn an_unchecked_or_stale_scope_is_never_read_as_in_scope() {
        let (mut report, key) = pick_report("egs", "Catnip");
        in_scope(&mut report, &key, "umu-397540");
        // Never verified against the fix list: unchecked is not "runs fine",
        // and it is not "needs umu" either - nothing gets submitted on it.
        report.update(&key, |m| m.fix = None);
        let (rows, held) = partition(&report);
        assert!(rows.is_empty());
        assert!(held[0].1.contains("unchecked"), "{}", held[0].1);

        // A check against a different id (the user assigned a new one since)
        // says nothing about the id this row would carry.
        in_scope(&mut report, &key, "umu-397540");
        report.update(&key, |m| {
            m.fix.as_mut().expect("set above").umu_id = "umu-somethingelse".into();
        });
        let (rows, held) = partition(&report);
        assert!(rows.is_empty());
        assert!(held[0].1.contains("unchecked"), "{}", held[0].1);
    }

    #[test]
    fn an_entry_without_an_id_has_nothing_to_check_a_fix_against() {
        let (report, _) = pick_report("egs", "Catnip");
        let (rows, held) = partition(&report);
        assert!(rows.is_empty());
        assert!(
            held[0].1.contains("no id could be drafted"),
            "{}",
            held[0].1
        );
    }

    #[test]
    fn the_merge_request_names_the_fix_that_justifies_each_row() {
        let (mut report, key) = pick_report("egs", "Catnip");
        in_scope(&mut report, &key, "umu-397540");
        let dir = std::env::temp_dir().join(format!("gamebus-md-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("mr.md");
        export_markdown(&report, Some(&file)).expect("one in-scope row");
        let md = std::fs::read_to_string(&file).expect("written");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(md.contains("already has a protonfix"), "{md}");
        assert!(
            md.contains("fix: [`gamefixes-steam/397540.py`](https://github.com/"),
            "{md}"
        );
    }

    #[test]
    fn the_export_partition_uses_the_override_and_its_gog_gate_passes() {
        let (mut report, key) = pick_report("gog", "witchery");
        in_scope(&mut report, &key, "umu-397540");
        // Name-shaped launcher codename: held back by the gogdb rule…
        let (rows, held) = partition(&report);
        assert!(rows.is_empty(), "non-numeric gog codename reached a row");
        assert_eq!(held.len(), 1);
        // …until the override supplies the numeric product id.
        report.update(&key, |m| m.codename_override = Some("1423049311".into()));
        let (rows, held) = partition(&report);
        assert!(held.is_empty(), "{:?}", held.first().map(|(_, r)| r));
        assert_eq!(rows[0].codename, "1423049311");
    }

    #[test]
    fn a_title_override_is_confident_and_reaches_the_row() {
        let (mut report, key) = pick_report("egs", "Catnip");
        in_scope(&mut report, &key, "umu-397540");
        // Downgrade to the resolver's weakest word: held back…
        report.update(&key, |m| {
            m.title = Some("Spellcraft".into());
            m.confidence = Some(Confidence::Low);
        });
        let (rows, held) = partition(&report);
        assert!(rows.is_empty(), "a low-confidence title reached a row");
        assert_eq!(held[0].1, "unresolved or low-confidence title");
        // …until the user says what the game is. The typed title outranks
        // any resolver guess - it exports, under the user's spelling.
        report.update(&key, |m| m.title_override = Some("Project Hospital".into()));
        let (rows, held) = partition(&report);
        assert!(held.is_empty(), "{:?}", held.first().map(|(_, r)| r));
        assert_eq!(rows[0].title, "Project Hospital");
    }

    #[test]
    fn the_evidence_says_who_set_the_title() {
        let (mut report, key) = pick_report("egs", "Catnip");
        report.update(&key, |m| m.title = Some("Spellcraft".into()));
        let resolver = title_provenance(&report.entries()[&key]);
        assert_eq!(resolver, "title from heroic-config (high confidence)");
        // An override drops the source/confidence phrase for the honest one.
        report.update(&key, |m| m.title_override = Some("Project Hospital".into()));
        let overridden = title_provenance(&report.entries()[&key]);
        assert_eq!(overridden, "title set by you (resolver said 'Spellcraft')");
        // No resolution at all: nothing to attribute to the resolver.
        report.update(&key, |m| m.title = None);
        assert_eq!(
            title_provenance(&report.entries()[&key]),
            "title set by you"
        );
    }

    // ---- The candidacy gate (owner policy 2026-08-24): the exports take
    // umu candidates, not every umu miss.

    #[test]
    fn a_promoted_entry_exports_without_a_protonfix() {
        let (mut report, key) = pick_report("egs", "Catnip");
        in_scope(&mut report, &key, "umu-397540");
        report.update(&key, |m| {
            m.fix.as_mut().expect("set above").fixes.clear();
        });
        // No fix, no cross-store match, not promoted: held, and the reason
        // names the way in.
        let (rows, held) = partition(&report);
        assert!(rows.is_empty());
        assert!(held[0].1.contains("not promoted"), "{}", held[0].1);
        assert!(held[0].1.contains("u in the TUI"), "{}", held[0].1);
        // The promotion is the opt-in: the very same entry exports.
        report.update(&key, |m| m.umu_promoted = Some("2026-08-24".into()));
        let (rows, held) = partition(&report);
        assert!(held.is_empty(), "{:?}", held.first().map(|(_, r)| r));
        assert_eq!(rows.len(), 1);
        assert!(rows[0].fixes.is_empty());
    }

    #[test]
    fn a_cross_store_match_is_a_candidate_without_a_fix() {
        // The game already has an active umu entry via another store: this
        // store's copy is the gap, fix or no fix.
        let (mut report, key) = pick_report("egs", "Catnip");
        report.update(&key, |m| {
            m.verification = Some(crate::umu_report::Verification {
                state: VerificationState::CrossStoreId,
                umu_id: Some("umu-397540".into()),
                checked: "2026-08-24".into(),
                note: None,
            });
        });
        let (rows, held) = partition(&report);
        assert!(held.is_empty(), "{:?}", held.first().map(|(_, r)| r));
        assert_eq!(rows[0].umu_id, "umu-397540");
        assert!(rows[0].fixes.is_empty());
    }

    #[test]
    fn a_launcher_launch_never_reaches_the_umu_exports() {
        let mut report = UmuReport::default();
        report.note_launch("itchio", Some("926077"), "", "itchio:926077");
        report.note_title(
            "itchio",
            Some("926077"),
            "itchio:926077",
            "Danger Scavenger",
            "lutris-wrapper",
            Confidence::Medium,
            None,
        );
        // Not even a (stray) promotion smuggles it in: it never went
        // through umu, so there is no umu gap to fill.
        report.update("itchio:926077", |m| {
            m.umu_promoted = Some("2026-08-24".into())
        });
        let (rows, held) = partition(&report);
        assert!(rows.is_empty());
        assert!(held[0].1.contains("launcher launch"), "{}", held[0].1);
    }

    #[test]
    fn the_merge_request_says_why_a_fixless_candidate_is_there() {
        let (mut report, key) = pick_report("egs", "Catnip");
        in_scope(&mut report, &key, "umu-397540");
        report.update(&key, |m| {
            m.fix.as_mut().expect("set above").fixes.clear();
            m.umu_promoted = Some("2026-08-24".into());
        });
        let dir = std::env::temp_dir().join(format!("gamebus-md-promoted-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("mr.md");
        export_markdown(&report, Some(&file)).expect("one promoted row");
        let md = std::fs::read_to_string(&file).expect("written");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            md.contains("promoted for submission by the reviewing user 2026-08-24"),
            "{md}"
        );
        assert!(
            !md.contains("Each game below already has a protonfix"),
            "the intro claims a fix the row does not have:\n{md}"
        );
    }

    #[test]
    fn exe_basename_strips_both_separator_styles() {
        assert_eq!(
            exe_basename("/home/somebody/Games/Heroic/Control/Control_DX12.exe"),
            "Control_DX12.exe"
        );
        assert_eq!(
            exe_basename("Z:\\Spiele\\Control\\Control_DX12.exe"),
            "Control_DX12.exe"
        );
        assert_eq!(exe_basename("bare.exe"), "bare.exe");
    }
}
