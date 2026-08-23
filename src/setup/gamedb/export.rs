//! Writing the pages out.
//!
//! One file per game under `<out>/games/`, named by the title's slug, which
//! is where a checkout of gamebus-gamedb keeps them - point the export at
//! one and the pages land ready to commit.
//!
//! Nothing here ever overwrites. A page already on disk may carry a
//! reviewer's edits, a note, a second store entry somebody added by hand;
//! regenerating over that would throw away the only copy. The export says
//! the file is there and moves on, and deleting it is the way to ask again.

use std::path::Path;

use crate::umu_report::UmuReport;

use super::index::Loaded;
use super::pages::{candidates, render_page, Status};

/// What one export did.
pub(super) struct Summary {
    pub(super) written: usize,
    pub(super) held: usize,
    /// One line per candidate, in the order they were considered.
    pub(super) lines: Vec<String>,
}

/// Write every ready page under `<out_dir>/games/`.
///
/// The index is not optional: a page written without checking what the data
/// set already carries is how two pages end up claiming one game, which is
/// the failure `gamedb/CONTRIBUTING.md` spends its longest section on. The
/// caller is the one that decides whether a stale index is good enough.
pub(super) fn export(
    report: &UmuReport,
    index: &Loaded,
    out_dir: &Path,
) -> Result<Summary, String> {
    let games = out_dir.join("games");
    std::fs::create_dir_all(&games)
        .map_err(|e| format!("cannot create {}: {e}", games.display()))?;

    let mut summary = Summary {
        written: 0,
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

/// What to do with the pages once they are written. The lint upstream is
/// what actually checks them, so pointing at it is more useful than any
/// reassurance this tool could give.
pub(super) fn hint() -> String {
    format!(
        "Next: open a pull request at {} - its lint checks the pages.",
        super::endpoints().gamedb_project
    )
}
