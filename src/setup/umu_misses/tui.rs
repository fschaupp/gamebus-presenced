//! The TUI flows for the misses pane: pick, assign, store, dismiss, verify.

use std::time::Duration;

use crate::umu_report::{
    self, DraftBasis, DraftedId, UmuDb, UmuEntry, UmuReport, Verification, VerificationState,
};

use super::verify::{check_assignment, fetch_full_dump, verify};
use super::{api_base, load_db, Opts};
use super::{EgsBuild, EgsOffer, GogProduct};

/// Everything the TUI misses pane's `v` key does: refresh the cached full
/// dump, then verify the stash against it (and the live API). Blocking —
/// run it off the render path. Returns the log lines and whether the flow
/// completed.
pub(crate) fn tui_fetch_and_verify() -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    match fetch_full_dump(&api_base()) {
        Ok((_, n)) => lines.push(format!("Fetched the umu database: {n} entries.")),
        // Not fatal: verify still has the previous cache and the API.
        Err(e) => lines.push(format!(
            "Fetch failed ({e}) — verifying with what is available."
        )),
    }
    let mut report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        lines.push(e.to_string());
        return (lines, false);
    }
    if report.entries().is_empty() {
        lines.push("No umu-database misses recorded yet.".to_string());
        return (lines, true);
    }
    let db = match load_db(&Opts::none()) {
        Ok(db) => db,
        Err(e) => {
            lines.push(e);
            return (lines, false);
        }
    };
    match verify(&mut report, db.as_ref()) {
        Ok(mut vlines) => {
            lines.append(&mut vlines);
            lines.push(
                "Export a submission: gamebus-setup umu-misses --export-md [file] (stdout without a file)".to_string(),
            );
            (lines, true)
        }
        Err(e) => {
            lines.push(format!("Verify failed: {e}"));
            (lines, false)
        }
    }
}

/// The TUI's manual id assignment: validate the shape, collision-check
/// against the local database (mandatory — no database, no assignment),
/// then store it as a [`DraftBasis::Manual`] draft on the entry.
pub(crate) fn tui_assign_id(key: &str, id: &str) -> (Vec<String>, bool) {
    let id = id.trim().to_lowercase();
    let mut report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        return (vec![e.to_string()], false);
    }
    let Some(title) = report
        .entries()
        .get(key)
        .map(|m| m.effective_title().map(str::to_string))
    else {
        return (
            vec![format!("No stash entry under '{key}' anymore.")],
            false,
        );
    };
    let db = match load_db(&Opts::none()) {
        Ok(Some(db)) => db,
        Ok(None) => {
            return (
                vec![
                    "No local database to collision-check against — press v to fetch it first."
                        .to_string(),
                ],
                false,
            );
        }
        Err(e) => return (vec![e], false),
    };
    let note = match check_assignment(&db, title.as_deref(), &id) {
        Ok(note) => note,
        Err(e) => return (vec![e], false),
    };
    report.update(key, |m| {
        m.drafted_id = Some(DraftedId {
            id: id.clone(),
            basis: DraftBasis::Manual,
            collision_checked: umu_report::today(),
        });
    });
    report.save();
    (vec![format!("Assigned {id}: {note}")], true)
}

/// One row of the TUI's pick list, tagged with what Enter on it means. The
/// kinds flow as one list through `Msg::UmuCandidates` and `ui::Pick`; the
/// keymap dispatches per kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickCandidate {
    /// A umu-database row — Enter records the id verdict on the miss.
    Db(UmuEntry),
    /// A Heroic library identity — Enter writes the store+codename
    /// overrides. NOT a verdict: the game may still be missing from the
    /// database; identity and verdict are different facts.
    Library(super::super::heroic_library::LibraryGame),
    /// A GOG catalog hit (`o`) — Enter writes the product id as the
    /// codename override.
    GogProduct(GogProduct),
    /// The GOG product record for the miss's own numeric codename (`o` on
    /// such an entry) — Enter writes the TITLE override: the id was never
    /// in question there, the (possibly mis-resolved) title was.
    GogById {
        id: String,
        title: String,
        game_type: String,
    },
    /// An egdata offer hit (`o`) — Enter commits its last Windows build's
    /// App Name, or fires the builds request when the hit carries none. The
    /// namespace itself is NEVER offered as a codename: Control's namespace
    /// is lowercase `calluna`, its Builds App Name is `Calluna`.
    EgsOffer(EgsOffer),
    /// One build from the sandboxes list — Enter writes its App Name as the
    /// codename override.
    EgsBuild(EgsBuild),
}

impl PickCandidate {
    /// The pick list's section header — the candidates arrive grouped, and
    /// the label is what tells a database row from a library identity.
    /// Owned, not `&'static`: the by-id header names the product it hit.
    pub fn section_label(&self) -> String {
        match self {
            PickCandidate::Db(_) => "umu database — Enter records the verdict".into(),
            PickCandidate::Library(_) => "your Heroic library — Enter sets the identity".into(),
            PickCandidate::GogProduct(_) => "GOG catalog — Enter sets the codename".into(),
            PickCandidate::GogById { id, .. } => {
                format!("GOG product {id} — Enter sets the title")
            }
            PickCandidate::EgsOffer(_) => "egdata offers — Enter picks the build".into(),
            PickCandidate::EgsBuild(_) => "egdata builds — Enter sets the codename".into(),
        }
    }
}

/// Candidates for the TUI's `p`: a title search against the local database
/// ([`UmuDb::search_title`] ranks and caps them) plus the user's Heroic
/// store_cache libraries — both local files, no network, ever; `v` stays
/// the only pane key that fetches. Blocking — run it off the render path.
/// The second value is a warning: an aging fetch cache, or a missing
/// database when the libraries still produced something to pick.
pub(crate) fn tui_pick_candidates(
    title: &str,
) -> Result<(Vec<PickCandidate>, Option<String>), String> {
    let library: Vec<PickCandidate> = super::super::heroic_library::candidates(title)
        .into_iter()
        .map(PickCandidate::Library)
        .collect();
    match load_db(&Opts::none()) {
        Ok(Some(db)) => {
            let mut candidates: Vec<PickCandidate> = db
                .search_title(title)
                .into_iter()
                .cloned()
                .map(PickCandidate::Db)
                .collect();
            candidates.extend(library);
            Ok((candidates, cache_staleness()))
        }
        Ok(None) if !library.is_empty() => Ok((
            library,
            Some(
                "No local umu database — candidates are your Heroic library only; \
                 v fetches the database (net)."
                    .to_string(),
            ),
        )),
        Ok(None) => {
            Err("No local database — press v to fetch, or set --db/GAMEBUS_UMU_DB.".to_string())
        }
        Err(e) => Err(e),
    }
}

/// Entries land upstream continuously; a cache this old silently misses
/// the newest ones, and a pick that cannot find a game reads as "not in
/// the database" when the truth is "not in YOUR COPY of the database".
const STALE_AFTER: Duration = Duration::from_secs(7 * 86_400);

fn staleness_note(age: Duration) -> Option<String> {
    if age < STALE_AFTER {
        return None;
    }
    Some(format!(
        "Database cache is {} days old — v refreshes it (net).",
        age.as_secs() / 86_400
    ))
}

/// Only the fetch cache ages into a warning: an explicit `--db`/
/// `GAMEBUS_UMU_DB` checkout is the user's to keep fresh, and `v` would
/// not refresh it anyway — warning about it would point at the wrong fix.
fn cache_staleness() -> Option<String> {
    if std::env::var_os("GAMEBUS_UMU_DB").is_some() {
        return None;
    }
    let path = UmuDb::cache_path()?;
    let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
    let age = std::time::SystemTime::now().duration_since(mtime).ok()?;
    staleness_note(age)
}

/// The TUI's pick: the user chose a database entry as "this game IS that
/// entry" — recorded as the verification verdict, no network. Same shape as
/// [`tui_assign_id`]; the decision itself lives in [`pick_entry`].
pub(crate) fn tui_pick_entry(
    key: &str,
    store: &str,
    codename: &str,
    umu_id: &str,
) -> (Vec<String>, bool) {
    let mut report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        return (vec![e.to_string()], false);
    }
    let (lines, ok) = pick_entry(&mut report, key, store, codename, umu_id);
    if ok {
        report.save();
    }
    (lines, ok)
}

/// Record a picked database entry on a miss. A pick whose store+codename
/// exactly equals the miss's own launch (case-insensitive) means the row
/// already exists — a launcher-side miss, [`AlreadyInDatabase`]; anything
/// else is the cross-store verdict carrying the picked id. The picked id
/// supersedes any drafted one, so the draft is cleared either way.
///
/// [`AlreadyInDatabase`]: VerificationState::AlreadyInDatabase
fn pick_entry(
    report: &mut UmuReport,
    key: &str,
    store: &str,
    codename: &str,
    umu_id: &str,
) -> (Vec<String>, bool) {
    let Some(m) = report.entries().get(key) else {
        return (
            vec![format!("No stash entry under '{key}' anymore.")],
            false,
        );
    };
    let title = m.effective_title().unwrap_or("(unresolved)").to_string();
    let same_row = m.effective_store().eq_ignore_ascii_case(store)
        && m.effective_codename()
            .is_some_and(|c| c.eq_ignore_ascii_case(codename));
    let (state, line) = if same_row {
        (
            VerificationState::AlreadyInDatabase,
            format!("{title}: already in the database as {umu_id} — the launcher missed, not the database."),
        )
    } else {
        (
            VerificationState::CrossStoreId,
            format!("{title}: recorded {umu_id} from the database's {store}/{codename} entry."),
        )
    };
    report.update(key, |m| {
        m.verification = Some(Verification {
            state,
            umu_id: Some(umu_id.to_string()),
            checked: umu_report::today(),
            note: Some(format!(
                "picked from the database's {store}/{codename} entry"
            )),
        });
        m.drafted_id = None;
    });
    (vec![line], true)
}

/// The TUI's identity write: a Heroic library pick or an online lookup
/// established what the game IS on its store. Same shape as
/// [`tui_pick_entry`]; the decision lives in [`set_identity`].
pub(crate) fn tui_set_identity(
    key: &str,
    store: Option<&str>,
    codename: &str,
    source: &str,
) -> (Vec<String>, bool) {
    let mut report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        return (vec![e.to_string()], false);
    }
    let (lines, ok) = set_identity(&mut report, key, store, codename, source);
    if ok {
        report.save();
    }
    (lines, ok)
}

/// Record a store identity on a miss: `codename_override`, plus
/// `store_override` when the pick names a store (a library pick does; an
/// online lookup already ran under the miss's effective store). Both are
/// annotation-half, so a daemon write never reverts them. Deliberately NOT
/// a verification verdict: knowing what the game is says nothing about
/// whether the database has it — a later `v` verifies with the new
/// identity.
fn set_identity(
    report: &mut UmuReport,
    key: &str,
    store: Option<&str>,
    codename: &str,
    source: &str,
) -> (Vec<String>, bool) {
    let Some(m) = report.entries().get(key) else {
        return (
            vec![format!("No stash entry under '{key}' anymore.")],
            false,
        );
    };
    let title = m.effective_title().unwrap_or("(unresolved)").to_string();
    let guessed = m.store.clone();
    let store = store.map(str::to_string);
    let mut what = format!("codename {codename}");
    if let Some(s) = &store {
        what.push_str(&format!(", store {s}"));
    }
    report.update(key, |m| {
        m.codename_override = Some(codename.to_string());
        if let Some(s) = &store {
            // Mirrors the s-cycle: landing on the daemon's own guess means
            // the entry is back to "guessed", not "corrected to the guess".
            m.store_override = (*s != guessed).then(|| s.clone());
        }
    });
    (
        vec![format!(
            "{title}: {what} from {source} — v verifies with the new identity (net)."
        )],
        true,
    )
}

/// The TUI's title write: the user typed a title (`t`), or a GOG product
/// lookup answered with the store's own. Same shape as [`tui_set_identity`];
/// the decision lives in [`set_title`].
pub(crate) fn tui_set_title(key: &str, title: &str, source: &str) -> (Vec<String>, bool) {
    let mut report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        return (vec![e.to_string()], false);
    }
    let (lines, ok) = set_title(&mut report, key, title, source);
    if ok {
        report.save();
    }
    (lines, ok)
}

/// Record a title correction on a miss: `title_override`, annotation-half
/// like the other overrides, so a daemon write never reverts it. Entering
/// the daemon's own resolved title clears the override instead of storing a
/// copy — mirrors the s-cycle: back to "resolved", not "corrected to the
/// resolution". A title, never a verdict; `v` re-verifies with it.
fn set_title(report: &mut UmuReport, key: &str, title: &str, source: &str) -> (Vec<String>, bool) {
    let title = title.trim();
    if title.is_empty() {
        return (vec!["Empty title — nothing recorded.".to_string()], false);
    }
    let Some(m) = report.entries().get(key) else {
        return (
            vec![format!("No stash entry under '{key}' anymore.")],
            false,
        );
    };
    let resolved = m.title.clone();
    let line = if resolved.as_deref() == Some(title) {
        report.update(key, |m| m.title_override = None);
        format!("Title back to the resolver's own: {title}.")
    } else {
        report.update(key, |m| m.title_override = Some(title.to_string()));
        let resolver_note = match resolved.as_deref() {
            Some(r) => format!("; resolver said '{r}'"),
            None => "; nothing had resolved one".to_string(),
        };
        format!("Title set to '{title}' (from {source}{resolver_note}).")
    };
    (vec![line], true)
}

/// Every store id the database actually uses (counted from the upstream
/// CSV, 2026-08-07), most common first — the TUI's `s` key cycles these.
/// The tail holds the ids a launcher can hand us but the CSV rarely
/// carries, in umu's own order; `none` stays last, as the way out.
pub(crate) const KNOWN_STORES: &[&str] = &[
    "egs",
    "gog",
    "amazon",
    "ubisoft",
    "humble",
    "ea",
    "zoomplatform",
    "steam",
    "itchio",
    "battlenet",
    "none",
];

/// The TUI's store correction: cycle the selected entry's effective store
/// to the next known id. Cycling onto the daemon's own guess clears the
/// override (the entry is back to "guessed"). A corrected store is what
/// makes the next verify's store+codename lookup able to hit.
pub(crate) fn tui_cycle_store(key: &str) -> (Vec<String>, bool) {
    let mut report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        return (vec![e.to_string()], false);
    }
    let Some(m) = report.entries().get(key) else {
        return (
            vec![format!("No stash entry under '{key}' anymore.")],
            false,
        );
    };
    let current = m.effective_store().to_string();
    let guessed = m.store.clone();
    let idx = KNOWN_STORES.iter().position(|s| *s == current);
    let next = KNOWN_STORES[(idx.map_or(0, |i| i + 1)) % KNOWN_STORES.len()].to_string();
    let line = if next == guessed {
        report.update(key, |m| m.store_override = None);
        format!("Store back to the daemon's guess: {guessed}.")
    } else {
        report.update(key, |m| m.store_override = Some(next.clone()));
        format!("Store set to {next} (daemon guessed {guessed}) — press v to re-verify.")
    };
    report.save();
    (vec![line], true)
}

/// The TUI's `d`: dismiss the selected entry, or restore it. A dismissed
/// entry is parked (bottom of the list, greyed, out of every export), not
/// deleted — a deleted key would be resurrected by the daemon's merge and
/// re-recorded on the next launch anyway, and "not wanted" is a judgment
/// worth being able to reverse.
pub(crate) fn tui_toggle_dismiss(key: &str) -> (Vec<String>, bool) {
    let mut report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        return (vec![e.to_string()], false);
    }
    let Some(m) = report.entries().get(key) else {
        return (
            vec![format!("No stash entry under '{key}' anymore.")],
            false,
        );
    };
    let title = m.effective_title().unwrap_or("(unresolved)").to_string();
    let line = if m.dismissed.is_some() {
        report.update(key, |m| m.dismissed = None);
        format!("{title} restored — back in the list and the exports.")
    } else {
        report.update(key, |m| m.dismissed = Some(umu_report::today()));
        format!("{title} dismissed — kept in the stash, out of the exports (d restores).")
    };
    report.save();
    (vec![line], true)
}

#[cfg(test)]
mod tests {
    use super::super::{pick_db, pick_report};
    use super::*;

    #[test]
    fn a_week_old_cache_warns_a_fresher_one_does_not() {
        assert_eq!(staleness_note(Duration::from_secs(6 * 86_400)), None);
        let note = staleness_note(Duration::from_secs(9 * 86_400)).expect("9 days is stale");
        assert!(note.contains("9 days old"), "{note}");
        assert!(note.contains("(net)"), "the fix must name its cost: {note}");
    }

    #[test]
    fn picking_the_misss_own_row_is_a_launcher_side_miss() {
        let db = pick_db();
        // Case differs from the database row — the comparison must not care.
        let (mut report, key) = pick_report("EGS", "catnip");
        let cand = db.search_title("Borderlands 3")[0].clone();
        let (lines, ok) = pick_entry(&mut report, &key, &cand.store, &cand.codename, &cand.umu_id);
        assert!(ok, "{lines:?}");
        let v = report.entries()[&key].verification.as_ref().unwrap();
        assert_eq!(v.state, VerificationState::AlreadyInDatabase);
        assert_eq!(v.umu_id.as_deref(), Some("umu-397540"));
        assert!(lines[0].contains("launcher missed"), "{lines:?}");
    }

    #[test]
    fn picking_another_stores_row_is_the_cross_store_verdict() {
        let db = pick_db();
        let (mut report, key) = pick_report("egs", "Catnip");
        // A stale draft that the pick must supersede.
        report.update(&key, |m| {
            m.drafted_id = Some(DraftedId {
                id: "umu-borderlands3".into(),
                basis: DraftBasis::Manual,
                collision_checked: "2026-08-08".into(),
            });
        });
        let cand = db
            .search_title("Borderlands 3")
            .into_iter()
            .find(|e| e.store == "gog")
            .unwrap()
            .clone();
        let (lines, ok) = pick_entry(&mut report, &key, &cand.store, &cand.codename, &cand.umu_id);
        assert!(ok, "{lines:?}");
        let m = &report.entries()[&key];
        let v = m.verification.as_ref().unwrap();
        assert_eq!(v.state, VerificationState::CrossStoreId);
        assert_eq!(v.umu_id.as_deref(), Some("umu-397540"));
        assert!(
            v.note.as_deref().unwrap().contains("gog/1454587428"),
            "{:?}",
            v.note
        );
        assert!(m.drafted_id.is_none(), "the pick left the stale draft");
    }

    #[test]
    fn a_store_override_counts_as_the_misss_own_store() {
        let db = pick_db();
        let (mut report, key) = pick_report("none", "Catnip");
        report.update(&key, |m| m.store_override = Some("egs".into()));
        let cand = db.search_title("Borderlands 3")[0].clone();
        let (_, ok) = pick_entry(&mut report, &key, &cand.store, &cand.codename, &cand.umu_id);
        assert!(ok);
        assert_eq!(
            report.entries()[&key].verification.as_ref().unwrap().state,
            VerificationState::AlreadyInDatabase
        );
    }

    #[test]
    fn picking_for_a_vanished_entry_fails_honestly() {
        let (mut report, _) = pick_report("egs", "Catnip");
        let (lines, ok) = pick_entry(&mut report, "gone:key", "egs", "Catnip", "umu-397540");
        assert!(!ok);
        assert!(lines[0].contains("gone:key"), "{lines:?}");
    }

    #[test]
    fn a_codename_override_counts_as_the_misss_own_row_in_a_pick() {
        let db = pick_db();
        let (mut report, key) = pick_report("egs", "WrongName");
        report.update(&key, |m| m.codename_override = Some("Catnip".into()));
        let cand = db.search_title("Borderlands 3")[0].clone();
        let (_, ok) = pick_entry(&mut report, &key, &cand.store, &cand.codename, &cand.umu_id);
        assert!(ok);
        assert_eq!(
            report.entries()[&key].verification.as_ref().unwrap().state,
            VerificationState::AlreadyInDatabase
        );
    }

    // ---- Identity writes (library picks and online lookups).

    #[test]
    fn a_library_pick_writes_the_identity_but_never_a_verdict() {
        let (mut report, key) = pick_report("none", "somewrapper");
        let (lines, ok) = set_identity(
            &mut report,
            &key,
            Some("egs"),
            "Calluna",
            "your Heroic library",
        );
        assert!(ok, "{lines:?}");
        let m = &report.entries()[&key];
        assert_eq!(m.codename_override.as_deref(), Some("Calluna"));
        assert_eq!(m.store_override.as_deref(), Some("egs"));
        assert!(
            m.verification.is_none(),
            "an identity pick minted a verdict"
        );
        assert!(lines[0].contains("codename Calluna"), "{lines:?}");
        assert!(lines[0].contains("your Heroic library"), "{lines:?}");
        assert!(lines[0].contains("store egs"), "{lines:?}");
    }

    #[test]
    fn an_online_pick_sets_the_codename_and_leaves_the_store_alone() {
        let (mut report, key) = pick_report("gog", "witchery");
        report.update(&key, |m| m.store_override = Some("gog".into()));
        let (lines, ok) = set_identity(&mut report, &key, None, "2049187585", "the GOG catalog");
        assert!(ok, "{lines:?}");
        let m = &report.entries()[&key];
        assert_eq!(m.codename_override.as_deref(), Some("2049187585"));
        assert_eq!(m.store_override.as_deref(), Some("gog"), "store touched");
        assert!(
            lines[0].contains("codename 2049187585 from the GOG catalog"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_library_pick_matching_the_daemons_guess_clears_the_store_override() {
        let (mut report, key) = pick_report("egs", "WrongName");
        report.update(&key, |m| m.store_override = Some("gog".into()));
        let (_, ok) = set_identity(
            &mut report,
            &key,
            Some("egs"),
            "Calluna",
            "your Heroic library",
        );
        assert!(ok);
        // Back on the guess: "guessed", not "corrected to the guess".
        assert!(report.entries()[&key].store_override.is_none());
    }

    // ---- Title writes (`t`, and the GOG by-id lookup).

    #[test]
    fn a_typed_title_writes_the_override_and_names_its_source() {
        let (mut report, key) = pick_report("gog", "1660194629");
        // The Spellcraft incident: the resolver's title is wrong.
        report.update(&key, |m| m.title = Some("Spellcraft".into()));
        let (lines, ok) = set_title(&mut report, &key, " Project Hospital ", "you");
        assert!(ok, "{lines:?}");
        let m = &report.entries()[&key];
        assert_eq!(m.title_override.as_deref(), Some("Project Hospital"));
        assert_eq!(m.title.as_deref(), Some("Spellcraft"), "resolution touched");
        assert_eq!(m.effective_title(), Some("Project Hospital"));
        assert!(lines[0].contains("from you"), "{lines:?}");
        assert!(lines[0].contains("resolver said 'Spellcraft'"), "{lines:?}");
    }

    #[test]
    fn entering_the_resolvers_own_title_clears_the_override() {
        let (mut report, key) = pick_report("egs", "Catnip");
        report.update(&key, |m| m.title_override = Some("Wrong Correction".into()));
        let (lines, ok) = set_title(&mut report, &key, "Borderlands 3", "you");
        assert!(ok, "{lines:?}");
        // Back to "resolved", not "corrected to the resolution".
        assert!(report.entries()[&key].title_override.is_none());
        assert!(lines[0].contains("back to the resolver's own"), "{lines:?}");
    }

    #[test]
    fn an_empty_title_is_refused_and_a_titleless_miss_still_takes_one() {
        let (mut report, key) = pick_report("egs", "Catnip");
        let (lines, ok) = set_title(&mut report, &key, "   ", "you");
        assert!(!ok);
        assert!(lines[0].contains("Empty title"), "{lines:?}");
        assert!(report.entries()[&key].title_override.is_none());
        // Nothing ever resolved: the override still lands, honestly labelled.
        report.update(&key, |m| m.title = None);
        let (lines, ok) = set_title(
            &mut report,
            &key,
            "Project Hospital",
            "GOG product 1660194629",
        );
        assert!(ok, "{lines:?}");
        assert!(lines[0].contains("nothing had resolved one"), "{lines:?}");
        assert!(lines[0].contains("GOG product 1660194629"), "{lines:?}");
        assert_eq!(
            report.entries()[&key].effective_title(),
            Some("Project Hospital")
        );
    }

    #[test]
    fn setting_a_title_for_a_vanished_entry_fails_honestly() {
        let (mut report, _) = pick_report("egs", "Catnip");
        let (lines, ok) = set_title(&mut report, "gone:key", "X", "you");
        assert!(!ok);
        assert!(lines[0].contains("gone:key"), "{lines:?}");
    }

    #[test]
    fn setting_an_identity_for_a_vanished_entry_fails_honestly() {
        let (mut report, _) = pick_report("egs", "Catnip");
        let (lines, ok) = set_identity(&mut report, "gone:key", None, "X", "somewhere");
        assert!(!ok);
        assert!(lines[0].contains("gone:key"), "{lines:?}");
    }
}
