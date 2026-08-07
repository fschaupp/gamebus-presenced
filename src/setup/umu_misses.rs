//! Review, verify, draft, and export the umu-database miss stash.
//!
//! The daemon (network-free, always) writes the stash's resolution half;
//! this module writes the annotation half (verification, drafted ids, PR
//! marks - merged, never clobbered: see `UmuReport`) and talks to the world
//! on the user's explicit command:
//!
//! - default: the review list.
//! - `--verify`: check every miss against the database - local copy first
//!   (`--db`, `GAMEBUS_UMU_DB`, or the cached full dump), then the public
//!   API for whatever the local copy did not settle. Entries confirmed
//!   missing get a umu id drafted per the database's own rules, and every
//!   draft is collision-checked - no exceptions.
//! - `--fetch`: refresh the cached full dump. One request, only when asked.
//! - `--export`: submission-shaped CSV on stdout.
//! - `--export-md [file]`: a slim, ready-to-paste merge-request text -
//!   written to the file, or to stdout when none is given.
//! - `--check-prs`: best-effort scan of open upstream merge requests for
//!   entries someone already submitted. Opt-in, non-fatal, clearly labelled.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use serde::Deserialize;

use crate::naming::NamingDb;
use crate::umu_report::{
    self, draft_umu_id, Confidence, DraftBasis, DraftOutcome, DraftedId, Miss, UmuDb, UmuReport,
    Verification, VerificationState,
};

use crate::endpoints::Endpoints;

/// The endpoints, from endpoints.toml (user config over installed copy over
/// bundled defaults) - loaded once per process.
fn endpoints() -> &'static Endpoints {
    static CACHE: std::sync::OnceLock<Endpoints> = std::sync::OnceLock::new();
    CACHE.get_or_init(Endpoints::load)
}
const CSV_HEADER: &str =
    "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)";
/// GitHub rejects requests without a User-Agent.
const USER_AGENT: &str = "gamebus-presenced-setup (umu-miss review)";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

pub fn run(args: &[String]) -> ExitCode {
    let opts = match Opts::parse(&args[1..]) {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("{e}");
            eprintln!("Run 'gamebus-setup help' for usage.");
            return ExitCode::FAILURE;
        }
    };

    if opts.fetch {
        match fetch_full_dump(&api_base()) {
            Ok((path, n)) => println!("Fetched the umu database: {n} entries → {}", path.display()),
            Err(e) => {
                eprintln!("Fetch failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let mut report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        // Never show "no misses" over a file that failed to parse - that
        // reads as data loss. Nothing writes to it either (the report
        // refuses), so the user can repair or move it.
        eprintln!("{e}");
        eprintln!("Fix or move the file; nothing has overwritten it.");
        return ExitCode::FAILURE;
    }
    if report.path().is_none() {
        eprintln!("Cannot resolve the stash path (no HOME).");
        return ExitCode::FAILURE;
    }
    if report.entries().is_empty() {
        if !opts.fetch {
            println!("No umu-database misses recorded yet.");
            println!(
                "({} - written by the daemon when a game launches with GAMEID=umu-0.)",
                report.path().expect("checked above").display()
            );
        }
        return ExitCode::SUCCESS;
    }

    let db = match load_db(&opts) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    if opts.verify {
        match verify(&mut report, db.as_ref()) {
            Ok(lines) => {
                for line in lines {
                    println!("{line}");
                }
            }
            Err(e) => {
                eprintln!("Verify failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    if opts.check_prs {
        if let Err(e) = check_open_prs(&mut report) {
            // Best-effort by design: report, do not fail the whole command.
            eprintln!("Open-PR check failed (non-fatal): {e}");
        }
    }

    if opts.export {
        export_csv(&report);
    }
    if let Some(dest) = &opts.export_md {
        if let Err(e) = export_markdown(&report, dest.as_deref()) {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    }
    if !opts.export && opts.export_md.is_none() && !opts.verify && !opts.fetch && !opts.check_prs {
        list(&report);
    }
    ExitCode::SUCCESS
}

struct Opts {
    verify: bool,
    fetch: bool,
    export: bool,
    /// `Some(None)` = stdout, `Some(Some(path))` = write to file.
    export_md: Option<Option<PathBuf>>,
    check_prs: bool,
    db: Option<PathBuf>,
}

impl Opts {
    /// No flags at all - the TUI flows use the same database resolution as
    /// a bare CLI invocation (env override, then the fetch cache).
    fn none() -> Self {
        Self {
            verify: false,
            fetch: false,
            export: false,
            export_md: None,
            check_prs: false,
            db: None,
        }
    }

    fn parse(args: &[String]) -> Result<Self, String> {
        let mut opts = Self::none();
        let mut it = args.iter().peekable();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--verify" => opts.verify = true,
                "--fetch" => opts.fetch = true,
                "--export" => opts.export = true,
                "--check-prs" => opts.check_prs = true,
                "--export-md" => {
                    let file = it
                        .peek()
                        .filter(|a| !a.starts_with("--"))
                        .map(|a| PathBuf::from(a.as_str()));
                    if file.is_some() {
                        it.next();
                    }
                    opts.export_md = Some(file);
                }
                "--db" => {
                    let path = it.next().ok_or("--db needs a path")?;
                    opts.db = Some(PathBuf::from(path));
                }
                other => return Err(format!("Unknown umu-misses option: {other}")),
            }
        }
        Ok(opts)
    }
}

fn api_base() -> String {
    std::env::var("GAMEBUS_UMU_API").unwrap_or_else(|_| endpoints().umu_api.clone())
}

/// The local database, from the first source that exists: `--db`, the
/// `GAMEBUS_UMU_DB` environment variable, or the `--fetch` cache. An
/// explicitly named file that fails to parse is a hard error; a stale cache
/// just degrades to API-only verification.
fn load_db(opts: &Opts) -> Result<Option<UmuDb>, String> {
    let explicit = opts
        .db
        .clone()
        .or_else(|| std::env::var_os("GAMEBUS_UMU_DB").map(PathBuf::from));
    if let Some(path) = explicit {
        return UmuDb::load_from(&path).map(Some);
    }
    let Some(cache) = UmuDb::cache_path().filter(|p| p.exists()) else {
        return Ok(None);
    };
    match UmuDb::load_from(&cache) {
        Ok(db) => Ok(Some(db)),
        Err(e) => {
            eprintln!("Ignoring the cached database ({e}) - re-run with --fetch to refresh it.");
            Ok(None)
        }
    }
}

/// `--fetch`: one request to the bare API endpoint (the full dump), validated
/// by parsing before it replaces the cache. Atomic, like every write here.
fn fetch_full_dump(api: &str) -> Result<(PathBuf, usize), String> {
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

/// Returns the human-readable summary as lines rather than printing: the
/// CLI prints them, the TUI logs them into its output pane (a `println!`
/// under raw mode would tear the screen).
fn verify(report: &mut UmuReport, db: Option<&UmuDb>) -> Result<Vec<String>, String> {
    let api = api_base();
    let naming = NamingDb::load();
    let today = umu_report::today();
    let mut lines = Vec::new();
    if db.is_none() {
        lines.push("No local database (--db / GAMEBUS_UMU_DB / --fetch cache) - every entry goes to the API, and id drafting is skipped: collisions cannot be checked without the full database.".to_string());
    }

    let mut keys: Vec<String> = report.entries().keys().cloned().collect();
    keys.sort();
    let mut verdicts: Vec<(String, Verdict)> = Vec::new();
    let mut api_down: Option<String> = None;

    for key in &keys {
        let m = &report.entries()[key];
        // Local pass: the exact launch first, then the title.
        if let Some(db) = db {
            if let Some(code) = m.codename.as_deref() {
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
            if let Some(title) = m.title.as_deref() {
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
            let Some(title) = m.title.as_deref() else {
                continue;
            };
            let appid = naming.as_ref().and_then(|n| n.steam_appid_for_title(title));
            match draft_umu_id(db, title, m.codename.as_deref(), appid) {
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

    // Single write-back, then the human-readable summary.
    for (key, verdict) in &verdicts {
        report.update(key, |m| {
            m.verification = Some(Verification {
                state: verdict.state,
                umu_id: verdict.umu_id.clone(),
                checked: today.clone(),
                note: verdict.note.clone(),
            });
            m.drafted_id = verdict.drafted.clone();
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
        let title = m.title.as_deref().unwrap_or("(unresolved)");
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
    }
    Ok(lines)
}

/// Everything the TUI misses pane's `v` key does: refresh the cached full
/// dump, then verify the stash against it (and the live API). Blocking -
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
/// against the local database (mandatory - no database, no assignment),
/// then store it as a [`DraftBasis::Manual`] draft on the entry.
pub(crate) fn tui_assign_id(key: &str, id: &str) -> (Vec<String>, bool) {
    let id = id.trim().to_lowercase();
    let mut report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        return (vec![e.to_string()], false);
    }
    let Some(title) = report.entries().get(key).map(|m| m.title.clone()) else {
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
                    "No local database to collision-check against - press v to fetch it first."
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

/// Every store id the database actually uses (counted from the upstream
/// CSV), most common first - the TUI's `s` key cycles these.
pub(crate) const KNOWN_STORES: &[&str] = &[
    "egs",
    "gog",
    "amazon",
    "ubisoft",
    "humble",
    "ea",
    "zoomplatform",
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
        format!("Store set to {next} (daemon guessed {guessed}) - press v to re-verify.")
    };
    report.save();
    (vec![line], true)
}

/// The TUI's `d`: dismiss the selected entry, or restore it. A dismissed
/// entry is parked (bottom of the list, greyed, out of every export), not
/// deleted - a deleted key would be resurrected by the daemon's merge and
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
    let title = m
        .title
        .clone()
        .unwrap_or_else(|| "(unresolved)".to_string());
    let line = if m.dismissed.is_some() {
        report.update(key, |m| m.dismissed = None);
        format!("{title} restored - back in the list and the exports.")
    } else {
        report.update(key, |m| m.dismissed = Some(umu_report::today()));
        format!("{title} dismissed - kept in the stash, out of the exports (d restores).")
    };
    report.save();
    (vec![line], true)
}

/// The pure half of a manual assignment: shape rules, then the same
/// mandatory collision check every draft gets.
fn check_assignment(db: &UmuDb, title: Option<&str>, id: &str) -> Result<String, String> {
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
/// authoritative - verified against the live API: it matches exactly. The title
/// lookup does substring matching (`?title=Control` returns Ground
/// Control's ids) and its rows carry no title to compare against, so a hit
/// there is advisory only: it lands in the note for the human, never in the
/// verdict's id. `Ok(None)` means the API answered and found nothing.
fn api_check(api: &str, m: &Miss) -> Result<Option<Verdict>, String> {
    if let Some(code) = m
        .codename
        .as_deref()
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
    if let Some(title) = m.title.as_deref() {
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
fn check_open_prs(report: &mut UmuReport) -> Result<(), String> {
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
            .title
            .clone()
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
        .codename
        .as_deref()
        .filter(|c| c.len() >= 4 && !c.eq_ignore_ascii_case("none"))
        .is_some_and(|c| diff_lower.contains(&format!(",{},", c.to_lowercase())));
    let title_hit = m.title.as_deref().is_some_and(|t| {
        let plain = format!("\n+{},", t.to_lowercase());
        let quoted = format!("\n+{},", csv_field(t).to_lowercase());
        diff_lower.contains(&plain) || diff_lower.contains(&quoted)
    });
    codename_hit || title_hit
}

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
            m.codename.clone().unwrap_or_else(|| "none".into())
        };
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
/// paths - those embed `/home/<user>` and whatever else the install layout
/// leaks. Basename only, either separator (Wine paths carry backslashes).
fn exe_basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn csv_line(row: &SubmissionRow<'_>) -> String {
    let m = row.miss;
    let mut note = format!(
        "resolved by gamebus-presenced ({}, {} confidence)",
        m.title_source.as_deref().unwrap_or("unknown"),
        confidence_label(m.confidence),
    );
    if let Some(p) = &row.id_provenance {
        note.push_str("; ");
        note.push_str(p);
    }
    if let Some(v) = &m.verification {
        if let Some(vn) = &v.note {
            note.push_str("; ");
            note.push_str(vn);
        }
    }
    if m.effective_store() == "none" {
        if let Some(code) = m.codename.as_deref().filter(|c| !c.is_empty()) {
            note.push_str(&format!("; launcher codename was '{code}' (store unknown)"));
        }
    }
    format!(
        "{},{},{},{},,{},{}",
        csv_field(row.title),
        m.effective_store().to_lowercase(),
        // Codename comes verbatim from an untrusted process's environment
        // (HEROIC_APP_NAME) - escaped like every other external field, or a
        // comma in it would shift the columns and forge the UMU_ID cell.
        csv_field(&row.codename),
        row.umu_id,
        csv_field(&note),
        m.executable
            .as_deref()
            .map(exe_basename)
            .map(csv_field)
            .unwrap_or_default(),
    )
}

/// `--export`: the submission CSV on stdout, held-back entries on stderr.
fn export_csv(report: &UmuReport) {
    let (rows, held) = partition(report);
    println!("# umu-database submission draft - review before submitting!");
    println!("# Rules: {}#readme", endpoints().umu_repository);
    println!("# umu-FIXME means no id could be drafted safely - resolve by hand.");
    println!("{CSV_HEADER}");
    for row in &rows {
        println!("{}", csv_line(row));
    }
    for (m, reason) in &held {
        eprintln!(
            "(held back: {} - {reason})",
            m.title.as_deref().unwrap_or("(unresolved)")
        );
    }
}

/// `--export-md`: the slim merge request - title line, one honest paragraph,
/// the CSV in a fenced block, per-entry evidence, and the README's own rules
/// as a checklist.
fn export_markdown(report: &UmuReport, dest: Option<&std::path::Path>) -> Result<(), String> {
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
        md.push_str(&format!(
            "- **{}** - store `{}`, codename `{}`{}; title from {} ({} confidence); {}.\n",
            row.title,
            m.effective_store().to_lowercase(),
            row.codename,
            m.executable
                .as_deref()
                .map(|e| format!(", exe `{}`", exe_basename(e)))
                .unwrap_or_default(),
            m.title_source.as_deref().unwrap_or("unknown"),
            confidence_label(m.confidence),
            row.id_provenance
                .as_deref()
                .unwrap_or("id left as umu-FIXME - needs a human"),
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
    if !held.is_empty() {
        md.push_str("\n<!-- Held back, not part of this submission:\n");
        for (m, reason) in &held {
            md.push_str(&format!(
                "  {} - {reason}\n",
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

/// The default review list.
fn list(report: &UmuReport) {
    println!(
        "umu-database misses collected by the daemon ({}):",
        report.path().expect("caller checked").display()
    );
    println!();
    let mut rows: Vec<&Miss> = report.entries().values().collect();
    rows.sort_by(|a, b| a.last_seen.cmp(&b.last_seen).reverse());
    for m in rows {
        println!(
            "  {:<28} store: {:<8} codename: {:<16} {} [{}{}] seen {}",
            m.title.as_deref().unwrap_or("(unresolved)"),
            m.store,
            m.codename.as_deref().unwrap_or("-"),
            m.umu_id,
            confidence_label(m.confidence),
            m.title_source
                .as_deref()
                .map(|s| format!(", {s}"))
                .unwrap_or_default(),
            m.last_seen,
        );
        let status = match (&m.verification, &m.drafted_id) {
            (Some(v), Some(d)) if v.state == VerificationState::ConfirmedMissing => Some(format!(
                "confirmed missing {} - drafted {} ({})",
                v.checked,
                d.id,
                basis_label(d.basis)
            )),
            (Some(v), _) => Some(match v.state {
                VerificationState::AlreadyInDatabase => format!(
                    "already in the database as {} (checked {})",
                    v.umu_id.as_deref().unwrap_or("?"),
                    v.checked
                ),
                VerificationState::CrossStoreId => format!(
                    "cross-store id {} (checked {})",
                    v.umu_id.as_deref().unwrap_or("?"),
                    v.checked
                ),
                VerificationState::ConfirmedMissing => {
                    format!("confirmed missing {} - no id drafted yet", v.checked)
                }
            }),
            (None, _) => None,
        };
        if let Some(status) = status {
            println!("  {:<28} {status}", "");
        }
        if let Some(pr) = &m.possible_pr {
            println!("  {:<28} possibly already submitted: {pr}", "");
        }
        if let Some(when) = &m.dismissed {
            println!(
                "  {:<28} dismissed {when} - excluded from exports (d in the TUI restores)",
                ""
            );
        }
    }
    println!();
    println!("Verify against the database:  gamebus-setup umu-misses --verify   (--fetch first for a local copy)");
    println!(
        "Export a submission draft:    gamebus-setup umu-misses --export | --export-md [file]\n                              (both print to stdout; --export-md writes to the file when given one)"
    );
}

/// Shared with the TUI's misses pane (`super::ui`).
pub(crate) fn basis_label(basis: DraftBasis) -> &'static str {
    match basis {
        DraftBasis::SteamSku => "the game's Steam appid (detectable.json sku)",
        DraftBasis::StoreId => "the store's own codename",
        DraftBasis::TitleSlug => "the title, standalone-rule slug",
        DraftBasis::Manual => "manual assignment in the setup TUI",
    }
}

/// Shared with the TUI's misses pane (`super::ui`).
pub(crate) fn confidence_label(c: Option<Confidence>) -> &'static str {
    match c {
        Some(Confidence::High) => "high",
        Some(Confidence::Medium) => "medium",
        Some(Confidence::Low) => "low",
        None => "unresolved",
    }
}

fn plural_y(n: usize) -> &'static str {
    if n == 1 {
        "y"
    } else {
        "ies"
    }
}

/// Quote a CSV field when it needs it.
fn csv_field(v: &str) -> String {
    if v.contains(',') || v.contains('"') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

/// Percent-encode a query-string value (RFC 3986 unreserved set kept).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
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
            verification: None,
            drafted_id: None,
            possible_pr: None,
            store_override: None,
            dismissed: None,
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
