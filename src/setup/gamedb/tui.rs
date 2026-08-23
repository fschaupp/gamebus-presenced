//! The TUI flows for the gamedb pane: refresh, export, set the directory.
//!
//! Same shape as the misses pane's bridge next door - each function does
//! the blocking work off the render path and hands back log lines plus
//! whether it completed, so the pane itself stays a pure render of state.

use std::path::PathBuf;

use crate::umu_report::UmuReport;

use super::config;
use super::export;
use super::index;
use super::pages::{candidates, Status};

/// One row of the gamedb pane: a display copy of a candidate page, not the
/// page itself. The pane never renders TOML, and keeping the two apart is
/// what stops the fold's internals from leaking into the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GamedbRow {
    /// The fold's group key - the row's identity across a refresh.
    pub key: String,
    pub title: String,
    /// The file this page would be written to, `<slug>.toml`.
    pub file: String,
    pub state: RowState,
    /// The canonical id the page would carry, where one is derivable.
    pub id: Option<String>,
    /// `store/codename`, one per store entry.
    pub stores: Vec<String>,
    pub exes: Vec<String>,
    pub steam: Option<u64>,
    pub note: Option<String>,
    /// How many stash entries folded into this one page.
    pub entries: usize,
}

/// Where a row stands - the three glyphs the pane draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    /// Already published, with the id it resolves to upstream.
    InGamedb(String),
    Ready,
    /// Would not pass the lint, and why.
    Incomplete(String),
}

/// Everything the pane needs in one message: the rows, how much to trust
/// the index, and where an export would go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GamedbView {
    pub rows: Vec<GamedbRow>,
    /// The index line: `release v…, fetched N days ago`, or why there is none.
    pub index: String,
    /// True when an index is cached and fresh enough to export against.
    pub index_ok: bool,
    pub export_dir: String,
}

/// Load the pane's state: the stash, the cached index, the configured
/// directory. Local files only - `r` is the pane's one network key.
pub(crate) fn tui_view() -> GamedbView {
    let report = UmuReport::load();
    let loaded = index::load();
    let (index_line, index_ok, loaded) = match &loaded {
        Ok(Some(l)) if l.stale => (
            format!(
                "{} - {} days old, r refreshes it (net)",
                index::describe(l),
                l.age.map(|a| a.as_secs() / 86_400).unwrap_or(0)
            ),
            false,
            Some(l),
        ),
        Ok(Some(l)) => (index::describe(l), true, Some(l)),
        Ok(None) => (
            "no index cached - r fetches it (net)".to_string(),
            false,
            None,
        ),
        Err(e) => (e.clone(), false, None),
    };
    let rows = candidates(&report, loaded.map(|l| &l.index))
        .into_iter()
        .map(|c| GamedbRow {
            file: c.file_name(),
            id: c.canonical_id(),
            state: match &c.status {
                Status::InGamedb { id, .. } => RowState::InGamedb(id.clone()),
                Status::Ready => RowState::Ready,
                Status::Incomplete { reason } => RowState::Incomplete(reason.clone()),
            },
            title: c.title.clone().unwrap_or_else(|| "(unresolved)".into()),
            stores: c
                .stores
                .iter()
                .map(|e| format!("{}/{}", e.store, e.codename))
                .collect(),
            exes: c.exes.clone(),
            steam: c.steam,
            note: c.note.clone(),
            entries: c.entries,
            key: c.key,
        })
        .collect();
    GamedbView {
        rows,
        index: index_line,
        index_ok,
        export_dir: config::load()
            .export_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown (no HOME)".to_string()),
    }
}

/// The pane's `r`: one request for the published identity index. Blocking.
pub(crate) fn tui_fetch() -> (Vec<String>, bool) {
    match index::fetch(index::identities_url()) {
        Ok((idx, release)) => (
            vec![format!(
                "Fetched the gamebus-gamedb index: {} games, release {}.",
                idx.len(),
                release.as_deref().unwrap_or("unknown")
            )],
            true,
        ),
        // An unpublished data set is a state of the world, not a failure of
        // this command - but the pane still cannot check anything against it.
        Err(e) => (vec![e.message()], false),
    }
}

/// The pane's `e`: write every ready page to the configured directory.
///
/// `force` is the answer to the confirm dialog a stale or missing index
/// puts up; without it this refuses exactly like the command line does.
pub(crate) fn tui_export(force: bool) -> (Vec<String>, bool) {
    let Some(out_dir) = config::load().export_dir() else {
        return (
            vec!["No export directory (no HOME) - press d to set one.".to_string()],
            false,
        );
    };
    let report = UmuReport::load_for_annotations();
    if let Some(e) = report.load_error() {
        return (vec![e.to_string()], false);
    }
    let (loaded, mut lines) = match index::load() {
        Ok(Some(loaded)) if loaded.stale && !force => {
            return (
                vec![format!(
                    "The index is {} days old - press r to refresh it (net).",
                    loaded.age.map(|a| a.as_secs() / 86_400).unwrap_or(0)
                )],
                false,
            );
        }
        Ok(Some(loaded)) => (loaded, Vec::new()),
        Ok(None) if !force => {
            return (
                vec!["No gamebus-gamedb index - press r to fetch it (net).".to_string()],
                false,
            );
        }
        // Confirmed without one. It still writes, because the user said so,
        // but the line above every page says what was not checked.
        Ok(None) => (
            index::empty(),
            vec![
                "No gamebus-gamedb index: nothing below was checked against the data \
                  set, so a page here may duplicate one it already carries."
                    .to_string(),
            ],
        ),
        Err(e) => return (vec![e], false),
    };

    match export::export(&report, &loaded, &out_dir) {
        Ok(summary) => {
            lines.extend(summary.lines);
            lines.push(format!(
                "{} page(s) written to {}, {} held back.",
                summary.written,
                out_dir.join("games").display(),
                summary.held
            ));
            if summary.written > 0 {
                lines.push(export::hint());
            }
            (lines, true)
        }
        Err(e) => (vec![format!("Export failed: {e}")], false),
    }
}

/// The pane's `d`: remember a new export directory in `setup.toml`.
pub(crate) fn tui_set_dir(dir: &str) -> (Vec<String>, bool) {
    let dir = dir.trim();
    if dir.is_empty() {
        return (
            vec!["Empty directory - nothing changed.".to_string()],
            false,
        );
    }
    let path = PathBuf::from(shellexpand(dir));
    if !path.is_absolute() {
        return (
            vec![format!(
                "'{}' is not an absolute path - the export has to know exactly where it writes.",
                path.display()
            )],
            false,
        );
    }
    let mut settings = config::load();
    settings.export_dir = Some(path.clone());
    match config::save(&settings) {
        Ok(()) => (
            vec![format!(
                "Export directory set to {} (saved in {}).",
                path.display(),
                config::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| config::CONFIG_NAME.to_string())
            )],
            true,
        ),
        Err(e) => (vec![e], false),
    }
}

/// Expand a leading `~` - the one thing a person typing a path into a text
/// field expects to work.
fn shellexpand(value: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match value.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_or_empty_directory_is_refused_before_anything_is_saved() {
        let (lines, ok) = tui_set_dir("   ");
        assert!(!ok);
        assert!(lines[0].contains("Empty directory"), "{lines:?}");
        let (lines, ok) = tui_set_dir("gamedb");
        assert!(!ok);
        assert!(lines[0].contains("not an absolute path"), "{lines:?}");
    }

    #[test]
    fn a_typed_tilde_means_the_home_directory() {
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(shellexpand("~/src/gamedb"), format!("{home}/src/gamedb"));
        assert_eq!(shellexpand("/srv/gamedb"), "/srv/gamedb");
        assert_eq!(shellexpand("~other/gamedb"), "~other/gamedb");
    }
}
