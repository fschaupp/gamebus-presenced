//! Writing the pages out.
//!
//! One file per game under `<out>/games/`, named by the title's slug, which
//! is where a checkout of gamebus-gamedb keeps them - point the export at
//! one and the pages land ready to commit.
//!
//! Nothing here ever overwrites a page this tool wrote. A page already on
//! disk may carry a reviewer's edits, a note, a second store entry somebody
//! added by hand; regenerating over that would throw away the only copy. The
//! export says the file is there and moves on, and deleting it is the way to
//! ask again.
//!
//! An ENHANCEMENT is the one case that writes over a file, and it is not a
//! regeneration: the page's own text is read first and the additions are
//! appended with `toml_edit`, so everything already there survives byte for
//! byte. That promise (see `enhance.rs`) is what makes it acceptable.

use std::path::{Path, PathBuf};

use crate::umu_report::UmuReport;

use super::index::Loaded;
use super::pages::{additions_label, candidates, render_page, slug, Addition, Status};
use super::{enhance, HTTP_TIMEOUT, USER_AGENT};

/// A local directory of pages, standing in for the raw files on the data
/// set's main branch. The gamedb twin of `GAMEBUS_GAMEDB_INDEX`, and what
/// keeps the tests off the network.
pub(super) const PAGES_ENV: &str = "GAMEBUS_GAMEDB_PAGES";

/// What one export did.
pub(super) struct Summary {
    pub(super) written: usize,
    /// Pages that already existed upstream and gained something.
    pub(super) enhanced: usize,
    pub(super) held: usize,
    /// One line per candidate, in the order they were considered.
    pub(super) lines: Vec<String>,
}

/// Write every ready page under `<out_dir>/games/`, and enhance every
/// published page this machine knows more about.
///
/// The index is not optional: a page written without checking what the data
/// set already carries is how two pages end up claiming one game, which is
/// the failure `gamedb/CONTRIBUTING.md` spends its longest section on. The
/// caller is the one that decides whether a stale index is good enough.
///
/// `fetch_pages` is the user's word that one request per enhancement is
/// acceptable. Without it, an enhancement whose page is not in `out_dir`
/// says what it would have added and stops.
pub(super) fn export(
    report: &UmuReport,
    index: &Loaded,
    out_dir: &Path,
    fetch_pages: bool,
) -> Result<Summary, String> {
    let games = out_dir.join("games");
    std::fs::create_dir_all(&games)
        .map_err(|e| format!("cannot create {}: {e}", games.display()))?;

    let mut summary = Summary {
        written: 0,
        enhanced: 0,
        held: 0,
        lines: Vec::new(),
    };
    for candidate in candidates(report, Some(&index.index)) {
        let title = candidate
            .title
            .clone()
            .unwrap_or_else(|| "(unresolved)".into());
        match &candidate.status {
            Status::InGamedb { id, title: theirs } => {
                summary.held += 1;
                summary.lines.push(format!(
                    "  held back  {title} - already in gamebus-gamedb as {id} ({theirs})"
                ));
                continue;
            }
            Status::Enhance {
                id,
                title: theirs,
                additions,
            } => {
                enhance_one(
                    &mut summary,
                    &index.index,
                    &games,
                    fetch_pages,
                    &title,
                    id,
                    theirs,
                    additions,
                );
                continue;
            }
            Status::Incomplete { reason } => {
                summary.held += 1;
                summary
                    .lines
                    .push(format!("  held back  {title} - {reason}"));
                continue;
            }
            Status::Ready => {}
        }
        let path = games.join(candidate.file_name());
        if path.exists() {
            summary.held += 1;
            summary.lines.push(format!(
                "  held back  {title} - already written at {}; delete it to regenerate",
                path.display()
            ));
            continue;
        }
        std::fs::write(&path, render_page(&candidate))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        summary.written += 1;
        summary.lines.push(format!(
            "  wrote      {} ({title}{})",
            path.display(),
            match candidate.entries {
                1 => String::new(),
                n => format!(", {n} launches folded in"),
            }
        ));
    }
    Ok(summary)
}

/// One enhancement: find the published page's text, apply the additions,
/// write it back. Every failure is a held-back line rather than an aborted
/// export - the other candidates have nothing to do with it.
#[allow(clippy::too_many_arguments)]
fn enhance_one(
    summary: &mut Summary,
    index: &super::index::GamedbIndex,
    games: &Path,
    fetch_pages: bool,
    title: &str,
    id: &str,
    theirs: &str,
    additions: &[Addition],
) {
    let labels = additions_label(additions);
    // The index names the file; an index built before the `page` column
    // existed does not, and the file name is the title's slug anyway.
    let page = index
        .page(id)
        .map(str::to_string)
        .unwrap_or_else(|| slug(theirs));
    let path = games.join(format!("{page}.toml"));

    let text = match page_text(games, &page, fetch_pages) {
        Ok(Some(text)) => text,
        Ok(None) => {
            summary.held += 1;
            summary.lines.push(format!(
                "  held back  {title} - in gamebus-gamedb as {id}, enhancement available: \
                 {labels} - point --out at a checkout of gamebus-gamedb, or pass --fetch-pages"
            ));
            return;
        }
        Err(e) => {
            summary.held += 1;
            summary
                .lines
                .push(format!("  held back  {title} - {id}: {e}"));
            return;
        }
    };
    let enhanced = match enhance::apply(&text, additions) {
        Ok(enhanced) => enhanced,
        Err(e) => {
            summary.held += 1;
            summary
                .lines
                .push(format!("  held back  {title} - {page}.toml {e}"));
            return;
        }
    };
    // The text was already what is on disk: nothing to say, nothing to write.
    if enhanced == text && path.exists() {
        summary.held += 1;
        summary.lines.push(format!(
            "  held back  {title} - already in gamebus-gamedb as {id} ({theirs})"
        ));
        return;
    }
    match std::fs::write(&path, enhanced) {
        Ok(()) => {
            summary.enhanced += 1;
            summary
                .lines
                .push(format!("  enhanced   {}: {labels}", path.display()));
        }
        Err(e) => {
            summary.held += 1;
            summary.lines.push(format!(
                "  held back  {title} - cannot write {}: {e}",
                path.display()
            ));
        }
    }
}

/// The published page's text, from the nearest place that has it: the
/// checkout being written to, a local directory named by [`PAGES_ENV`], then
/// (only when asked) the raw file on the data set's main branch. `Ok(None)`
/// means none of them had it, which is a hold-back and not an error.
fn page_text(games: &Path, page: &str, fetch_pages: bool) -> Result<Option<String>, String> {
    let name = format!("{page}.toml");
    let local = games.join(&name);
    if local.exists() {
        return std::fs::read_to_string(&local)
            .map(Some)
            .map_err(|e| format!("cannot read {}: {e}", local.display()));
    }
    if let Some(dir) = std::env::var_os(PAGES_ENV) {
        let path = PathBuf::from(dir).join(&name);
        if path.exists() {
            return std::fs::read_to_string(&path)
                .map(Some)
                .map_err(|e| format!("cannot read {}: {e}", path.display()));
        }
        // A directory that was named but does not have this page is a plain
        // miss, exactly like the checkout not having it.
        return Ok(None);
    }
    if !fetch_pages {
        return Ok(None);
    }
    let url = format!("{}/{name}", super::endpoints().gamedb_pages);
    match ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .timeout(HTTP_TIMEOUT)
        .call()
    {
        Ok(response) => response
            .into_string()
            .map(Some)
            .map_err(|e| format!("reading {url}: {e}")),
        // The index says the page exists, so a 404 means the two disagree -
        // an index older than a rename, most likely. Not fatal, and a
        // refreshed index is the fix.
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(e) => Err(format!("{url}: {e}")),
    }
}

/// What to do with the pages once they are written. The lint upstream is
/// what actually checks them, so pointing at it is more useful than any
/// reassurance this tool could give.
pub(super) fn hint() -> String {
    format!(
        "Next: open a pull request at {} - its lint checks the pages.",
        super::endpoints().gamedb_project
    )
}
