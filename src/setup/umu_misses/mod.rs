//! S9b — review, verify, draft, and export the umu-database miss stash.
//!
//! The daemon (network-free, always) writes the stash's resolution half;
//! this module writes the annotation half (verification, drafted ids, PR
//! marks — merged, never clobbered: see `UmuReport`) and talks to the world
//! on the user's explicit command:
//!
//! - default: the review list.
//! - `--verify`: check every miss against the database — local copy first
//!   (`--db`, `GAMEBUS_UMU_DB`, or the cached full dump), then the public
//!   API for whatever the local copy did not settle. Entries confirmed
//!   missing get a umu id drafted per the database's own rules, and every
//!   draft is collision-checked — no exceptions.
//! - `--fetch`: refresh the cached full dump. One request, only when asked.
//! - `--export`: submission-shaped CSV on stdout.
//! - `--export-md [file]`: a slim, ready-to-paste merge-request text —
//!   written to the file, or to stdout when none is given.
//! - `--check-prs`: best-effort scan of open upstream merge requests for
//!   entries someone already submitted. Opt-in, non-fatal, clearly labelled.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crate::endpoints::Endpoints;
use crate::umu_report::{Confidence, DraftBasis, Miss, UmuDb, UmuReport, VerificationState};

mod export;
mod online;
mod tui;
mod verify;

pub(crate) use self::online::{tui_egs_builds, tui_online_candidates};
pub use self::online::{EgsBuild, EgsOffer, GogProduct};
pub use self::tui::PickCandidate;
pub(crate) use self::tui::{
    tui_assign_id, tui_cycle_store, tui_fetch_and_verify, tui_pick_candidates, tui_pick_entry,
    tui_set_identity, tui_set_title, tui_toggle_dismiss,
};

use self::export::{export_csv, export_markdown};
use self::verify::{check_open_prs, fetch_full_dump, verify};

/// The endpoints, from endpoints.toml (user config over installed copy over
/// bundled defaults) — loaded once per process.
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
        // Never show "no misses" over a file that failed to parse — that
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
                "({} — written by the daemon when a game launches with GAMEID=umu-0.)",
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
    /// No flags at all — the TUI flows use the same database resolution as
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
            eprintln!("Ignoring the cached database ({e}) — re-run with --fetch to refresh it.");
            Ok(None)
        }
    }
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
        // An overridden title is the user's word, not the resolver's — the
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
            m.umu_id,
            m.last_seen,
        );
        let status = match (&m.verification, &m.drafted_id) {
            (Some(v), Some(d)) if v.state == VerificationState::ConfirmedMissing => Some(format!(
                "confirmed missing {} — drafted {} ({})",
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
                    format!("confirmed missing {} — no id drafted yet", v.checked)
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
                "  {:<28} dismissed {when} — excluded from exports (d in the TUI restores)",
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
/// Fixture database for the pick flow — the shapes the search hands the
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
