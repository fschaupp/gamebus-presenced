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
    /// store `none` with codename `none` — a codename without a store
    /// namespace is meaningless upstream, so it moves to the NOTE instead.
    codename: String,
    /// True when the id provably honors the "Steam appid when on Steam"
    /// rule: verified cross-store from the database, or drafted from a
    /// detectable.json Steam sku. Slug/codename drafts only prove Discord's
    /// file had no entry — NOT that the game is absent from Steam.
    steam_rule_certain: bool,
}

/// Split the stash into rows worth submitting and entries listed after the
/// block with the reason they were held back.
fn partition(report: &UmuReport) -> (Vec<SubmissionRow<'_>>, Vec<(&Miss, String)>) {
    let mut rows = Vec::new();
    let mut held = Vec::new();
    let mut entries: Vec<&Miss> = report.entries().values().collect();
    entries.sort_by(|a, b| a.last_seen.cmp(&b.last_seen).reverse());

    for m in entries {
        if m.dismissed.is_some() {
            held.push((m, "dismissed by you (d in the TUI restores it)".to_string()));
            continue;
        }
        if let Some(v) = &m.verification {
            if v.state == VerificationState::AlreadyInDatabase {
                held.push((
                    m,
                    format!(
                        "already in the database as {} — a launcher-side miss, not a gap",
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
        let confident = matches!(
            m.confidence,
            Some(Confidence::High) | Some(Confidence::Medium)
        );
        let Some(title) = m.title.as_deref().filter(|_| confident) else {
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
        });
    }
    (rows, held)
}

/// The submission wants executable names, not this machine's absolute
/// paths — those embed `/home/<user>` and whatever else the install layout
/// leaks. Basename only, either separator (Wine paths carry backslashes).
fn exe_basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn csv_line(row: &SubmissionRow<'_>) -> String {
    let m = row.miss;
    // The NOTE column is the database's, not ours: it carries game-related
    // remarks only (which of two standalone versions a row means, per the
    // README's Genshin example). Provenance — who resolved the title, how
    // the id was drafted — belongs in the merge request's evidence text,
    // never in the submitted CSV. The column stays empty.
    format!(
        "{},{},{},{},,,{}",
        csv_field(row.title),
        m.effective_store().to_lowercase(),
        // Codename comes verbatim from an untrusted process's environment
        // (HEROIC_APP_NAME) — escaped like every other external field, or a
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
    println!("# umu-database submission draft — review before submitting!");
    println!("# Rules: {}#readme", endpoints().umu_repository);
    println!("# umu-FIXME means no id could be drafted safely — resolve by hand.");
    println!("{CSV_HEADER}");
    for row in &rows {
        println!("{}", csv_line(row));
    }
    for (m, reason) in &held {
        eprintln!(
            "(held back: {} — {reason})",
            m.title.as_deref().unwrap_or("(unresolved)")
        );
    }
}

/// `--export-md`: the slim merge request — title line, one honest paragraph,
/// the CSV in a fenced block, per-entry evidence, and the README's own rules
/// as a checklist.
pub(super) fn export_markdown(
    report: &UmuReport,
    dest: Option<&std::path::Path>,
) -> Result<(), String> {
    let (rows, held) = partition(report);
    if rows.is_empty() {
        return Err("Nothing to export: no verified, submission-ready entries. Run --verify first, or check the held-back reasons in --export.".into());
    }
    let titles: Vec<&str> = rows.iter().map(|r| r.title).collect();
    let all_checked = rows.iter().all(|r| r.umu_id != "umu-FIXME");
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
    md.push_str(&format!(
        "These games launch through umu with `GAMEID=umu-0` (no database entry). \
         [gamebus-presenced]({}) resolved their titles on a live system from the \
         launchers' own install records; per-entry evidence below.\n\n",
        "https://github.com/fschaupp/gamebus-presenced"
    ));
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
        md.push_str(&format!(
            "- **{}** — store `{store}`, codename `{}`{codename_ref}{}; title from {} ({} confidence); {}{advisory}.\n",
            row.title,
            row.codename,
            m.executable
                .as_deref()
                .map(|e| format!(", exe `{}`", exe_basename(e)))
                .unwrap_or_default(),
            m.title_source.as_deref().unwrap_or("unknown"),
            confidence_label(m.confidence),
            row.id_provenance
                .as_deref()
                .unwrap_or("id left as umu-FIXME — needs a human"),
        ));
    }
    md.push_str("\n## Checklist\n\n");
    md.push_str("- [ ] Titles match the store's spelling and capitalization\n");
    md.push_str("- [x] Store ids are lowercase\n");
    md.push_str(&format!(
        "- [{}] Every id follows the database rules (Steam appid when the game is on Steam)\n",
        if steam_rule { 'x' } else { ' ' }
    ));
    md.push_str(&format!(
        "- [{}] Drafted ids collision-checked against the full database\n",
        if all_checked { 'x' } else { ' ' }
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
    md.push_str(
        "- [x] NOTE column carries game-related remarks only (provenance stays in this text)\n",
    );
    if !held.is_empty() {
        md.push_str("\n<!-- Held back, not part of this submission:\n");
        for (m, reason) in &held {
            md.push_str(&format!(
                "  {} — {reason}\n",
                m.title.as_deref().unwrap_or("(unresolved)")
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

    #[test]
    fn the_export_partition_uses_the_override_and_its_gog_gate_passes() {
        let (mut report, key) = pick_report("gog", "witchery");
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
