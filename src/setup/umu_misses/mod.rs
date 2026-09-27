//! Review, verify, draft, and export the umu-database miss stash.
//!
//! The stash holds every identity miss the daemon recorded - umu launches
//! that reported `GAMEID=umu-0`, and launcher launches (Lutris, Heroic)
//! that carried no umu id at all. All of them feed gamebus-gamedb; this
//! module runs the umu-database half of the pipeline, which takes only
//! [`umu_candidate`] entries: umu participation is opt-in per entry
//! (owner policy 2026-08-24), suggestion and promotion, never automatic
//! enrollment.
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
//!   draft is collision-checked - no exceptions. Each entry is also checked
//!   against the protonfix list: the database collects games that need a fix
//!   in Proton, so a game that runs out of the box is not a gap to submit.
//! - `--fetch`: refresh the cached full dump and the protonfix list. One
//!   request each, only when asked.
//! - `--export`: submission-shaped CSV on stdout - the games that need umu.
//! - `--export-md [file]`: a slim, ready-to-paste merge-request text  -
//!   written to the file, or to stdout when none is given.
//! - `--check-prs`: best-effort scan of open upstream merge requests for
//!   entries someone already submitted. Opt-in, non-fatal, clearly labelled.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crate::endpoints::Endpoints;
use crate::umu_report::{Confidence, DraftBasis, Miss, UmuDb, UmuReport, VerificationState};

mod export;
mod fixes;
mod online;
mod tui;
mod verify;

pub(crate) use self::online::{tui_egs_builds, tui_online_candidates};
pub use self::online::{EgsBuild, EgsOffer, GogProduct};
pub use self::tui::PickCandidate;
pub(crate) use self::tui::{
    tui_assign_id, tui_cycle_store, tui_fetch_and_verify, tui_pick_candidates, tui_pick_entry,
    tui_set_identity, tui_set_title, tui_toggle_dismiss, tui_toggle_promote,
};

use self::export::{export_csv, export_markdown};
use self::verify::{check_open_prs, fetch_full_dump, verify};

pub(crate) use gamebus_coupler::umu_candidate;

/// One line on where an entry stands with the umu-database pipeline.
/// Shared by the CLI list and the TUI's misses detail pane.
pub(crate) fn candidacy_line(m: &Miss) -> String {
    if !m.is_umu_miss() {
        return "launcher launch - no umu id; a gamedb identity record, not a umu-database gap"
            .into();
    }
    if let Some(date) = &m.umu_promoted {
        return format!("umu candidate: promoted by you {date} (u in the TUI reverts)");
    }
    if m.verification
        .as_ref()
        .is_some_and(|v| v.state == VerificationState::AlreadyInDatabase)
    {
        return "already in the umu database - the launcher missed, not the database".into();
    }
    if let Some(v) = m
        .verification
        .as_ref()
        .filter(|v| v.state == VerificationState::CrossStoreId)
    {
        return format!(
            "umu candidate, suggested: in the umu database via another store as {}",
            v.umu_id.as_deref().unwrap_or("?")
        );
    }
    if m.fix.as_ref().is_some_and(|f| f.has_fix()) {
        return "umu candidate, suggested: a protonfix exists - the game needs umu".into();
    }
    if m.fix.as_ref().is_some_and(|f| f.has_local_fix()) {
        return "umu candidate, suggested: a local protonfix exists - the game needs umu".into();
    }
    "not a umu candidate - no protonfix and no cross-store match (u in the TUI promotes)".into()
}

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
        // The second half of "what does upstream know": which games need a
        // fix at all. Non-fatal - verification degrades to an unchecked
        // scope, and the database copy is still worth having.
        match fixes::fetch(&endpoints().umu_protonfixes_tree) {
            Ok((_, n)) => println!("Fetched the protonfix list: {n} games need umu."),
            Err(e) => eprintln!("Protonfix list unavailable ({e}) - scope stays unchecked."),
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
            println!("No identity misses recorded yet.");
            println!(
                "({} - written by the daemon when umu launches a game unrecognised \
                 (GAMEID=umu-0), and when Lutris or Heroic hand it a launch without \
                 a store identity.)",
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

/// The default review list: every identity miss, umu or not. Non-umu
/// entries (launcher launches) show `-` for the id and their launcher-launch
/// tag; umu misses show where they stand with the candidacy policy.
fn list(report: &UmuReport) {
    println!(
        "Identity misses collected by the daemon ({}):",
        report.path().expect("caller checked").display()
    );
    println!();
    let mut rows: Vec<&Miss> = report.entries().values().collect();
    rows.sort_by(|a, b| a.last_seen.cmp(&b.last_seen).reverse());
    for m in rows {
        // An overridden title is the user's word, not the resolver's - the
        // bracket must not claim a source/confidence for it.
        let provenance = if m.title_override.is_some() {
            "set by you".to_string()
        } else {
            format!(
                "{}{}",
                confidence_label(m.confidence),
                m.title_source
                    .as_deref()
                    .map(|s| format!(", {s}"))
                    .unwrap_or_default()
            )
        };
        println!(
            "  {:<28} store: {:<8} codename: {:<16} {} [{provenance}] seen {}",
            m.effective_title().unwrap_or("(unresolved)"),
            m.store,
            m.codename.as_deref().unwrap_or("-"),
            // A launcher launch never carried a umu id: `-`, never "".
            if m.is_umu_miss() {
                m.umu_id.as_str()
            } else {
                "-"
            },
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
        if let Some(scope) = scope_line(m) {
            println!("  {:<28} {scope}", "");
        }
        println!("  {:<28} {}", "", candidacy_line(m));
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
    println!(
        "Launches Lutris or Heroic handed over without a store identity land here too;\nthey feed gamebus-gamedb and are never submitted to the umu database."
    );
    println!("Verify against the database:  gamebus-setup umu-misses --verify   (--fetch first for a local copy)");
    println!(
        "Export a submission draft:    gamebus-setup umu-misses --export | --export-md [file]\n                              (umu candidates only; both print to stdout, --export-md writes to the file when given one)"
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

/// One line on whether the database wants this entry at all. Upstream takes
/// games that need a fix in Proton; for the rest, a row is noise the
/// maintainers have to review. Shared by the CLI list, the verify summary,
/// and the TUI's misses pane. `None` until a verification run could read the
/// fix list.
pub(crate) fn scope_line(m: &Miss) -> Option<String> {
    let scope = m.fix.as_ref()?;
    Some(match scope.fixes.split_first() {
        Some((first, rest)) => {
            let more = if rest.is_empty() {
                String::new()
            } else {
                format!(" +{}", rest.len())
            };
            format!("needs umu: protonfix {first}{more} - worth submitting")
        }
        None if scope.has_local_fix() => format!(
            "needs umu: local protonfix {} - not upstream yet, submit it to umu-protonfixes before the row",
            scope.local[0]
        ),
        None if id_is_firm(m) => format!(
            "no protonfix for {} - runs out of the box, and the database only wants games that need a fix",
            scope.umu_id
        ),
        None => format!(
            "no protonfix for {} - but that id is our own guess, so nothing here shows the game needs umu either",
            scope.umu_id
        ),
    })
}

/// Whether the id the scope check ran against is the game's real one: the
/// database's own, or a Steam appid from detectable.json. A codename or slug
/// draft is a guess - a fix could exist under the Steam appid we never
/// learned, so "no fix found" must not be read as "runs out of the box".
pub(super) fn id_is_firm(m: &Miss) -> bool {
    if m.verification
        .as_ref()
        .is_some_and(|v| v.umu_id.is_some() && v.state != VerificationState::ConfirmedMissing)
    {
        return true;
    }
    m.drafted_id
        .as_ref()
        .is_some_and(|d| matches!(d.basis, DraftBasis::SteamSku | DraftBasis::Manual))
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
/// Fixture database for the pick flow - the shapes the search hands the
/// TUI as candidates.
fn pick_db() -> UmuDb {
    UmuDb::parse(concat!(
        "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)\n",
        "Borderlands 3,egs,Catnip,umu-397540,bl3,,\n",
        "Borderlands 3,gog,1454587428,umu-397540,bl3,,\n",
    ))
    .unwrap()
}

#[cfg(test)]
/// In-memory stash with one resolved miss (no path: persist no-ops).
fn pick_report(store: &str, codename: &str) -> (UmuReport, String) {
    let key = format!("{store}:{codename}");
    let mut report = UmuReport::default();
    report.note_launch(store, Some(codename), "umu-0", &key);
    report.note_title(
        store,
        Some(codename),
        &key,
        "Borderlands 3",
        "heroic-config",
        Confidence::High,
        None,
    );
    (report, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::umu_report::{FixCheck, Verification};

    fn miss(umu_id: &str) -> Miss {
        let (report, key) = pick_report("egs", "Catnip");
        let mut m = report.entries()[&key].clone();
        m.umu_id = umu_id.to_string();
        m
    }

    fn verified(state: VerificationState) -> Option<Verification> {
        Some(Verification {
            state,
            umu_id: Some("umu-397540".into()),
            checked: "2026-08-24".into(),
            note: None,
        })
    }

    fn fix_check(fixes: &[&str]) -> Option<FixCheck> {
        Some(FixCheck {
            umu_id: "umu-397540".into(),
            fixes: fixes.iter().map(|s| s.to_string()).collect(),
            local: vec![],
            checked: "2026-08-24".into(),
        })
    }

    /// The owner policy's truth table: opt-in via promotion, or one of the
    /// two suggestions - never membership by default, and never for a
    /// launch that did not go through umu.
    #[test]
    fn umu_candidacy_is_opt_in() {
        // A plain umu miss: recorded, but not a candidate.
        assert!(!umu_candidate(&miss("umu-0")));

        // Promoted by the user: a candidate whatever else is known.
        let mut m = miss("umu-0");
        m.umu_promoted = Some("2026-08-24".into());
        assert!(umu_candidate(&m));

        // The game is already active in the database via another store.
        let mut m = miss("umu-0");
        m.verification = verified(VerificationState::CrossStoreId);
        assert!(umu_candidate(&m));

        // Found under its own store: the launcher missed, not the database.
        let mut m = miss("umu-0");
        m.verification = verified(VerificationState::AlreadyInDatabase);
        assert!(!umu_candidate(&m));

        // A protonfix exists: the game needs umu.
        let mut m = miss("umu-0");
        m.fix = fix_check(&["gamefixes-steam/397540.py"]);
        assert!(umu_candidate(&m));

        // A local fix proves the same need.
        let mut m = miss("umu-0");
        m.fix = fix_check(&[]);
        m.fix.as_mut().unwrap().local =
            vec!["/h/.config/protonfixes/localfixes/umu-397540.py".into()];
        assert!(umu_candidate(&m));
        assert!(scope_line(&m).unwrap().contains("local protonfix"));

        // A fix check that found nothing is no suggestion.
        let mut m = miss("umu-0");
        m.fix = fix_check(&[]);
        assert!(!umu_candidate(&m));

        // A launcher launch never qualifies, whatever its annotations say.
        let mut m = miss("");
        m.umu_promoted = Some("2026-08-24".into());
        m.verification = verified(VerificationState::CrossStoreId);
        m.fix = fix_check(&["gamefixes-steam/397540.py"]);
        assert!(!umu_candidate(&m));
    }

    #[test]
    fn the_candidacy_line_names_each_state() {
        assert!(candidacy_line(&miss("")).contains("launcher launch"));
        assert!(candidacy_line(&miss("umu-0")).contains("not a umu candidate"));
        assert!(candidacy_line(&miss("umu-0")).contains("u in the TUI promotes"));

        let mut m = miss("umu-0");
        m.umu_promoted = Some("2026-08-24".into());
        assert!(candidacy_line(&m).contains("promoted by you 2026-08-24"));

        let mut m = miss("umu-0");
        m.verification = verified(VerificationState::CrossStoreId);
        let line = candidacy_line(&m);
        assert!(
            line.contains("suggested") && line.contains("umu-397540"),
            "{line}"
        );

        let mut m = miss("umu-0");
        m.fix = fix_check(&["gamefixes-steam/397540.py"]);
        let line = candidacy_line(&m);
        assert!(
            line.contains("suggested") && line.contains("protonfix"),
            "{line}"
        );
    }
}
