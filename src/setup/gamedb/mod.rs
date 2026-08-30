//! S10 - contributing the daemon's identity misses to gamebus-gamedb.
//!
//! The umu pipeline next door answers one question: which of these games
//! does umu-database want? Its answer for most of them is "none of them" -
//! upstream takes games that need a fix in Proton, and a game that runs out
//! of the box is not a gap to submit. What is left over is still knowledge:
//! this Epic app name is that game, this executable is not the game Discord
//! thinks it is. gamebus-gamedb is where that goes, and this module turns
//! the stash into its pages.
//!
//! - default: the candidate list, with what the data set already carries.
//! - `--fetch`: refresh the cached identity index. One request, on the
//!   user's word, like every other fetch in this tool.
//! - `--export --out DIR` (or `--documents`): write the ready pages, and
//!   enhance the published ones this machine knows more about.
//! - `--fetch-pages`: let an enhancement fetch the page it is adding to,
//!   for when `--out` is not a checkout of the data set. One request per
//!   page, on the user's word, like every other fetch in this tool.
//!
//! Nothing here runs without being asked, nothing overwrites a file, and
//! the export refuses to run against an index too old to be trusted -
//! because the one thing that must not happen is a second page for a game
//! the data set already has.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crate::endpoints::Endpoints;
use crate::umu_report::UmuReport;

mod config;
mod enhance;
mod export;
mod index;
mod pages;
mod tui;

pub(crate) use self::tui::{
    tui_export, tui_fetch, tui_set_dir, tui_view, GamedbFilter, GamedbRow, GamedbView, RowState,
};

use self::pages::{additions_label, candidates, Status};

/// The endpoints, from endpoints.toml - loaded once per process, like the
/// umu side's.
fn endpoints() -> &'static Endpoints {
    static CACHE: std::sync::OnceLock<Endpoints> = std::sync::OnceLock::new();
    CACHE.get_or_init(Endpoints::load)
}

/// GitHub rejects requests without a User-Agent.
const USER_AGENT: &str = "gamebus-presenced-setup (gamedb export)";
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
        match index::fetch(index::identities_url()) {
            Ok((idx, release)) => println!(
                "Fetched the gamebus-gamedb index: {} games, release {}.",
                idx.len(),
                release.as_deref().unwrap_or("unknown")
            ),
            // Not fatal, and not an HTTP error either: an unpublished data
            // set is a normal state of the world, and a listing still works.
            Err(e) => eprintln!("{}", e.message()),
        }
    }

    let report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        eprintln!("{e}");
        eprintln!("Fix or move the file; nothing has overwritten it.");
        return ExitCode::FAILURE;
    }
    if report.path().is_none() {
        eprintln!("Cannot resolve the stash path (no HOME).");
        return ExitCode::FAILURE;
    }

    let loaded = match index::load() {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    if opts.export {
        let Some(out_dir) = opts.out_dir() else {
            eprintln!("--export needs somewhere to write: --out DIR, or --documents for");
            eprintln!(
                "{}.",
                config::default_export_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "your documents directory".to_string())
            );
            return ExitCode::FAILURE;
        };
        let Some(loaded) = loaded else {
            eprintln!("No gamebus-gamedb index - run with --fetch first.");
            return ExitCode::FAILURE;
        };
        if loaded.stale && !opts.stale_ok {
            eprintln!(
                "The index is {} days old - add --fetch to refresh it, or --stale-ok to \
                 export against it anyway.",
                loaded.age.map(|a| a.as_secs() / 86_400).unwrap_or(0)
            );
            return ExitCode::FAILURE;
        }
        return match export::export(&report, &loaded, &out_dir, opts.fetch_pages) {
            Ok(summary) => {
                for line in &summary.lines {
                    println!("{line}");
                }
                println!();
                println!(
                    "{} page(s) written to {}, {} enhanced, {} held back.",
                    summary.written,
                    out_dir.join("games").display(),
                    summary.enhanced,
                    summary.held
                );
                if summary.written > 0 || summary.enhanced > 0 {
                    println!("{}", export::hint());
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("Export failed: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if !opts.fetch {
        list(&report, loaded.as_ref());
    }
    ExitCode::SUCCESS
}

struct Opts {
    fetch: bool,
    export: bool,
    out: Option<PathBuf>,
    documents: bool,
    stale_ok: bool,
    /// Let an enhancement fetch the published page it adds to, when the
    /// destination is not a checkout that already has it.
    fetch_pages: bool,
}

impl Opts {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut opts = Self {
            fetch: false,
            export: false,
            out: None,
            documents: false,
            stale_ok: false,
            fetch_pages: false,
        };
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--fetch" => opts.fetch = true,
                "--export" => opts.export = true,
                "--documents" => opts.documents = true,
                "--stale-ok" => opts.stale_ok = true,
                "--fetch-pages" => opts.fetch_pages = true,
                "--out" => {
                    let path = it.next().ok_or("--out needs a directory")?;
                    opts.out = Some(PathBuf::from(path));
                }
                other => return Err(format!("Unknown gamedb option: {other}")),
            }
        }
        Ok(opts)
    }

    /// Where the export writes. Deliberately NOT the configured directory:
    /// on the command line the destination is stated, every time. The
    /// setting is the TUI's, where `e` has no room for a path argument.
    fn out_dir(&self) -> Option<PathBuf> {
        self.out
            .clone()
            .or_else(|| self.documents.then(config::default_export_dir).flatten())
    }
}

/// The default listing: one line per game, and where things stand.
fn list(report: &UmuReport, loaded: Option<&index::Loaded>) {
    println!(
        "Games this machine could contribute to gamebus-gamedb ({}):",
        report.path().expect("caller checked").display()
    );
    println!();
    let pages = candidates(report, loaded.map(|l| &l.index));
    if pages.is_empty() {
        println!("  (nothing yet - the daemon records a miss when umu launches a game");
        println!("   it has no database entry for)");
    }
    for candidate in &pages {
        let title = candidate.title.as_deref().unwrap_or("(unresolved)");
        let (glyph, note) = match &candidate.status {
            Status::InGamedb { id, .. } => ("✓", format!("in gamebus-gamedb as {id}")),
            // Not a page to write: a page to add to. The glyph says so, and
            // the line says exactly what would be added.
            Status::Enhance { id, additions, .. } => (
                "+",
                format!(
                    "in gamebus-gamedb as {id}, enhancement: {}",
                    additions_label(additions)
                ),
            ),
            Status::Ready => (
                "●",
                format!(
                    "{}{}",
                    candidate
                        .canonical_id()
                        .unwrap_or_else(|| "no id yet".to_string()),
                    describe_identity(candidate)
                ),
            ),
            Status::Incomplete { reason } => ("○", reason.clone()),
        };
        println!("  {glyph} {title:<32} {note}");
    }
    println!();
    match loaded {
        Some(loaded) => println!("gamebus-gamedb index: {}", index::describe(loaded)),
        None => println!("gamebus-gamedb index: no index cached - run --fetch"),
    }
    match config::load().export_dir() {
        Some(dir) => println!("Export directory:     {}", dir.display()),
        None => println!("Export directory:     unknown (no HOME)"),
    }
    println!();
    println!("Refresh what the data set carries:  gamebus-setup gamedb --fetch");
    println!("Write the ready pages:              gamebus-setup gamedb --export --out DIR");
    println!("                                    (--documents writes to the directory above)");
    println!("A + above is an edit to a page that already exists: point --out at a checkout");
    println!("of gamebus-gamedb, or add --fetch-pages to fetch the page it adds to (net).");
}

/// The stores and executables behind a ready page, for the listing's line.
fn describe_identity(candidate: &pages::Candidate) -> String {
    let mut parts = Vec::new();
    for entry in &candidate.stores {
        parts.push(format!("{}/{}", entry.store, entry.codename));
    }
    for exe in &candidate.exes {
        parts.push(exe.clone());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flags_parse_and_an_unknown_one_is_refused() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let opts = Opts::parse(&args(&["--fetch", "--export", "--out", "/tmp/x"])).unwrap();
        assert!(opts.fetch && opts.export && !opts.stale_ok && !opts.fetch_pages);
        assert!(Opts::parse(&args(&["--fetch-pages"])).unwrap().fetch_pages);
        assert_eq!(opts.out.as_deref(), Some(std::path::Path::new("/tmp/x")));
        assert!(
            Opts::parse(&args(&["--out"])).is_err(),
            "--out needs a path"
        );
        assert!(Opts::parse(&args(&["--nope"])).is_err());
    }

    /// The command line names its destination every time. Falling back to
    /// the configured directory here would let a copy-pasted command write
    /// somewhere the person running it never read.
    #[test]
    fn an_export_without_a_destination_has_none() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(Opts::parse(&args(&["--export"])).unwrap().out_dir(), None);
        assert_eq!(
            Opts::parse(&args(&["--export", "--out", "/tmp/x"]))
                .unwrap()
                .out_dir()
                .as_deref(),
            Some(std::path::Path::new("/tmp/x"))
        );
    }
}
