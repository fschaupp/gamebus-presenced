//! The TUI flows for the misses pane: pick, assign, store, dismiss, verify.

use std::time::Duration;

#[cfg(test)]
use gamebus_coupler::{apply_miss, Refusal};
use gamebus_coupler::{pickable_stores, ErrorReason, MissChange, MissRow, MissVerb};
use serde_json::json;

use crate::setup::mcp::CallError;
use crate::setup::tui_client::with_session;
#[cfg(test)]
use crate::umu_report;
use crate::umu_report::{Miss, UmuDb, UmuEntry, UmuReport};

use super::verify::{fetch_full_dump, verify};
use super::{api_base, load_db, Opts};
use super::{EgsBuild, EgsOffer, GogProduct};

/// Everything the TUI misses pane's `v` key does: refresh the cached full
/// dump, then verify the stash against it (and the live API). Blocking  -
/// run it off the render path. Returns the log lines and whether the flow
/// completed.
pub(crate) fn tui_fetch_and_verify() -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    match fetch_full_dump(&api_base()) {
        Ok((_, n)) => lines.push(format!("Fetched the umu database: {n} entries.")),
        // Not fatal: verify still has the previous cache and the API.
        Err(e) => lines.push(format!(
            "Fetch failed ({e}) - verifying with what is available."
        )),
    }
    let mut report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        lines.push(e.to_string());
        return (lines, false);
    }
    if report.entries().is_empty() {
        lines.push("No identity misses recorded yet.".to_string());
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

/// The TUI's manual id assignment. The server validates the shape and
/// collision-checks it against the local database (no database, no
/// assignment) before it stores a [`DraftBasis::Manual`] draft.
///
/// [`DraftBasis::Manual`]: crate::umu_report::DraftBasis::Manual
pub(crate) fn tui_assign_id(key: &str, id: &str) -> (Vec<String>, bool) {
    assign_id(&mut McpEditor, key, id)
}

fn assign_id(editor: &mut impl Editor, key: &str, id: &str) -> (Vec<String>, bool) {
    let id = id.trim().to_lowercase();
    match editor.apply(key, &MissVerb::AssignId { id: id.clone() }) {
        Ok(done) => {
            let note = done.note.map(|n| format!(": {n}")).unwrap_or_default();
            (vec![format!("Assigned {id}{note}")], true)
        }
        Err(line) => (vec![line], false),
    }
}

/// What an applied edit came back with.
pub(crate) struct Applied {
    /// The entry's display title after the edit.
    title: String,
    change: MissChange,
    /// The engine's remark, when it had one (an assigned id's check).
    note: Option<String>,
}

/// Where the TUI's edits go. In the TUI it is the MCP session, the same
/// surface every client uses; the tests drive an in-memory stash that
/// applies the coupler's verbs directly, so the flows and their wording are
/// tested without a process.
pub(crate) trait Editor {
    fn get(&mut self, key: &str) -> Result<Miss, String>;
    fn apply(&mut self, key: &str, verb: &MissVerb) -> Result<Applied, String>;
}

/// The TUI's editor: `gamebus-setup mcp` as a child, see
/// [`crate::setup::tui_client`].
pub(crate) struct McpEditor;

impl Editor for McpEditor {
    fn get(&mut self, key: &str) -> Result<Miss, String> {
        let row =
            with_session(|s| s.read("get_miss", json!({"key": key}))).map_err(|e| cure(e, key))?;
        serde_json::from_value::<MissRow>(row)
            .map(|r| r.miss)
            .map_err(|e| format!("gamebus-setup mcp answered an unreadable miss: {e}"))
    }

    fn apply(&mut self, key: &str, verb: &MissVerb) -> Result<Applied, String> {
        let answer = with_session(|s| s.edit("apply_miss", json!({"key": key, "verb": verb})))
            .map_err(|e| cure(e, key))?;
        let change: MissChange = serde_json::from_value(answer["change"].clone())
            .map_err(|e| format!("gamebus-setup mcp answered an unreadable change: {e}"))?;
        let row: MissRow = serde_json::from_value(answer["row"].clone())
            .map_err(|e| format!("gamebus-setup mcp answered an unreadable miss: {e}"))?;
        Ok(Applied {
            title: row
                .miss
                .effective_title()
                .unwrap_or("(unresolved)")
                .to_string(),
            change,
            note: answer["note"].as_str().map(str::to_string),
        })
    }
}

/// A failed call as the line the TUI shows, with the cure a TUI user can
/// reach from here.
fn cure(e: CallError, key: &str) -> String {
    let CallError::Refused(refusal) = e else {
        return format!("gamebus-setup mcp: {e}");
    };
    match refusal.reason {
        ErrorReason::NotFound => format!("No stash entry under '{key}' anymore."),
        ErrorReason::AuthUninitialized => {
            "Editing needs auth first: press i on the auth tab to set it up.".into()
        }
        ErrorReason::ClientUnapproved => {
            "This TUI (gamebus-tui) is waiting for your approval: press a on the auth tab.".into()
        }
        ErrorReason::ClientKeyMismatch => {
            "This TUI's key is not the one on record: approve the new one on the auth tab.".into()
        }
        ErrorReason::LedgerMissing | ErrorReason::LedgerUnverified => format!(
            "{} Every edit is blocked until `gamebus-setup auth reset-ledger`.",
            refusal.message
        ),
        _ => refusal.message,
    }
}

#[cfg(test)]
impl Editor for UmuReport {
    fn get(&mut self, key: &str) -> Result<Miss, String> {
        self.entries()
            .get(key)
            .cloned()
            .ok_or_else(|| format!("No stash entry under '{key}' anymore."))
    }

    fn apply(&mut self, key: &str, verb: &MissVerb) -> Result<Applied, String> {
        let mut edited = self.get(key)?;
        let before = edited
            .effective_title()
            .unwrap_or("(unresolved)")
            .to_string();
        let change = apply_miss(&mut edited, verb, &umu_report::today()).map_err(|r| match r {
            Refusal::NotAUmuMiss => format!(
                "{before} never went through umu - a launcher launch has nothing to promote into the umu database."
            ),
            Refusal::EmptyTitle => "Empty title - nothing recorded.".to_string(),
            Refusal::EmptyId => "Empty id - nothing recorded.".to_string(),
            Refusal::UnknownTarget { target } => format!("Unknown target {target}."),
        })?;
        let title = edited
            .effective_title()
            .unwrap_or("(unresolved)")
            .to_string();
        self.update(key, |m| *m = edited);
        Ok(Applied {
            title,
            change,
            note: None,
        })
    }
}

/// One row of the TUI's pick list, tagged with what Enter on it means. The
/// kinds flow as one list through `Msg::UmuCandidates` and `ui::Pick`; the
/// keymap dispatches per kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickCandidate {
    /// A umu-database row - Enter records the id verdict on the miss.
    Db(UmuEntry),
    /// A Heroic library identity - Enter writes the store+codename
    /// overrides. NOT a verdict: the game may still be missing from the
    /// database; identity and verdict are different facts.
    Library(super::super::heroic_library::LibraryGame),
    /// A Lutris library identity (pga.db) - Enter writes the store+codename
    /// overrides, exactly like a Heroic library pick. NOT a verdict either.
    Lutris(super::super::lutris_library::LutrisGame),
    /// A GOG catalog hit (`o`) - Enter writes the product id as the
    /// codename override.
    GogProduct(GogProduct),
    /// The GOG product record for the miss's own numeric codename (`o` on
    /// such an entry) - Enter writes the TITLE override: the id was never
    /// in question there, the (possibly mis-resolved) title was.
    GogById {
        id: String,
        title: String,
        game_type: String,
    },
    /// An egdata offer hit (`o`) - Enter commits its last Windows build's
    /// App Name, or fires the builds request when the hit carries none. The
    /// namespace itself is NEVER offered as a codename: Control's namespace
    /// is lowercase `calluna`, its Builds App Name is `Calluna`.
    EgsOffer(EgsOffer),
    /// One build from the sandboxes list - Enter writes its App Name as the
    /// codename override.
    EgsBuild(EgsBuild),
}

impl PickCandidate {
    /// The pick list's section header - the candidates arrive grouped, and
    /// the label is what tells a database row from a library identity.
    /// Owned, not `&'static`: the by-id header names the product it hit.
    pub fn section_label(&self) -> String {
        match self {
            PickCandidate::Db(_) => "umu database - Enter records the verdict".into(),
            PickCandidate::Library(_) => "your Heroic library - Enter sets the identity".into(),
            PickCandidate::Lutris(_) => "your Lutris library - Enter sets the identity".into(),
            PickCandidate::GogProduct(_) => "GOG catalog - Enter sets the codename".into(),
            PickCandidate::GogById { id, .. } => {
                format!("GOG product {id} - Enter sets the title")
            }
            PickCandidate::EgsOffer(_) => "egdata offers - Enter picks the build".into(),
            PickCandidate::EgsBuild(_) => "egdata builds - Enter sets the codename".into(),
        }
    }
}

/// Candidates for the TUI's `p`: a title search against the local database
/// ([`UmuDb::search_title`] ranks and caps them) plus the user's Heroic
/// store_cache libraries and Lutris library - all local files, no network,
/// ever; `v` stays the only pane key that fetches. Blocking - run it off
/// the render path. The second value is a warning: an aging fetch cache, a
/// missing database when the libraries still produced something to pick,
/// or a Lutris library that exists but cannot be read.
pub(crate) fn tui_pick_candidates(
    title: &str,
) -> Result<(Vec<PickCandidate>, Option<String>), String> {
    let mut library: Vec<PickCandidate> = super::super::heroic_library::candidates(title)
        .into_iter()
        .map(PickCandidate::Library)
        .collect();
    // The Lutris library joins the pick on equal footing with Heroic's: the
    // same identity class, the same Enter. An unreadable pga.db is a
    // warning naming its path, never a failure - the database and Heroic
    // still pick without it.
    let mut lutris_warning = None;
    match super::super::lutris_library::load() {
        Ok(games) => library.extend(
            super::super::lutris_library::candidates(&games, title)
                .into_iter()
                .cloned()
                .map(PickCandidate::Lutris),
        ),
        Err(e) => lutris_warning = Some(format!("Lutris library unreadable ({e}).")),
    }
    let merged = |stale: Option<String>, lutris: Option<String>| -> Option<String> {
        let parts: Vec<String> = stale.into_iter().chain(lutris).collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    };
    match load_db(&Opts::none()) {
        Ok(Some(db)) => {
            let mut candidates: Vec<PickCandidate> = db
                .search_title(title)
                .into_iter()
                .cloned()
                .map(PickCandidate::Db)
                .collect();
            candidates.extend(library);
            Ok((candidates, merged(cache_staleness(), lutris_warning)))
        }
        Ok(None) if !library.is_empty() => Ok((
            library,
            merged(
                Some(
                    "No local umu database - candidates are your launcher libraries only; \
                     v fetches the database (net)."
                        .to_string(),
                ),
                lutris_warning,
            ),
        )),
        Ok(None) => {
            // Nothing to pick from at all; if the Lutris library could not
            // even be read, say so - it is why this list is emptier than
            // the machine's games deserve.
            let mut e =
                "No local database - press v to fetch, or set --db/GAMEBUS_UMU_DB.".to_string();
            if let Some(w) = lutris_warning {
                e.push(' ');
                e.push_str(&w);
            }
            Err(e)
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
        "Database cache is {} days old - v refreshes it (net).",
        age.as_secs() / 86_400
    ))
}

/// Only the fetch cache ages into a warning: an explicit `--db`/
/// `GAMEBUS_UMU_DB` checkout is the user's to keep fresh, and `v` would
/// not refresh it anyway - warning about it would point at the wrong fix.
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
/// entry" - recorded as the verification verdict, no network. The decision
/// itself lives in [`pick_entry`].
pub(crate) fn tui_pick_entry(
    key: &str,
    store: &str,
    codename: &str,
    umu_id: &str,
) -> (Vec<String>, bool) {
    pick_entry(&mut McpEditor, key, store, codename, umu_id)
}

/// Record a picked database entry on a miss. A pick whose store+codename
/// exactly equals the miss's own launch (case-insensitive) means the row
/// already exists - a launcher-side miss; anything else is the cross-store
/// verdict carrying the picked id. The picked id supersedes any drafted one.
fn pick_entry(
    editor: &mut impl Editor,
    key: &str,
    store: &str,
    codename: &str,
    umu_id: &str,
) -> (Vec<String>, bool) {
    let verb = MissVerb::PickEntry {
        store: store.to_string(),
        codename: codename.to_string(),
        umu_id: umu_id.to_string(),
    };
    match editor.apply(key, &verb) {
        Ok(Applied {
            title,
            change: MissChange::AlreadyInDatabase,
            ..
        }) => (
            vec![format!("{title}: already in the database as {umu_id} - the launcher missed, not the database.")],
            true,
        ),
        Ok(Applied { title, .. }) => (
            vec![format!("{title}: recorded {umu_id} from the database's {store}/{codename} entry.")],
            true,
        ),
        Err(line) => (vec![line], false),
    }
}

/// The TUI's identity write: a Heroic library pick or an online lookup
/// established what the game IS on its store.
pub(crate) fn tui_set_identity(
    key: &str,
    store: Option<&str>,
    codename: &str,
    source: &str,
) -> (Vec<String>, bool) {
    set_identity(&mut McpEditor, key, store, codename, source)
}

/// Record a store identity on a miss. Deliberately NOT a verification
/// verdict: knowing what the game is says nothing about whether the
/// database has it - a later `v` verifies with the new identity.
fn set_identity(
    editor: &mut impl Editor,
    key: &str,
    store: Option<&str>,
    codename: &str,
    source: &str,
) -> (Vec<String>, bool) {
    let verb = MissVerb::SetIdentity {
        store: store.map(str::to_string),
        codename: codename.to_string(),
    };
    let mut what = format!("codename {codename}");
    if let Some(s) = store {
        what.push_str(&format!(", store {s}"));
    }
    match editor.apply(key, &verb) {
        Ok(Applied { title, .. }) => (
            vec![format!(
                "{title}: {what} from {source} - v verifies with the new identity (net)."
            )],
            true,
        ),
        Err(line) => (vec![line], false),
    }
}

/// The TUI's title write: the user typed a title (`t`), or a GOG product
/// lookup answered with the store's own.
pub(crate) fn tui_set_title(key: &str, title: &str, source: &str) -> (Vec<String>, bool) {
    set_title(&mut McpEditor, key, title, source)
}

/// Record a title correction. Entering the daemon's own resolved title
/// clears the override instead of storing a copy. A title, never a verdict.
fn set_title(
    editor: &mut impl Editor,
    key: &str,
    title: &str,
    source: &str,
) -> (Vec<String>, bool) {
    let title = title.trim();
    let verb = MissVerb::SetTitle {
        title: title.to_string(),
    };
    let line = match editor.apply(key, &verb) {
        Ok(Applied {
            change: MissChange::TitleSet { resolved },
            ..
        }) => {
            let resolver_note = match resolved.as_deref() {
                Some(r) => format!("; resolver said '{r}'"),
                None => "; nothing had resolved one".to_string(),
            };
            format!("Title set to '{title}' (from {source}{resolver_note}).")
        }
        Ok(_) => format!("Title back to the resolver's own: {title}."),
        Err(line) => return (vec![line], false),
    };
    (vec![line], true)
}

/// The TUI's store correction: cycle the entry's effective store to the
/// next known id. The cycle is a keyboard affordance; what is sent is a
/// plain SetStore, and landing on the daemon's own guess clears the
/// override.
pub(crate) fn tui_cycle_store(key: &str) -> (Vec<String>, bool) {
    cycle_store(&mut McpEditor, key)
}

fn cycle_store(editor: &mut impl Editor, key: &str) -> (Vec<String>, bool) {
    let current = match editor.get(key) {
        Ok(m) => m.effective_store().to_string(),
        Err(line) => return (vec![line], false),
    };
    // The cycle offers what gamedb's stores.toml offers a picker, in its order.
    let stores: Vec<&str> = pickable_stores().collect();
    let idx = stores.iter().position(|s| *s == current);
    let next = stores[(idx.map_or(0, |i| i + 1)) % stores.len()].to_string();
    let line = match editor.apply(
        key,
        &MissVerb::SetStore {
            store: next.clone(),
        },
    ) {
        Ok(Applied {
            change: MissChange::StoreSet { guessed },
            ..
        }) => format!("Store set to {next} (daemon guessed {guessed}) - press v to re-verify."),
        Ok(_) => format!("Store back to the daemon's guess: {next}."),
        Err(line) => return (vec![line], false),
    };
    (vec![line], true)
}

/// The TUI's `d`: dismiss the selected entry, or restore it. Parked, not
/// deleted: the daemon's merge would resurrect a deleted key, and "not
/// wanted" is a judgment worth being able to reverse.
pub(crate) fn tui_toggle_dismiss(key: &str) -> (Vec<String>, bool) {
    toggle_dismiss(&mut McpEditor, key)
}

fn toggle_dismiss(editor: &mut impl Editor, key: &str) -> (Vec<String>, bool) {
    let verb = match editor.get(key) {
        Ok(m) if m.dismissed.is_some() => MissVerb::Undismiss,
        Ok(_) => MissVerb::Dismiss,
        Err(line) => return (vec![line], false),
    };
    let line = match editor.apply(key, &verb) {
        Ok(Applied {
            title,
            change: MissChange::Restored,
            ..
        }) => format!("{title} restored - back in the list and the exports."),
        Ok(Applied { title, .. }) => {
            format!("{title} dismissed - kept in the stash, out of the exports (d restores).")
        }
        Err(line) => return (vec![line], false),
    };
    (vec![line], true)
}

/// The TUI's `u`: promote the selected entry into the umu-database pipeline,
/// or take the promotion back.
pub(crate) fn tui_toggle_promote(key: &str) -> (Vec<String>, bool) {
    toggle_promote(&mut McpEditor, key)
}

/// Promotion is the opt-in of the owner policy: it makes the entry a umu
/// candidate even without a protonfix or cross-store match. A launcher
/// launch never went through umu, so it is refused with a line saying so.
fn toggle_promote(editor: &mut impl Editor, key: &str) -> (Vec<String>, bool) {
    let verb = match editor.get(key) {
        Ok(m) if m.umu_promoted.is_some() => MissVerb::Demote,
        Ok(_) => MissVerb::Promote,
        Err(line) => return (vec![line], false),
    };
    let line = match editor.apply(key, &verb) {
        Ok(Applied { title, change: MissChange::Demoted, .. }) => format!("{title} no longer promoted - a umu candidate only if a protonfix or cross-store match suggests it."),
        Ok(Applied { title, .. }) => format!("{title} promoted - in the umu exports even without a protonfix or cross-store match (u reverts)."),
        Err(line) => return (vec![line], false),
    };
    (vec![line], true)
}

#[cfg(test)]
mod tests {
    use super::super::{pick_db, pick_report};
    use super::*;
    use crate::umu_report::{DraftBasis, DraftedId, VerificationState};

    /// The `p` pick's local sources are the umu database, the Heroic store
    /// caches, and the Lutris library. Database rows come first; a Lutris
    /// row arrives as an identity candidate; and a pga.db that exists but
    /// cannot be read is a warning naming the path, not a failure - the
    /// database still picks.
    #[test]
    fn the_pick_joins_the_lutris_library_and_survives_an_unreadable_one() {
        let _env = crate::setup::lutris_library::ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("gamebus-pick-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");

        // A one-row umu database (a game the query must NOT match, so the
        // Lutris rows are the only hits) and a pga.db carrying Control
        // twice, on two stores - the choice the pick exists to offer.
        let db = dir.join("umu.csv");
        std::fs::write(
            &db,
            "TITLE,STORE,CODENAME,UMU_ID\nBorderlands 3,egs,Catnip,umu-397540\n",
        )
        .expect("scratch csv");
        let pga = dir.join("pga.db");
        {
            let conn = rusqlite::Connection::open(&pga).expect("scratch pga.db");
            conn.execute_batch(
                "create table games (id integer primary key, name text, slug text, \
                 runner text, service text, service_id text, directory text)",
            )
            .expect("create games table");
            for (service, service_id) in [("egs", "Calluna"), ("gog", "2049187585")] {
                conn.execute(
                    "insert into games (name, slug, runner, service, service_id, directory) \
                     values ('Control', 'control', 'wine', ?1, ?2, null)",
                    rusqlite::params![service, service_id],
                )
                .expect("insert row");
            }
        }

        std::env::set_var("GAMEBUS_UMU_DB", &db);
        std::env::set_var("GAMEBUS_LUTRIS_DB", &pga);
        let (candidates, warning) = tui_pick_candidates("control").expect("the pick resolves");
        // The explicit --db-style override never ages into a warning.
        assert_eq!(
            warning, None,
            "an explicit db and a healthy pga.db warn about nothing"
        );
        let lutris: Vec<&str> = candidates
            .iter()
            .filter_map(|c| match c {
                PickCandidate::Lutris(g) => Some(g.codename.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            lutris,
            vec!["Calluna", "2049187585"],
            "the library's editions, as identities"
        );
        assert!(
            candidates
                .iter()
                .all(|c| !matches!(c, PickCandidate::Db(_))),
            "the database row is for another game"
        );

        // An unreadable pga.db: the pick still answers, with a warning that
        // names the file instead of hiding the library's absence.
        let corrupt = dir.join("corrupt.db");
        std::fs::write(&corrupt, "this is not a sqlite database at all").unwrap();
        std::env::set_var("GAMEBUS_LUTRIS_DB", &corrupt);
        let (candidates, warning) =
            tui_pick_candidates("control").expect("the pick still resolves");
        assert!(
            candidates
                .iter()
                .all(|c| !matches!(c, PickCandidate::Lutris(_))),
            "a corrupt library contributed identities"
        );
        let warning = warning.expect("the corruption must be said, not hidden");
        assert!(warning.contains("corrupt.db"), "{warning}");

        std::env::remove_var("GAMEBUS_UMU_DB");
        std::env::remove_var("GAMEBUS_LUTRIS_DB");
    }

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
        // Case differs from the database row - the comparison must not care.
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

    // ---- Promotion (`u`): the opt-in of the owner policy.

    #[test]
    fn u_toggles_the_promotion_on_a_umu_miss() {
        let (mut report, key) = pick_report("egs", "Catnip");
        let (lines, ok) = toggle_promote(&mut report, &key);
        assert!(ok, "{lines:?}");
        assert!(
            report.entries()[&key].umu_promoted.is_some(),
            "promotion not recorded"
        );
        assert!(lines[0].contains("promoted"), "{lines:?}");
        // The same key takes it back.
        let (lines, ok) = toggle_promote(&mut report, &key);
        assert!(ok, "{lines:?}");
        assert!(
            report.entries()[&key].umu_promoted.is_none(),
            "promotion not cleared"
        );
        assert!(lines[0].contains("no longer promoted"), "{lines:?}");
    }

    #[test]
    fn u_refuses_a_launcher_launch_and_a_vanished_entry() {
        let (mut report, _) = pick_report("egs", "Catnip");
        report.note_launch("itchio", Some("926077"), "", "itchio:926077");
        let (lines, ok) = toggle_promote(&mut report, "itchio:926077");
        assert!(!ok);
        assert!(lines[0].contains("never went through umu"), "{lines:?}");
        assert!(
            report.entries()["itchio:926077"].umu_promoted.is_none(),
            "a launcher launch got promoted"
        );

        let (lines, ok) = toggle_promote(&mut report, "gone:key");
        assert!(!ok);
        assert!(lines[0].contains("gone:key"), "{lines:?}");
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
