//! The terminal interface: three views, a confirm modal, and an output log.
//!
//! Rendering is a pure function of [`App`]; nothing here does I/O beyond
//! drawing. Everything slow - probing, subprocesses, the 12 MB download -
//! happens on other tasks and arrives as a [`Msg`].

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, ListState, Paragraph};
use ratatui::Frame;

use super::actions::{Action, Plan};
use super::auth::{AuthView, ClientRow, ClientStatus, OwnerOp, Registration, Strategy};
use super::gamedb::{GamedbFilter, GamedbRow, GamedbView};
use super::paths::Target;
use super::status::{Health, Row, Status};
use super::umu_misses::PickCandidate;
use crate::client::ActivityView;
use crate::umu_report::Miss;

mod auth;
mod gamedb;
mod misses;
mod monitor;
mod status;

use self::auth::{render_audit, render_auth};
use self::gamedb::render_gamedb;
use self::misses::render_misses;
use self::monitor::render_monitor;
use self::status::render_status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Status,
    Monitor,
    /// The identity-miss stash: what the daemon collected - umu misses
    /// and launcher launches without a store identity - and what
    /// verification made of it. The pane drives the flows on explicit
    /// keypresses - `v` fetches+verifies (network, and the footer says so),
    /// `o` looks the title up at its store (network, labelled likewise),
    /// `a` assigns an id (collision-checked before it saves), `p` picks the
    /// matching entry from the local database or the Heroic libraries,
    /// `s` corrects the store guess, `t` corrects the title,
    /// `d` dismisses/restores an entry, `u` promotes a umu miss into the
    /// umu-database pipeline (or takes the promotion back).
    Misses,
    /// The same stash, folded into gamebus-gamedb pages: one row per GAME
    /// rather than per launch identity, checked against what the data set
    /// already publishes. `r` refreshes that index (network, and the footer
    /// says so), `e` writes the ready pages, `d` sets where they go.
    ///
    /// It also carries the misses pane's matchup verbs, acting on the
    /// selected game's representative stash entry - `p`, `o`, `t`, `s`, `a`,
    /// and `x` for dismiss, because `d` is the directory here. This is the
    /// tab where you find out an identity is wrong.
    Gamedb,
    /// MCP client identities and the ledger's policy: who initialised it,
    /// which clients are on record or waiting, what gets recorded. Every
    /// change here is an owner action and asks for the passphrase.
    Auth,
    /// The ledger itself, read-only: who changed what, from which process,
    /// and what it replaced.
    Audit,
}

impl View {
    /// Tab-bar order; `Tab` cycles it.
    pub const ALL: [View; 6] = [
        View::Status,
        View::Monitor,
        View::Misses,
        View::Gamedb,
        View::Auth,
        View::Audit,
    ];

    /// Whether this view's keys correct a stash entry. Both stash views do,
    /// so the text-entry and pick modes are gated on this rather than on one
    /// view - a title typed from the gamedb tab has to land somewhere.
    pub fn edits_misses(self) -> bool {
        matches!(self, View::Misses | View::Gamedb)
    }

    pub fn title(self) -> &'static str {
        match self {
            View::Status => "status",
            View::Monitor => "monitor",
            View::Misses => "identity misses",
            View::Gamedb => "gamedb",
            View::Auth => "auth",
            View::Audit => "audit",
        }
    }
}

/// Which pane has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Checks,
    Actions,
}

/// A pending confirmation.
///
/// Carries both layers: `explanation` is what the change means, `plan` is what
/// it literally runs. The plain layer is shown first because most people want
/// to know what happens to their machine, not which flags `install` gets; the
/// exact list is one keypress away because some people want exactly that, and
/// hiding it from them would make the tool untrustworthy.
pub struct Confirm {
    /// The install action this dialog is about, when it is about one. `None`
    /// for a confirmation that runs an [`Intent`] instead - see `on_yes`.
    pub action: Option<Action>,
    pub plan: Plan,
    pub needs_root: bool,
    pub explanation: Vec<String>,
    pub scope: String,
    /// Whether the exact command list is currently on screen.
    pub details: bool,
    /// When set, `y` fires this intent rather than running `action`'s plan.
    /// The gamedb pane's export uses it to ask before writing pages against
    /// an index too old to be trusted.
    pub on_yes: Option<Intent>,
    /// The dialog's heading, for a confirmation with no action to name it.
    pub label: Option<String>,
}

impl Confirm {
    /// What the dialog calls itself.
    pub fn heading(&self) -> String {
        match (&self.label, self.action) {
            (Some(label), _) => label.clone(),
            (None, Some(action)) => action.label(),
            (None, None) => "Confirm".to_string(),
        }
    }
}

/// The misses pane's pick mode: candidates for one miss, waiting for the
/// user to choose (or Esc out). The candidates are kind-tagged - database
/// rows, Heroic library identities, online hits - and Enter dispatches per
/// kind; the list renders them in labelled sections.
pub struct Pick {
    /// The stash key the candidates were fetched for - the pick lands on
    /// this entry, never on whatever the selection moved to since.
    pub key: String,
    /// Never empty: the binary logs "no matches" instead of opening the mode.
    pub candidates: Vec<PickCandidate>,
    pub selected: usize,
    /// A warning when the candidates came from an aging fetch cache - the
    /// footer then advertises `v` as the way out.
    pub stale: Option<String>,
}

/// A passphrase being typed for one owner action. Never echoed; the buffer
/// is dropped the moment Enter or Esc ends the mode.
pub struct PassPrompt {
    pub op: OwnerOp,
    pub buffer: String,
}

/// A passphrase in flight to the owner action. Its `Debug` never prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct Passphrase(pub String);

impl std::fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Passphrase(<hidden>)")
    }
}

pub struct App {
    pub view: View,
    pub focus: Focus,
    pub target: Target,
    /// Set once the user picks a target explicitly, so an incoming probe does
    /// not silently move the install they are about to confirm.
    pub target_pinned: bool,
    pub status: Option<Status>,
    pub rows: Vec<Row>,
    pub checks: ListState,
    pub actions: ListState,
    pub activities: Vec<ActivityView>,
    pub monitor: ListState,
    /// The umu-miss stash as `(stash key, entry)`, newest first. Loaded off
    /// the render path like everything else and arriving as a message; the
    /// key is the row's identity, so a refresh that reorders the list can
    /// keep the selection on the same game.
    pub misses: Vec<(String, Miss)>,
    pub miss_list: ListState,
    /// While `Some`, the misses pane is in id-entry mode and printable keys
    /// land here instead of the keymap. Committed with Enter (which checks
    /// the id before anything is saved), cancelled with Esc.
    pub id_input: Option<String>,
    /// While `Some`, the misses pane is in title-entry mode (`t`) - same
    /// keyboard ownership as `id_input`. Starts empty; the input line shows
    /// the current effective title beside it.
    pub title_input: Option<String>,
    /// The gamedb pane's rows as currently shown: the same stash folded
    /// into one page per game, with what the published index made of each,
    /// narrowed by [`App::gamedb_filter`]. Loaded off the render path and
    /// arriving as a message, like the miss list.
    pub gamedb: Vec<GamedbRow>,
    /// Every gamedb row, unfiltered - what `f` narrows down from.
    pub gamedb_all: Vec<GamedbRow>,
    /// The pane's `f`: which rows [`App::gamedb`] shows.
    pub gamedb_filter: GamedbFilter,
    pub gamedb_list: ListState,
    /// The one-line index state the pane's header shows.
    pub gamedb_index: String,
    /// Whether an index is cached and fresh enough to export against. `e`
    /// asks before exporting when it is not.
    pub gamedb_index_ok: bool,
    /// Where an export would write, as configured.
    pub gamedb_dir: String,
    /// While `Some`, the gamedb pane is in directory-entry mode (`d`) and
    /// printable keys land here - same keyboard ownership as `id_input`.
    pub dir_input: Option<String>,
    /// While `Some`, the misses pane is in pick mode and the candidate list
    /// owns the keyboard: ↑↓/j/k choose, Enter records the pick, Esc
    /// cancels. Remembers the stash key it was opened for; a refresh that
    /// drops that miss cancels the mode (see [`App::set_misses`]).
    pub pick: Option<Pick>,
    /// The auth and audit tabs' model, loaded off the render path.
    pub auth: AuthView,
    pub client_list: ListState,
    pub audit_list: ListState,
    /// While `Some`, the auth tab is asking for the passphrase and owns the
    /// keyboard.
    pub pass_input: Option<PassPrompt>,
    pub output: Vec<Line<'static>>,
    pub confirm: Option<Confirm>,
    /// Set while a mutating action is running. Also the mutual-exclusion gate
    /// for starting another one, which is why a probe must never clear it.
    pub busy: Option<String>,
    /// Set while a status probe is in flight. Cosmetic only.
    pub probing: bool,
    pub spinner: usize,
    pub should_quit: bool,
}

impl Default for App {
    fn default() -> Self {
        let mut checks = ListState::default();
        checks.select(Some(0));
        let mut actions = ListState::default();
        actions.select(Some(0));
        Self {
            view: View::Status,
            focus: Focus::Checks,
            target: Target::User,
            target_pinned: false,
            status: None,
            rows: Vec::new(),
            checks,
            actions,
            activities: Vec::new(),
            monitor: ListState::default(),
            misses: Vec::new(),
            miss_list: ListState::default(),
            gamedb: Vec::new(),
            gamedb_all: Vec::new(),
            gamedb_filter: GamedbFilter::default(),
            gamedb_list: ListState::default(),
            gamedb_index: "loading…".to_string(),
            gamedb_index_ok: false,
            gamedb_dir: String::new(),
            dir_input: None,
            id_input: None,
            title_input: None,
            pick: None,
            auth: AuthView::default(),
            client_list: ListState::default(),
            audit_list: ListState::default(),
            pass_input: None,
            output: Vec::new(),
            confirm: None,
            busy: None,
            probing: false,
            spinner: 0,
            should_quit: false,
        }
    }
}

impl App {
    pub fn set_status(&mut self, status: Status) {
        self.rows = super::status::rows(&status);
        if !self.target_pinned {
            self.target = status.primary().target;
        }
        self.status = Some(status);
        if self.checks.selected().unwrap_or(0) >= self.rows.len() {
            self.checks.select(Some(0));
        }
    }

    /// The actions offered in the right-hand pane, for the current target.
    pub fn action_list(&self) -> Vec<Action> {
        vec![
            Action::Install(self.target),
            Action::EnableAutostart(self.target),
            Action::DisableAutostart(self.target),
            Action::Start,
            Action::Restart,
            Action::Stop,
            Action::FetchDetectable,
            Action::Uninstall(self.target),
        ]
    }

    /// What pressing Enter would do right now.
    pub fn selected_action(&self) -> Option<Action> {
        match self.focus {
            // On a check row, Enter runs that row's remedy - the shortest path
            // from "this is broken" to "fixed".
            Focus::Checks => self
                .checks
                .selected()
                .and_then(|i| self.rows.get(i))
                .and_then(|row| row.remedy),
            Focus::Actions => self
                .actions
                .selected()
                .and_then(|i| self.action_list().get(i).copied()),
        }
    }

    pub fn log(&mut self, line: impl Into<String>) {
        self.output.push(Line::from(line.into()));
        self.trim_output();
    }

    pub fn log_styled(&mut self, text: impl Into<String>, style: Style) {
        self.output
            .push(Line::from(Span::styled(text.into(), style)));
        self.trim_output();
    }

    fn trim_output(&mut self) {
        const MAX: usize = 200;
        if self.output.len() > MAX {
            self.output.drain(0..self.output.len() - MAX);
        }
    }

    /// The stash key of the currently selected miss, if any.
    pub fn selected_miss_key(&self) -> Option<String> {
        self.miss_list
            .selected()
            .and_then(|i| self.misses.get(i))
            .map(|(k, _)| k.clone())
    }

    /// The gamedb row under the cursor, if any.
    pub fn selected_gamedb_row(&self) -> Option<&GamedbRow> {
        self.gamedb_list.selected().and_then(|i| self.gamedb.get(i))
    }

    /// The stash entry a correction typed right now would land on: the
    /// selected entry on the misses pane, and on the gamedb pane - whose
    /// rows are games, not entries - that game's representative entry.
    pub fn edit_key(&self) -> Option<String> {
        match self.view {
            View::Gamedb => self
                .selected_gamedb_row()
                .map(|row| row.rep_key.clone())
                .filter(|key| !key.is_empty()),
            _ => self.selected_miss_key(),
        }
    }

    /// Move the selection off the current entry onto its list neighbor  -
    /// the one below, or the one above when the cursor sits on the last
    /// row. Used before an action that resorts the current entry away
    /// (dismiss), so the key-stable refresh follows the neighbor instead
    /// of trailing the acted-on entry to its new position.
    fn select_neighbor_miss(&mut self) {
        let Some(idx) = self.miss_list.selected() else {
            return;
        };
        if idx + 1 < self.misses.len() {
            self.miss_list.select(Some(idx + 1));
        } else if idx > 0 {
            self.miss_list.select(Some(idx - 1));
        }
    }

    /// Replace the miss list, keeping the selection on the same entry (by
    /// stash key): the once-a-second refresh may reorder rows - a bump of
    /// `last_seen`, a new miss on top - and a bare index would silently
    /// switch the detail pane to a different game mid-review.
    pub fn set_misses(&mut self, misses: Vec<(String, Miss)>) {
        let selected_key = self
            .miss_list
            .selected()
            .and_then(|i| self.misses.get(i))
            .map(|(k, _)| k.clone());
        self.misses = misses;
        if let Some(key) = selected_key {
            if let Some(idx) = self.misses.iter().position(|(k, _)| k == &key) {
                self.miss_list.select(Some(idx));
            }
        }
        // Pick mode is bound to one stash entry; if a refresh dropped it,
        // there is nothing left to pick for.
        if let Some(pick) = &self.pick {
            if !self.misses.iter().any(|(k, _)| k == &pick.key) {
                self.pick = None;
            }
        }
    }

    /// Replace the gamedb rows, keeping the selection on the same game by
    /// its fold key - the pane refreshes once a second, and a bare index
    /// would move the detail onto a different game mid-read.
    pub fn set_gamedb(&mut self, view: GamedbView) {
        let selected_key = self
            .gamedb_list
            .selected()
            .and_then(|i| self.gamedb.get(i))
            .map(|row| row.key.clone());
        self.gamedb_all = view.rows;
        self.gamedb_index = view.index;
        self.gamedb_index_ok = view.index_ok;
        self.gamedb_dir = view.export_dir;
        self.refilter_gamedb(selected_key);
    }

    /// The pane's `f`: narrow the rows to the next filter mode, keeping the
    /// selection on the same game where it is still shown.
    pub fn cycle_gamedb_filter(&mut self) {
        let selected_key = self
            .gamedb_list
            .selected()
            .and_then(|i| self.gamedb.get(i))
            .map(|row| row.key.clone());
        self.gamedb_filter = self.gamedb_filter.next();
        self.refilter_gamedb(selected_key);
    }

    /// Rebuild the shown rows from the unfiltered set, putting the
    /// selection back on `selected_key` when the filter still shows it,
    /// else on the first row. The pane's keys act on the selected game;
    /// without a selection they stay inert until the first keypress.
    fn refilter_gamedb(&mut self, selected_key: Option<String>) {
        let filter = self.gamedb_filter;
        self.gamedb = self
            .gamedb_all
            .iter()
            .filter(|row| filter.matches(row))
            .cloned()
            .collect();
        let idx = selected_key
            .and_then(|key| self.gamedb.iter().position(|row| row.key == key))
            .or_else(|| (!self.gamedb.is_empty()).then_some(0));
        self.gamedb_list.select(idx);
    }

    /// Replace the auth model, keeping the client selection on the same name.
    pub fn set_auth(&mut self, view: AuthView) {
        let selected = self.selected_client().map(|c| c.name.clone());
        self.auth = view;
        let at = selected
            .and_then(|n| self.auth.clients.iter().position(|c| c.name == n))
            .or((!self.auth.clients.is_empty()).then_some(0));
        self.client_list.select(at);
        if self.audit_list.selected().is_none() && !self.auth.entries.is_empty() {
            self.audit_list.select(Some(0));
        }
        if self
            .audit_list
            .selected()
            .is_some_and(|i| i >= self.auth.entries.len())
        {
            self.audit_list
                .select(self.auth.entries.len().checked_sub(1));
        }
    }

    pub fn selected_client(&self) -> Option<&ClientRow> {
        self.client_list
            .selected()
            .and_then(|i| self.auth.clients.get(i))
    }

    fn move_selection(&mut self, delta: isize) {
        let action_count = self.action_list().len();
        let (state, len) = match (self.view, self.focus) {
            (View::Monitor, _) => (&mut self.monitor, self.activities.len()),
            (View::Misses, _) => (&mut self.miss_list, self.misses.len()),
            (View::Gamedb, _) => (&mut self.gamedb_list, self.gamedb.len()),
            (View::Auth, _) => (&mut self.client_list, self.auth.clients.len()),
            (View::Audit, _) => (&mut self.audit_list, self.auth.entries.len()),
            (_, Focus::Checks) => (&mut self.checks, self.rows.len()),
            (_, Focus::Actions) => (&mut self.actions, action_count),
        };
        if len == 0 {
            return;
        }
        let current = state.selected().unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len as isize);
        state.select(Some(next as usize));
    }
}

/// What a key press asks for. Returned rather than acted on, so the event loop
/// keeps all the I/O and this stays testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    None,
    Quit,
    Refresh,
    Run(Action),
    ConfirmYes,
    ConfirmNo,
    /// The stash panes' `v` (misses and gamedb alike): fetch the database
    /// dump and verify the stash. Network - the footer labels the key as
    /// such.
    UmuVerify,
    /// An owner action from the auth tab, with the passphrase typed for it.
    AuthOwner {
        op: OwnerOp,
        passphrase: Passphrase,
    },
    /// Misses pane, Enter in id-entry mode: collision-check `id` against
    /// the local database and save it on the entry when it survives.
    UmuAssign {
        key: String,
        id: String,
    },
    /// Misses pane `p`: search the local database and the Heroic libraries
    /// for candidates matching the selected entry's title. Local files
    /// only - never the network.
    UmuPick {
        key: String,
    },
    /// Misses pane `o`: one online lookup at the entry's effective store.
    /// Network - the footer labels the key as such.
    UmuOnline {
        key: String,
    },
    /// Misses pane, Enter on a database candidate: record "this game IS
    /// that database entry" on the miss.
    UmuPickEntry {
        key: String,
        store: String,
        codename: String,
        umu_id: String,
    },
    /// Misses pane, Enter on a library or online candidate: write the store
    /// identity (codename override, plus the store when the pick names one)
    /// onto the annotation half. An identity, never a verdict.
    UmuSetIdentity {
        key: String,
        store: Option<String>,
        codename: String,
        /// Where the identity came from, for the outcome line.
        source: String,
    },
    /// Misses pane, Enter on an egs offer with no Windows build in it: the
    /// second request, this namespace's builds. Network, like `o` itself.
    UmuEgsBuilds {
        key: String,
        namespace: String,
    },
    /// Misses pane `s`: cycle the selected entry's store correction.
    UmuStore {
        key: String,
    },
    /// Misses pane, Enter in title-entry mode or on a GOG product record:
    /// write the title override onto the annotation half. A title, never a
    /// verdict.
    UmuSetTitle {
        key: String,
        title: String,
        /// Where the title came from, for the outcome line.
        source: String,
    },
    /// Misses pane `d`: dismiss the selected entry (parked, out of the
    /// exports) or restore it - a toggle, not a deletion.
    UmuDismiss {
        key: String,
    },
    /// Misses pane `u`: promote the selected umu miss into the umu-database
    /// pipeline, or take the promotion back - a toggle, like dismiss.
    /// Emitted only for umu misses: on a launcher launch the key answers in
    /// the status line instead (nothing to promote).
    UmuPromote {
        key: String,
    },
    /// gamedb pane `r`: fetch the published identity index. Network - the
    /// footer labels the key as such.
    GamedbFetch,
    /// gamedb pane `e`: write every ready page to the configured directory.
    /// `force` is set only by the confirm dialog an aging or absent index
    /// puts up first.
    GamedbExport {
        force: bool,
    },
    /// gamedb pane, Enter in directory-entry mode: remember where the
    /// export writes.
    GamedbSetDir {
        dir: String,
    },
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> Intent {
    // The modal owns the keyboard while it is up.
    if let Some(confirm) = app.confirm.as_mut() {
        return match key.code {
            // Enter deliberately does NOT confirm: it is also "run the selected
            // action", so accepting it here would let one key repeat carry you
            // from a highlighted Uninstall row straight through the dialog.
            KeyCode::Char('y') | KeyCode::Char('Y') => Intent::ConfirmYes,
            // Show the exact commands. Not a cancel: someone reaching for the
            // detail is deciding, not backing out.
            KeyCode::Char('d') | KeyCode::Char('D') => {
                confirm.details = !confirm.details;
                Intent::None
            }
            _ => Intent::ConfirmNo,
        };
    }

    // The passphrase prompt owns every key while it is up, and echoes none.
    if app.pass_input.is_some() {
        return match key.code {
            KeyCode::Esc => {
                app.pass_input = None;
                Intent::None
            }
            KeyCode::Backspace => {
                app.pass_input.as_mut().expect("checked").buffer.pop();
                Intent::None
            }
            KeyCode::Enter => {
                let prompt = app.pass_input.take().expect("checked");
                if prompt.buffer.is_empty() {
                    Intent::None
                } else {
                    Intent::AuthOwner {
                        op: prompt.op,
                        passphrase: Passphrase(prompt.buffer),
                    }
                }
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.pass_input = None;
                Intent::Quit
            }
            KeyCode::Char(c) => {
                app.pass_input.as_mut().expect("checked").buffer.push(c);
                Intent::None
            }
            _ => Intent::None,
        };
    }

    // Directory-entry mode owns printable keys on the gamedb pane, for the
    // same reason id entry does on the misses pane: a path with a q in it
    // must not quit.
    if app.view == View::Gamedb && app.dir_input.is_some() {
        let buffer = app.dir_input.as_mut().expect("checked");
        return match key.code {
            KeyCode::Esc => {
                app.dir_input = None;
                Intent::None
            }
            KeyCode::Backspace => {
                buffer.pop();
                Intent::None
            }
            KeyCode::Enter => {
                let dir = app.dir_input.take().expect("checked");
                if dir.trim().is_empty() {
                    Intent::None
                } else {
                    Intent::GamedbSetDir { dir }
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                buffer.push(c);
                Intent::None
            }
            KeyCode::Char('c') => Intent::Quit, // ctrl-c stays an exit
            _ => Intent::None,
        };
    }

    // Id-entry mode owns printable keys next: typing "status" into the id
    // field must not switch views or quit.
    if app.view.edits_misses() && app.id_input.is_some() {
        let buffer = app.id_input.as_mut().expect("checked");
        return match key.code {
            KeyCode::Esc => {
                app.id_input = None;
                Intent::None
            }
            KeyCode::Backspace => {
                buffer.pop();
                Intent::None
            }
            KeyCode::Enter => {
                let id = app.id_input.take().expect("checked");
                match app.edit_key() {
                    Some(key) if !id.trim().is_empty() => Intent::UmuAssign { key, id },
                    _ => Intent::None,
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                buffer.push(c);
                Intent::None
            }
            KeyCode::Char('c') => Intent::Quit, // ctrl-c stays an exit
            _ => Intent::None,
        };
    }

    // Title-entry mode owns printable keys the same way.
    if app.view.edits_misses() && app.title_input.is_some() {
        let buffer = app.title_input.as_mut().expect("checked");
        return match key.code {
            KeyCode::Esc => {
                app.title_input = None;
                Intent::None
            }
            KeyCode::Backspace => {
                buffer.pop();
                Intent::None
            }
            KeyCode::Enter => {
                let title = app.title_input.take().expect("checked");
                match app.edit_key() {
                    Some(key) if !title.trim().is_empty() => Intent::UmuSetTitle {
                        key,
                        title,
                        source: "you".into(),
                    },
                    _ => Intent::None,
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                buffer.push(c);
                Intent::None
            }
            KeyCode::Char('c') => Intent::Quit, // ctrl-c stays an exit
            _ => Intent::None,
        };
    }

    // Pick mode owns the keyboard likewise: the candidate list is a modal
    // choice, so the selection keys move through candidates and no pane verb
    // fires underneath it.
    if app.view.edits_misses() && app.pick.is_some() {
        let pick = app.pick.as_mut().expect("checked");
        return match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Intent::Quit,
            KeyCode::Esc => {
                app.pick = None;
                Intent::None
            }
            // The candidates may have come from a stale cache; v abandons
            // the pick and runs the pane's one network action, the same
            // fetch+verify it means everywhere else.
            KeyCode::Char('v') => {
                app.pick = None;
                Intent::UmuVerify
            }
            KeyCode::Down | KeyCode::Char('j') => {
                pick.selected = (pick.selected + 1) % pick.candidates.len();
                Intent::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                pick.selected = (pick.selected + pick.candidates.len() - 1) % pick.candidates.len();
                Intent::None
            }
            // Enter dispatches on the candidate's kind: a database row is a
            // verdict, a library or online hit is an identity, and an egs
            // offer without a Windows build fires the builds request. The
            // offer's namespace is never committed as a codename.
            KeyCode::Enter => {
                let pick = app.pick.take().expect("checked");
                match pick.candidates.get(pick.selected) {
                    Some(PickCandidate::Db(c)) => Intent::UmuPickEntry {
                        key: pick.key,
                        store: c.store.clone(),
                        codename: c.codename.clone(),
                        umu_id: c.umu_id.clone(),
                    },
                    Some(PickCandidate::Library(g)) => Intent::UmuSetIdentity {
                        key: pick.key,
                        store: Some(g.store.clone()),
                        codename: g.codename.clone(),
                        source: "your Heroic library".into(),
                    },
                    // A Lutris library pick commits exactly like a Heroic
                    // one: the identity the launcher itself installed under.
                    Some(PickCandidate::Lutris(g)) => Intent::UmuSetIdentity {
                        key: pick.key,
                        store: Some(g.store.clone()),
                        codename: g.codename.clone(),
                        source: "your Lutris library".into(),
                    },
                    Some(PickCandidate::GogProduct(p)) => Intent::UmuSetIdentity {
                        key: pick.key,
                        store: None,
                        codename: p.id.clone(),
                        source: "the GOG catalog".into(),
                    },
                    // The by-id record answers the opposite question: the
                    // codename was already right, the title was not.
                    Some(PickCandidate::GogById { id, title, .. }) => Intent::UmuSetTitle {
                        key: pick.key,
                        title: title.clone(),
                        source: format!("GOG product {id}"),
                    },
                    Some(PickCandidate::EgsOffer(o)) => match &o.windows_app_name {
                        Some(app_name) => Intent::UmuSetIdentity {
                            key: pick.key,
                            store: None,
                            codename: app_name.clone(),
                            source: "the egdata offer's Windows build".into(),
                        },
                        None => Intent::UmuEgsBuilds {
                            key: pick.key,
                            namespace: o.namespace.clone(),
                        },
                    },
                    Some(PickCandidate::EgsBuild(b)) => Intent::UmuSetIdentity {
                        key: pick.key,
                        store: None,
                        codename: b.app_name.clone(),
                        source: "the egdata builds list".into(),
                    },
                    None => Intent::None,
                }
            }
            _ => Intent::None,
        };
    }

    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Intent::Quit,
        KeyCode::Char('q') | KeyCode::Esc => Intent::Quit,
        // The gamedb pane's own verbs. `r` shadows the global refresh here:
        // the pane's own "bring it up to date" is the index fetch, and the
        // rows themselves refresh once a second anyway.
        KeyCode::Char('r') if app.view == View::Gamedb => Intent::GamedbFetch,
        KeyCode::Char('e') if app.view == View::Gamedb => {
            if app.gamedb_index_ok {
                Intent::GamedbExport { force: false }
            } else {
                // Without a trustworthy index nothing can say whether a page
                // duplicates one the data set already carries, which is the
                // one mistake that costs a reviewer real work. Ask first.
                app.confirm = Some(Confirm {
                    action: None,
                    plan: Plan::default(),
                    needs_root: false,
                    label: Some("Export pages without checking the data set".to_string()),
                    explanation: vec![
                        format!("gamebus-gamedb index: {}", app.gamedb_index),
                        "write every ready page anyway, unchecked".to_string(),
                    ],
                    scope: "A page that duplicates one the data set already has fails its \
                            lint, and consolidating the two is a reviewer's work. Press r \
                            to fetch the index (network) instead."
                        .to_string(),
                    details: false,
                    on_yes: Some(Intent::GamedbExport { force: true }),
                });
                Intent::None
            }
        }
        KeyCode::Char('d') if app.view == View::Gamedb => {
            // Prefilled with the directory in force, so changing it is an
            // edit rather than a retype.
            app.dir_input = Some(app.gamedb_dir.clone());
            Intent::None
        }
        // Narrow the list to what still needs work: all / gaps / umu / weak.
        KeyCode::Char('f') if app.view == View::Gamedb => {
            app.cycle_gamedb_filter();
            Intent::None
        }
        // `d` is taken here, so dismiss gets its own key. It parks the
        // representative entry only - the other launches folded into this
        // game are separate judgements.
        KeyCode::Char('x') if app.view == View::Gamedb => match app.edit_key() {
            Some(key) => Intent::UmuDismiss { key },
            None => Intent::None,
        },
        // The auth tab's owner actions. Each opens the passphrase prompt;
        // nothing changes until it is confirmed.
        KeyCode::Char('a') if app.view == View::Auth && app.auth.settings.is_some() => {
            if let Some(c) = app.selected_client() {
                if c.status != ClientStatus::Approved {
                    app.pass_input = Some(PassPrompt {
                        op: OwnerOp::Approve(c.name.clone()),
                        buffer: String::new(),
                    });
                }
            }
            Intent::None
        }
        KeyCode::Char('x') if app.view == View::Auth && app.auth.settings.is_some() => {
            if let Some(c) = app.selected_client() {
                if !matches!(
                    c.status,
                    ClientStatus::Waiting {
                        replaces: false,
                        ..
                    }
                ) {
                    app.pass_input = Some(PassPrompt {
                        op: OwnerOp::Forget(c.name.clone()),
                        buffer: String::new(),
                    });
                }
            }
            Intent::None
        }
        KeyCode::Char('m') if app.view == View::Auth => {
            if let Some(mut s) = app.auth.settings {
                s.registration = match s.registration {
                    Registration::TrustOnFirstUse => Registration::ExplicitApproval,
                    Registration::ExplicitApproval => Registration::TrustOnFirstUse,
                };
                app.pass_input = Some(PassPrompt {
                    op: OwnerOp::SetSettings(s),
                    buffer: String::new(),
                });
            }
            Intent::None
        }
        KeyCode::Char('s') if app.view == View::Auth => {
            if let Some(mut s) = app.auth.settings {
                s.strategy = match s.strategy {
                    Strategy::McpEdits => Strategy::Everything,
                    Strategy::Everything => Strategy::OptIn,
                    Strategy::OptIn => Strategy::McpEdits,
                };
                app.pass_input = Some(PassPrompt {
                    op: OwnerOp::SetSettings(s),
                    buffer: String::new(),
                });
            }
            Intent::None
        }
        KeyCode::Char('r') => Intent::Refresh,
        KeyCode::Tab => {
            let idx = View::ALL.iter().position(|v| *v == app.view).unwrap_or(0);
            app.view = View::ALL[(idx + 1) % View::ALL.len()];
            Intent::None
        }
        // Both stash tabs share `v`: the flows reachable from the gamedb
        // tab too (the assign refusal, the pick's stale note, the store
        // cycle's re-verify line) all say "press v", so the key has to
        // work wherever the sentence was read from.
        KeyCode::Char('v') if app.view.edits_misses() => Intent::UmuVerify,
        // The misses pane's own verbs.
        KeyCode::Char('a') if app.view.edits_misses() => {
            if let Some(key) = app.edit_key() {
                // Prefill with the existing draft so a small correction is
                // an edit, not a retype.
                let current = app
                    .misses
                    .iter()
                    .find(|(k, _)| *k == key)
                    .and_then(|(_, m)| m.drafted_id.as_ref())
                    .map(|d| d.id.clone())
                    .unwrap_or_else(|| "umu-".to_string());
                app.id_input = Some(current);
            }
            Intent::None
        }
        KeyCode::Char('t') if app.view.edits_misses() => {
            // Empty on purpose, not prefilled with the resolver's title: the
            // point of the verb is that the resolver got it wrong. The input
            // line shows the current effective title beside the buffer.
            if app.edit_key().is_some() {
                app.title_input = Some(String::new());
            }
            Intent::None
        }
        KeyCode::Char('p') if app.view.edits_misses() => match app.edit_key() {
            Some(key) => Intent::UmuPick { key },
            None => Intent::None,
        },
        KeyCode::Char('o') if app.view.edits_misses() => match app.edit_key() {
            Some(key) => Intent::UmuOnline { key },
            None => Intent::None,
        },
        KeyCode::Char('s') if app.view.edits_misses() => match app.edit_key() {
            Some(key) => Intent::UmuStore { key },
            None => Intent::None,
        },
        // Promotion is a umu-pipeline verb, so it only means something on a
        // umu miss; on a launcher launch the key refuses in the status line
        // rather than silently doing nothing.
        KeyCode::Char('u') if app.view == View::Misses => {
            let selected = app
                .miss_list
                .selected()
                .and_then(|i| app.misses.get(i))
                .map(|(k, m)| {
                    (
                        k.clone(),
                        m.is_umu_miss(),
                        m.effective_title().unwrap_or("(unresolved)").to_string(),
                    )
                });
            match selected {
                Some((key, true, _)) => Intent::UmuPromote { key },
                Some((_, false, title)) => {
                    app.log(format!(
                        "{title} never went through umu - a launcher launch has nothing to promote into the umu database."
                    ));
                    Intent::None
                }
                None => Intent::None,
            }
        }
        KeyCode::Char('d') if app.view == View::Misses => match app.selected_miss_key() {
            Some(key) => {
                // Dismissing resorts the entry to the bottom of the list; the
                // key-stable selection would follow it there, stranding a
                // triage run (d, navigate all the way back, d, …). Hop to the
                // neighbor first - the refresh then keeps THAT entry selected
                // wherever the resort puts everything.
                app.select_neighbor_miss();
                Intent::UmuDismiss { key }
            }
            None => Intent::None,
        },
        // Pane focus, install target, and Enter act on the Status pane's
        // selections - which are invisible from the other views. Gated on
        // the view, or Enter in the read-only misses pane would fire
        // whatever Status row happened to be highlighted underneath.
        KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l')
            if app.view == View::Status =>
        {
            app.focus = match app.focus {
                Focus::Checks => Focus::Actions,
                Focus::Actions => Focus::Checks,
            };
            Intent::None
        }
        KeyCode::Char('u') if app.view == View::Status => {
            app.target = Target::User;
            app.target_pinned = true;
            Intent::None
        }
        KeyCode::Char('s') if app.view == View::Status => {
            app.target = Target::System;
            app.target_pinned = true;
            Intent::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.move_selection(1);
            Intent::None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.move_selection(-1);
            Intent::None
        }
        // The seam between the two tabs: this game, as the stash actually
        // recorded it. The misses selection is key-stable, so it stays put.
        KeyCode::Enter if app.view == View::Gamedb => {
            if let Some(key) = app.edit_key() {
                if let Some(idx) = app.misses.iter().position(|(k, _)| *k == key) {
                    app.view = View::Misses;
                    app.miss_list.select(Some(idx));
                }
            }
            Intent::None
        }
        KeyCode::Enter if app.view == View::Status => match app.selected_action() {
            Some(action) => Intent::Run(action),
            None => Intent::None,
        },
        _ => Intent::None,
    }
}

const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

fn health_style(health: Health) -> Style {
    match health {
        Health::Ok => Style::default().fg(Color::Green),
        Health::Warn => Style::default().fg(Color::Yellow),
        Health::Bad => Style::default().fg(Color::Red),
        Health::Unknown => Style::default().fg(Color::DarkGray),
    }
}

pub fn render(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(8),
            Constraint::Length(1),
        ])
        .split(f.area());

    render_header(f, chunks[0], app);
    render_tabs(f, chunks[1], app);
    match app.view {
        View::Status => render_status(f, chunks[2], app),
        View::Monitor => render_monitor(f, chunks[2], app),
        View::Misses => render_misses(f, chunks[2], app),
        View::Gamedb => render_gamedb(f, chunks[2], app),
        View::Auth => render_auth(f, chunks[2], app),
        View::Audit => render_audit(f, chunks[2], app),
    }
    render_output(f, chunks[3], app);
    render_footer(f, chunks[4], app);

    if app.confirm.is_some() {
        render_confirm(f, app);
    }
}

/// Where am I, and what else is there - the answer Tab cycles through.
fn render_tabs(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::raw(" ")];
    for (i, view) in View::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        }
        if *view == app.view {
            spans.push(Span::styled(
                view.title(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ));
        } else {
            spans.push(Span::styled(
                view.title(),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    spans.push(Span::styled(
        "   (tab)",
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let (health, summary) = match &app.status {
        Some(s) => super::status::overall(s),
        None => (Health::Unknown, "probing…".to_string()),
    };
    let mut spans = vec![
        Span::styled(
            "gamebus-presenced ",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} {summary}", health.marker()),
            health_style(health),
        ),
    ];
    let activity = app
        .busy
        .clone()
        .or_else(|| app.probing.then(|| "probing".to_string()));
    if let Some(what) = activity {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(
            format!("{} {what}", SPINNER[app.spinner % SPINNER.len()]),
            Style::default().fg(Color::Cyan),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Greedy word wrap. Words longer than the width are left alone rather than
/// broken - they are paths, and a broken path is worse than a long line.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn field(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<11} "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(value.to_string()),
    ])
}

fn render_output(f: &mut Frame, area: Rect, app: &App) {
    let height = area.height.saturating_sub(2) as usize;
    let start = app.output.len().saturating_sub(height);
    let visible: Vec<Line> = app.output[start..].to_vec();
    f.render_widget(
        Paragraph::new(visible).block(Block::default().borders(Borders::ALL).title(" Output ")),
        area,
    );
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(
        Paragraph::new(Span::styled(
            footer_keys(app),
            Style::default().fg(Color::DarkGray),
        )),
        area,
    );
}

/// The footer's key list, as the app's CURRENT state makes each key true.
/// A hint for a key that would do nothing right now - no row to act on, no
/// candidates to pick through - trains nobody to read the footer, so the
/// entry modes come first and the list states drop their entry verbs.
fn footer_keys(app: &App) -> &'static str {
    if app.pass_input.is_some() {
        return "type the passphrase (hidden) · ⏎ confirm · esc cancel";
    }
    // The entry modes edit the stash, so both stash tabs open them and both
    // draw the same line while one is up.
    if app.view.edits_misses() {
        if app.pick.as_ref().is_some_and(|p| p.stale.is_some()) {
            return "↑↓ choose · Enter pick · Esc cancel · v refresh (net)";
        }
        if app.pick.is_some() {
            return "↑↓ choose · Enter pick · Esc cancel";
        }
        if app.id_input.is_some() {
            return "type the id · ⏎ check+save · esc cancel";
        }
        if app.title_input.is_some() {
            return "type the title · ⏎ save · esc cancel";
        }
        if app.dir_input.is_some() {
            return "type the export directory · ⏎ save · esc cancel";
        }
    }
    match app.view {
        View::Status => {
            "↑↓ select · ←→ pane · ⏎ run · u/s target · tab next view · r refresh · q quit"
        }
        View::Monitor => "↑↓ select · tab next view · r refresh · q quit",
        // Nothing to act on: the matchup keys and the selection are inert,
        // so none of them is advertised. `v` stays - it fetches and then
        // honestly reports an empty stash.
        View::Misses if app.misses.is_empty() => "tab view · v verify (net) · q quit",
        View::Misses => {
            "↑↓ · tab view · v verify (net) · o lookup (net) · a assign · t title · p pick · s store · u promote (umu) · d dismiss · q quit"
        }
        // The filter is hiding every row: the entry verbs have no row to
        // act on, but `f` is the way back and must stay in the line.
        View::Gamedb if app.gamedb.is_empty() && !app.gamedb_all.is_empty() => {
            "tab view · f filter · r index (net) · v verify (net) · e export · d directory · q quit"
        }
        View::Gamedb if app.gamedb.is_empty() => {
            "tab view · r index (net) · v verify (net) · e export · d directory · q quit"
        }
        // The pane's own verbs, then the matchup verbs - which act on this
        // game's representative stash entry only.
        View::Auth if app.auth.settings.is_none() => "tab view · q quit",
        View::Auth => {
            "↑↓ · tab view · a approve · x forget · m registration mode · s what is recorded · q quit"
        }
        View::Audit => "↑↓ · tab view · q quit",
        View::Gamedb => {
            "↑↓ · tab view · ⏎ show entry · f filter · r index (net) · v verify (net) · \
             e export · d directory · on the entry: o lookup (net) · a assign · t title · \
             p pick · s store · x dismiss · q quit"
        }
    }
}

fn render_confirm(f: &mut Frame, app: &App) {
    let Some(confirm) = &app.confirm else {
        return;
    };

    let width = 88.min(f.area().width.saturating_sub(4));
    let inner = width.saturating_sub(4) as usize;

    let mut lines = vec![
        Line::from(Span::styled(
            confirm.heading(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    // Reserve room for the trailer, and say plainly how much was not shown
    // rather than letting the box swallow it.
    let trailer = if confirm.needs_root { 5 } else { 3 };
    let available = (f.area().height as usize)
        .saturating_sub(6 + trailer)
        .max(3);

    // A confirmation with no plan behind it (the gamedb export) has no
    // "exact commands" layer to open, so the detail toggle stays shut and
    // the trailer below never offers it.
    let details = confirm.details && confirm.action.is_some();
    let body: Vec<String> = if details {
        confirm
            .plan
            .all_steps()
            .map(|s| format!("  {}", s.summary()))
            .collect()
    } else {
        lines.push(Line::from("This will:"));
        confirm
            .explanation
            .iter()
            .map(|line| format!("  • {line}"))
            .collect()
    };

    if body.is_empty() {
        lines.push(Line::from("  (nothing to do)"));
    }
    let shown = body.len().min(available);
    for entry in &body[..shown] {
        for line in wrap(entry, inner) {
            lines.push(Line::from(line));
        }
    }
    if shown < body.len() {
        lines.push(Line::from(Span::styled(
            format!("  … and {} more", body.len() - shown),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // How far the change reaches - the question behind the dialog.
    if !confirm.details {
        lines.push(Line::from(""));
        for line in wrap(&confirm.scope, inner) {
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    if confirm.needs_root {
        lines.push(Line::from(""));
        for line in wrap(
            "Some steps need root - you will be asked to authenticate. \
             Only the file writes run privileged.",
            inner,
        ) {
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(Color::Yellow),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        match (details, confirm.action.is_some()) {
            (true, _) => "y to proceed · d for the plain summary · any other key to cancel",
            (false, true) => "y to proceed · d for the exact commands · any other key to cancel",
            (false, false) => "y to proceed · any other key to cancel",
        },
        Style::default().fg(Color::DarkGray),
    )));

    // Height is derived from the already-wrapped lines, so the trailer cannot
    // be pushed outside the box.
    let area = centred(f.area(), width, lines.len() as u16 + 2);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Confirm ")
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        area,
    );
}

fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventKind;

    use super::super::gamedb::RowState;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        }
    }

    fn app_with_rows() -> App {
        let rows = vec![
            Row {
                label: "Daemon",
                health: Health::Bad,
                value: "not running".to_string(),
                hint: None,
                remedy: Some(Action::Install(Target::User)),
            },
            Row {
                label: "GameMode",
                health: Health::Ok,
                value: "running".to_string(),
                hint: None,
                remedy: None,
            },
        ];
        App {
            rows,
            ..Default::default()
        }
    }

    #[test]
    fn enter_on_a_check_row_runs_that_rows_remedy() {
        let mut app = app_with_rows();
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Intent::Run(Action::Install(Target::User))
        );
    }

    #[test]
    fn enter_on_a_healthy_row_does_nothing() {
        let mut app = app_with_rows();
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), Intent::None);
    }

    #[test]
    fn selection_wraps_around_both_ends() {
        let mut app = app_with_rows();
        handle_key(&mut app, key(KeyCode::Up));
        assert_eq!(app.checks.selected(), Some(1));
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.checks.selected(), Some(0));
    }

    #[test]
    fn the_modal_swallows_every_key_and_only_y_proceeds() {
        let mut app = app_with_rows();
        app.confirm = Some(Confirm {
            action: Some(Action::Install(Target::User)),
            plan: Plan::default(),
            needs_root: false,
            explanation: vec!["copy 3 programs".to_string()],
            scope: "Everything stays inside your home directory.".to_string(),
            details: false,
            on_yes: None,
            label: None,
        });
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('y'))),
            Intent::ConfirmYes
        );
        // Enter must NOT confirm: it is the same key that opened the dialog,
        // so accepting it would let one key repeat run the action unread.
        assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), Intent::ConfirmNo);
        // Even quit does not escape the modal - it cancels it.
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('q'))),
            Intent::ConfirmNo
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('n'))),
            Intent::ConfirmNo
        );
    }

    /// Asking to see the exact commands must not read as backing out.
    #[test]
    fn d_toggles_the_detail_layer_instead_of_cancelling() {
        let mut app = app_with_rows();
        app.confirm = Some(Confirm {
            action: Some(Action::Install(Target::User)),
            plan: Plan::default(),
            needs_root: false,
            explanation: vec!["copy 3 programs".to_string()],
            scope: "Everything stays inside your home directory.".to_string(),
            details: false,
            on_yes: None,
            label: None,
        });

        assert_eq!(handle_key(&mut app, key(KeyCode::Char('d'))), Intent::None);
        assert!(app.confirm.as_ref().unwrap().details, "detail did not open");
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('d'))), Intent::None);
        assert!(
            !app.confirm.as_ref().unwrap().details,
            "detail did not close"
        );
        // And confirming still works from either layer.
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('y'))),
            Intent::ConfirmYes
        );
    }

    #[test]
    fn switching_target_changes_which_install_the_actions_refer_to() {
        let mut app = app_with_rows();
        app.focus = Focus::Actions;
        handle_key(&mut app, key(KeyCode::Char('s')));
        assert_eq!(app.selected_action(), Some(Action::Install(Target::System)));
        handle_key(&mut app, key(KeyCode::Char('u')));
        assert_eq!(app.selected_action(), Some(Action::Install(Target::User)));
    }

    #[test]
    fn tab_cycles_the_views_and_q_quits() {
        let mut app = app_with_rows();
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.view, View::Monitor);
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.view, View::Misses);
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.view, View::Gamedb);
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.view, View::Auth);
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.view, View::Audit);
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.view, View::Status);
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('q'))), Intent::Quit);
    }

    fn sample_gamedb_row(title: &str, state: RowState) -> GamedbRow {
        GamedbRow {
            key: format!("steam-{}", title.len()),
            title: title.to_string(),
            file: format!("{}.toml", title.to_lowercase()),
            state,
            id: Some("steam-870780".into()),
            stores: vec!["egs/Calluna".into()],
            exes: vec!["Control_DX12.exe".into()],
            steam: Some(870780),
            note: None,
            entries: 9,
            entry_keys: vec![format!("egs:{title}")],
            rep_key: format!("egs:{title}"),
            gap: false,
            umu: false,
            weak: false,
            launcher: None,
        }
    }

    fn gamedb_view(index_ok: bool) -> GamedbView {
        GamedbView {
            rows: vec![
                sample_gamedb_row("Control", RowState::Ready),
                sample_gamedb_row(
                    "Project Hospital",
                    RowState::InGamedb("steam-868360".into()),
                ),
            ],
            index: if index_ok {
                "release v2026.08.23, fetched today".to_string()
            } else {
                "no index cached - r fetches it (net)".to_string()
            },
            index_ok,
            export_dir: "/home/tester/Documents/gamebus-gamedb".to_string(),
        }
    }

    #[test]
    fn the_gamedb_verbs_fire_only_in_the_gamedb_view() {
        let mut app = app_with_rows();
        app.set_gamedb(gamedb_view(true));
        // Inert everywhere else - and `r` keeps its global meaning there.
        for view in [View::Status, View::Monitor, View::Misses] {
            app.view = view;
            assert_eq!(handle_key(&mut app, key(KeyCode::Char('e'))), Intent::None);
            assert_eq!(
                handle_key(&mut app, key(KeyCode::Char('r'))),
                Intent::Refresh
            );
            assert!(app.dir_input.is_none(), "d opened the field on {view:?}");
        }
        app.view = View::Gamedb;
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('r'))),
            Intent::GamedbFetch
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('e'))),
            Intent::GamedbExport { force: false }
        );
        assert!(app.confirm.is_none(), "a fresh index asked anyway");
    }

    /// The pane's `f` cycles all → gaps → umu → weak → all, narrows the
    /// shown rows to the matching predicate, and fires no intent - it is a
    /// view of the same rows, not an action.
    #[test]
    fn f_cycles_the_gamedb_filter_through_gaps_umu_and_weak() {
        let mut app = app_with_rows();
        app.view = View::Gamedb;
        let mut view = gamedb_view(true);
        view.rows = vec![
            GamedbRow {
                gap: true,
                ..sample_gamedb_row("Gappy", RowState::Ready)
            },
            GamedbRow {
                umu: true,
                ..sample_gamedb_row("Umuish", RowState::Ready)
            },
            GamedbRow {
                weak: true,
                ..sample_gamedb_row("Weakling", RowState::Ready)
            },
        ];
        app.set_gamedb(view);
        assert_eq!(app.gamedb.len(), 3, "all is the default");

        let titles = |app: &App| {
            app.gamedb
                .iter()
                .map(|r| r.title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('f'))), Intent::None);
        assert_eq!(app.gamedb_filter, GamedbFilter::Gaps);
        assert_eq!(titles(&app), ["Gappy"]);
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('f'))), Intent::None);
        assert_eq!(app.gamedb_filter, GamedbFilter::Umu);
        assert_eq!(titles(&app), ["Umuish"]);
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('f'))), Intent::None);
        assert_eq!(app.gamedb_filter, GamedbFilter::Weak);
        assert_eq!(titles(&app), ["Weakling"]);
        handle_key(&mut app, key(KeyCode::Char('f')));
        assert_eq!(app.gamedb_filter, GamedbFilter::All);
        assert_eq!(app.gamedb.len(), 3, "the cycle did not come back to all");

        // Every narrowed list has a selection, so the entry verbs keep
        // working; and a refresh keeps the filter in force.
        handle_key(&mut app, key(KeyCode::Char('f')));
        assert!(app.gamedb_list.selected().is_some());
        assert_eq!(app.edit_key().as_deref(), Some("egs:Gappy"));
        let mut view = gamedb_view(true);
        view.rows = vec![GamedbRow {
            gap: true,
            ..sample_gamedb_row("Gappy", RowState::Ready)
        }];
        app.set_gamedb(view);
        assert_eq!(app.gamedb_filter, GamedbFilter::Gaps);
        assert_eq!(titles(&app), ["Gappy"]);
    }

    /// `f` is a gamedb verb: everywhere else the key stays inert.
    #[test]
    fn the_filter_key_is_inert_outside_the_gamedb_view() {
        let mut app = app_with_rows();
        app.set_gamedb(gamedb_view(true));
        for view in [View::Status, View::Monitor, View::Misses] {
            app.view = view;
            assert_eq!(handle_key(&mut app, key(KeyCode::Char('f'))), Intent::None);
            assert_eq!(app.gamedb_filter, GamedbFilter::All, "{view:?} cycled it");
        }
    }

    /// `v` means the same thing on both stash tabs: the flows reachable
    /// from the gamedb tab too (the assign refusal, the pick's stale note,
    /// the store cycle's re-verify line) all say "press v", so the key has
    /// to fire there - and to stay inert on the read-only views.
    #[test]
    fn v_verifies_from_both_stash_tabs_and_is_inert_elsewhere() {
        let mut app = app_with_rows();
        app.set_gamedb(gamedb_view(true));
        app.set_misses(vec![sample_miss("Control")]);
        for view in [View::Misses, View::Gamedb] {
            app.view = view;
            assert_eq!(
                handle_key(&mut app, key(KeyCode::Char('v'))),
                Intent::UmuVerify,
                "{view:?}"
            );
        }
        for view in [View::Status, View::Monitor] {
            app.view = view;
            assert_eq!(
                handle_key(&mut app, key(KeyCode::Char('v'))),
                Intent::None,
                "{view:?}"
            );
        }
    }

    /// A footer key that would do nothing right now is a lie: with no row
    /// selected the entry verbs stay out of the line, and a filter hiding
    /// every row keeps `f` (the way back) while dropping them.
    #[test]
    fn the_footer_advertises_only_keys_that_work_right_now() {
        let mut app = app_with_rows();
        app.view = View::Misses;
        assert_eq!(footer_keys(&app), "tab view · v verify (net) · q quit");
        app.set_misses(vec![sample_miss("Control")]);
        let misses = footer_keys(&app);
        assert!(misses.contains("v verify (net)"), "{misses}");
        assert!(misses.contains("u promote (umu)"), "{misses}");

        app.view = View::Gamedb;
        assert_eq!(
            footer_keys(&app),
            "tab view · r index (net) · v verify (net) · e export · d directory · q quit"
        );
        // A filter that matches nothing: f is the way out, the entry verbs
        // are not in the line.
        app.gamedb_filter = GamedbFilter::Umu;
        let mut view = gamedb_view(true);
        view.rows = vec![sample_gamedb_row("Control", RowState::Ready)];
        app.set_gamedb(view);
        assert!(
            app.gamedb.is_empty(),
            "the fixture row should not match the umu filter"
        );
        let keys = footer_keys(&app);
        assert!(keys.contains("f filter"), "{keys}");
        assert!(!keys.contains("on the entry"), "{keys}");
        // Unfiltered, the full verb set is back - v included.
        app.gamedb_filter = GamedbFilter::All;
        app.set_gamedb(gamedb_view(true));
        let keys = footer_keys(&app);
        assert!(keys.contains("v verify (net)"), "{keys}");
        assert!(keys.contains("on the entry"), "{keys}");
    }

    /// Without an index nothing can say whether a page duplicates one the
    /// data set already has, so `e` must ask before writing.
    #[test]
    fn exporting_without_a_usable_index_asks_first() {
        let mut app = app_with_rows();
        app.view = View::Gamedb;
        app.set_gamedb(gamedb_view(false));
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('e'))), Intent::None);
        let confirm = app.confirm.as_ref().expect("no dialog");
        assert!(confirm.action.is_none());
        assert_eq!(
            confirm.on_yes,
            Some(Intent::GamedbExport { force: true }),
            "the dialog would run something else"
        );
        assert!(confirm.heading().contains("without checking"));
        // The modal still owns the keyboard, and y is still the only yes.
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('y'))),
            Intent::ConfirmYes
        );
    }

    #[test]
    fn directory_entry_mode_owns_the_keyboard_and_commits_on_enter() {
        let mut app = app_with_rows();
        app.view = View::Gamedb;
        app.set_gamedb(gamedb_view(true));

        // d opens the field, prefilled with the directory in force.
        handle_key(&mut app, key(KeyCode::Char('d')));
        assert_eq!(
            app.dir_input.as_deref(),
            Some("/home/tester/Documents/gamebus-gamedb")
        );
        // Printable keys type - including q, r and e, which must not quit,
        // fetch or export while the field is open.
        for c in ['q', 'r', 'e'] {
            assert_eq!(handle_key(&mut app, key(KeyCode::Char(c))), Intent::None);
        }
        assert_eq!(handle_key(&mut app, key(KeyCode::Backspace)), Intent::None);
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Intent::GamedbSetDir {
                dir: "/home/tester/Documents/gamebus-gamedbqr".into()
            }
        );
        assert!(app.dir_input.is_none(), "field stayed open after commit");

        // An empty buffer commits nothing; Esc cancels without an intent.
        app.gamedb_dir = String::new();
        handle_key(&mut app, key(KeyCode::Char('d')));
        assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), Intent::None);
        assert!(app.dir_input.is_none());
        handle_key(&mut app, key(KeyCode::Char('d')));
        assert_eq!(handle_key(&mut app, key(KeyCode::Esc)), Intent::None);
        assert!(app.dir_input.is_none());
        // And the pane is back to normal keys.
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('q'))), Intent::Quit);
    }

    #[test]
    fn the_gamedb_pane_owns_its_own_selection_and_keeps_it_across_a_refresh() {
        let mut app = app_with_rows();
        app.view = View::Gamedb;
        app.set_gamedb(gamedb_view(true));
        handle_key(&mut app, key(KeyCode::Down));
        let selected = |app: &App| {
            app.gamedb_list
                .selected()
                .and_then(|i| app.gamedb.get(i))
                .map(|r| r.title.clone())
        };
        assert_eq!(selected(&app).as_deref(), Some("Project Hospital"));
        // A refresh that reorders keeps the selection on the same game.
        let mut view = gamedb_view(true);
        view.rows.reverse();
        app.set_gamedb(view);
        assert_eq!(selected(&app).as_deref(), Some("Project Hospital"));
        // Wrap-around, like every other list here: the reversed refresh put
        // the selection on row 0, and ↑ from there wraps to the last row.
        handle_key(&mut app, key(KeyCode::Up));
        assert_eq!(app.gamedb_list.selected(), Some(1));
    }

    /// Gap 2: the gamedb tab shows a game whose identity is wrong and, until
    /// now, offered nothing to fix it with. The matchup verbs work here, and
    /// each one acts on the game's REPRESENTATIVE stash entry - the one the
    /// fold reads first, so correcting it corrects the page.
    #[test]
    fn the_matchup_verbs_work_on_the_gamedb_tab_and_act_on_the_representative_entry() {
        let mut app = app_with_rows();
        app.view = View::Gamedb;
        app.set_gamedb(gamedb_view(true));
        app.set_misses(vec![
            sample_miss("Control"),
            sample_miss("Project Hospital"),
        ]);
        // Row 0 is Control, whose representative entry is egs:Control.
        assert_eq!(app.edit_key().as_deref(), Some("egs:Control"));

        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('p'))),
            Intent::UmuPick {
                key: "egs:Control".into()
            }
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('o'))),
            Intent::UmuOnline {
                key: "egs:Control".into()
            }
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('s'))),
            Intent::UmuStore {
                key: "egs:Control".into()
            }
        );
        // `d` is the export directory here, so dismiss moved to `x`.
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('d'))), Intent::None);
        assert!(app.dir_input.is_some(), "d stopped opening the directory");
        handle_key(&mut app, key(KeyCode::Esc));
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('x'))),
            Intent::UmuDismiss {
                key: "egs:Control".into()
            }
        );

        // And the verbs follow the selection: row 1 is a different game.
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('p'))),
            Intent::UmuPick {
                key: "egs:Project Hospital".into()
            }
        );
    }

    /// The text-entry modes are gated on "this view edits stash entries",
    /// not on the misses view: a title typed from the gamedb tab has to land
    /// on the representative entry rather than nowhere.
    #[test]
    fn the_title_and_id_fields_commit_against_the_representative_entry() {
        let mut app = app_with_rows();
        app.view = View::Gamedb;
        app.set_gamedb(gamedb_view(true));
        app.set_misses(vec![sample_miss("Control")]);

        handle_key(&mut app, key(KeyCode::Char('t')));
        assert_eq!(app.title_input.as_deref(), Some(""));
        // The field owns printable keys here too: e, r and q must not
        // export, fetch or quit while it is open.
        for c in ['P', 'e', 'r', 'q'] {
            assert_eq!(handle_key(&mut app, key(KeyCode::Char(c))), Intent::None);
        }
        for _ in 0..3 {
            handle_key(&mut app, key(KeyCode::Backspace));
        }
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Intent::UmuSetTitle {
                key: "egs:Control".into(),
                title: "P".into(),
                source: "you".into(),
            }
        );

        // `a` prefills from the entry's own draft, exactly as it does on the
        // misses tab - the entry is the same entry.
        handle_key(&mut app, key(KeyCode::Char('a')));
        assert_eq!(app.id_input.as_deref(), Some("umu-"));
        for c in ['8', '7'] {
            handle_key(&mut app, key(KeyCode::Char(c)));
        }
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Intent::UmuAssign {
                key: "egs:Control".into(),
                id: "umu-87".into()
            }
        );
    }

    /// Pick mode is bound to a stash entry, so it has to survive being
    /// opened from the gamedb tab - and Enter still commits the candidate.
    #[test]
    fn pick_mode_opens_and_commits_from_the_gamedb_tab_too() {
        let mut app = app_with_rows();
        app.view = View::Gamedb;
        app.set_gamedb(gamedb_view(true));
        app.set_misses(vec![sample_miss("Control")]);
        app.pick = Some(Pick {
            key: "egs:Control".into(),
            candidates: vec![sample_candidate("egs", "Calluna", "umu-870780")],
            selected: 0,
            stale: None,
        });
        // The mode owns the keyboard: q does not quit underneath it.
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('q'))), Intent::None);
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Intent::UmuPickEntry {
                key: "egs:Control".into(),
                store: "egs".into(),
                codename: "Calluna".into(),
                umu_id: "umu-870780".into(),
            }
        );
        assert!(app.pick.is_none());
    }

    /// Enter crosses the seam between the two tabs: this game, as the stash
    /// actually recorded it.
    #[test]
    fn enter_on_a_gamedb_row_jumps_to_the_entry_behind_it() {
        let mut app = app_with_rows();
        app.view = View::Gamedb;
        app.set_gamedb(gamedb_view(true));
        app.set_misses(vec![
            sample_miss("Brotato"),
            sample_miss("Control"),
            sample_miss("Project Hospital"),
        ]);
        handle_key(&mut app, key(KeyCode::Down)); // row 1: Project Hospital
        assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), Intent::None);
        assert_eq!(app.view, View::Misses);
        assert_eq!(
            app.selected_miss_key().as_deref(),
            Some("egs:Project Hospital")
        );

        // A row whose entry the stash no longer has leaves the view alone -
        // jumping to nothing would be worse than not jumping.
        app.view = View::Gamedb;
        app.set_misses(vec![sample_miss("Brotato")]);
        assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), Intent::None);
        assert_eq!(app.view, View::Gamedb);
    }

    #[test]
    fn the_misses_view_owns_the_selection_keys() {
        let mut app = app_with_rows();
        app.view = View::Misses;
        app.misses = vec![
            sample_miss("Control"),
            sample_miss("Far Cry Primal"),
            sample_miss("Brotato"),
        ];
        // No selection yet: the first ↓ lands on an entry and wraps cleanly.
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.miss_list.selected(), Some(1));
        handle_key(&mut app, key(KeyCode::Up));
        handle_key(&mut app, key(KeyCode::Up));
        assert_eq!(app.miss_list.selected(), Some(2), "wrap-around broke");
    }

    #[test]
    fn a_refresh_keeps_the_selection_on_the_same_game() {
        let mut app = app_with_rows();
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control"), sample_miss("Brotato")]);
        // No selection yet, so the first ↓ lands on index 1: Brotato.
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.miss_list.selected(), Some(1)); // Brotato

        // The refresh reorders (a new miss lands on top, Brotato moves):
        // the selection must follow Brotato, not stay at index 1.
        app.set_misses(vec![
            sample_miss("Hades II"),
            sample_miss("Brotato"),
            sample_miss("Control"),
        ]);
        let selected = app
            .miss_list
            .selected()
            .and_then(|i| app.misses.get(i))
            .map(|(k, _)| k.as_str());
        assert_eq!(selected, Some("egs:Brotato"), "selection lost its game");
    }

    #[test]
    fn dismissing_hops_the_selection_to_the_neighbor_not_the_sunk_entry() {
        let mut app = app_with_rows();
        app.view = View::Misses;
        app.set_misses(vec![
            sample_miss("Control"),
            sample_miss("Brotato"),
            sample_miss("Hades II"),
        ]);
        handle_key(&mut app, key(KeyCode::Down)); // first ↓ lands on index 1
        handle_key(&mut app, key(KeyCode::Up)); // index 0: Control

        // d dismisses Control, but the cursor must hop to Brotato - a triage
        // run (d, d, d) works top-down without re-navigating.
        let intent = handle_key(&mut app, key(KeyCode::Char('d')));
        assert_eq!(
            intent,
            Intent::UmuDismiss {
                key: "egs:Control".into()
            }
        );
        assert_eq!(app.selected_miss_key().as_deref(), Some("egs:Brotato"));

        // The refresh resorts the dismissed entry to the bottom; the
        // selection stays with Brotato at its new position.
        app.set_misses(vec![
            sample_miss("Brotato"),
            sample_miss("Hades II"),
            sample_miss("Control"),
        ]);
        assert_eq!(app.selected_miss_key().as_deref(), Some("egs:Brotato"));

        // On the last row there is no entry below: hop upward instead.
        app.miss_list.select(Some(2));
        handle_key(&mut app, key(KeyCode::Char('d')));
        assert_eq!(app.selected_miss_key().as_deref(), Some("egs:Hades II"));
    }

    #[test]
    fn status_pane_keys_are_inert_outside_the_status_view() {
        let mut app = app_with_rows();
        app.focus = Focus::Actions; // an Install row is highlighted underneath
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control")]);
        // Enter must NOT fire the hidden Status-pane action from the
        // read-only misses pane - nor may ←→/u/s mutate invisible state.
        assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), Intent::None);
        handle_key(&mut app, key(KeyCode::Char('s')));
        assert_eq!(
            app.target,
            Target::User,
            "target changed from a read-only view"
        );
        handle_key(&mut app, key(KeyCode::Left));
        assert_eq!(
            app.focus,
            Focus::Actions,
            "focus changed from a read-only view"
        );
        // Back on Status, Enter works as before.
        app.view = View::Status;
        assert!(matches!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Intent::Run(_)
        ));
    }

    #[test]
    fn the_misses_verbs_fire_only_in_the_misses_view() {
        let mut app = app_with_rows();
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('v'))), Intent::None);
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control")]);
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('v'))),
            Intent::UmuVerify
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('s'))),
            Intent::UmuStore {
                key: "egs:Control".into()
            }
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('d'))),
            Intent::UmuDismiss {
                key: "egs:Control".into()
            }
        );
    }

    /// `u` promotes only what can be promoted: a umu miss toggles, a
    /// launcher launch (empty umu id) gets a status line and no intent.
    #[test]
    fn u_promotes_a_umu_miss_and_refuses_a_launcher_launch() {
        let mut app = app_with_rows();
        // Inert outside the misses view (on Status it pins the target).
        app.view = View::Gamedb;
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('u'))), Intent::None);

        app.view = View::Misses;
        let mut launcher = sample_miss("Danger Scavenger");
        launcher.1.umu_id = String::new();
        app.set_misses(vec![sample_miss("Control"), launcher]);

        // First ↓ lands on index 1: the launcher launch. Refused, with a
        // status line saying why.
        handle_key(&mut app, key(KeyCode::Down));
        let logged = app.output.len();
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('u'))), Intent::None);
        assert_eq!(
            app.output.len(),
            logged + 1,
            "no status line for the refusal"
        );

        // On the umu miss the toggle fires.
        handle_key(&mut app, key(KeyCode::Up));
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('u'))),
            Intent::UmuPromote {
                key: "egs:Control".into()
            }
        );
    }

    #[test]
    fn id_entry_mode_owns_the_keyboard_and_commits_on_enter() {
        let mut app = app_with_rows();
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control")]);
        handle_key(&mut app, key(KeyCode::Down));

        // 'a' opens the input, prefilled with "umu-".
        handle_key(&mut app, key(KeyCode::Char('a')));
        assert_eq!(app.id_input.as_deref(), Some("umu-"));
        // Printable keys type - including q and v, which must NOT quit or
        // verify while the field is open.
        for c in ['8', '7', 'q', 'v'] {
            assert_eq!(handle_key(&mut app, key(KeyCode::Char(c))), Intent::None);
        }
        assert_eq!(handle_key(&mut app, key(KeyCode::Backspace)), Intent::None);
        assert_eq!(handle_key(&mut app, key(KeyCode::Backspace)), Intent::None);
        assert_eq!(app.id_input.as_deref(), Some("umu-87"));
        // Enter commits: the intent carries the entry's key and the id.
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Intent::UmuAssign {
                key: "egs:Control".into(),
                id: "umu-87".into()
            }
        );
        assert!(app.id_input.is_none(), "input stayed open after commit");

        // Esc cancels without an intent.
        handle_key(&mut app, key(KeyCode::Char('a')));
        assert_eq!(handle_key(&mut app, key(KeyCode::Esc)), Intent::None);
        assert!(app.id_input.is_none());
        // And the pane is back to normal keys.
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('q'))), Intent::Quit);
    }

    /// Follows the a-verb precedent exactly: the mode owns the keyboard,
    /// Enter commits, Esc cancels - but the buffer starts EMPTY (the point
    /// of the verb is that the resolver's title is wrong).
    #[test]
    fn title_entry_mode_owns_the_keyboard_and_commits_on_enter() {
        let mut app = app_with_rows();
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control")]);
        handle_key(&mut app, key(KeyCode::Down));

        handle_key(&mut app, key(KeyCode::Char('t')));
        assert_eq!(app.title_input.as_deref(), Some(""));
        // Printable keys type - including q, v and t itself.
        for c in ['P', 'q', 'v', 't'] {
            assert_eq!(handle_key(&mut app, key(KeyCode::Char(c))), Intent::None);
        }
        for _ in 0..3 {
            assert_eq!(handle_key(&mut app, key(KeyCode::Backspace)), Intent::None);
        }
        assert_eq!(app.title_input.as_deref(), Some("P"));
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Intent::UmuSetTitle {
                key: "egs:Control".into(),
                title: "P".into(),
                source: "you".into(),
            }
        );
        assert!(app.title_input.is_none(), "input stayed open after commit");

        // An empty buffer commits nothing; Esc cancels without an intent.
        handle_key(&mut app, key(KeyCode::Char('t')));
        assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), Intent::None);
        assert!(app.title_input.is_none());
        handle_key(&mut app, key(KeyCode::Char('t')));
        assert_eq!(handle_key(&mut app, key(KeyCode::Esc)), Intent::None);
        assert!(app.title_input.is_none());
        // And the pane is back to normal keys.
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('q'))), Intent::Quit);
    }

    fn sample_candidate(store: &str, codename: &str, umu_id: &str) -> PickCandidate {
        PickCandidate::Db(crate::umu_report::UmuEntry {
            title: "Control".into(),
            store: store.into(),
            codename: codename.into(),
            umu_id: umu_id.into(),
        })
    }

    #[test]
    fn v_inside_pick_mode_abandons_the_pick_and_fetches() {
        let mut app = app_with_rows();
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control")]);
        app.pick = Some(Pick {
            key: "egs:Control".into(),
            candidates: vec![sample_candidate("egs", "Calluna", "umu-870780")],
            selected: 0,
            stale: Some("Database cache is 9 days old - v refreshes it (net).".into()),
        });
        // The one network key means the same thing inside the mode: leave
        // the stale candidate list and fetch fresh.
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('v'))),
            Intent::UmuVerify
        );
        assert!(app.pick.is_none(), "pick mode survived the refresh");
    }

    #[test]
    fn p_asks_for_candidates_for_the_selected_miss() {
        let mut app = app_with_rows();
        // Inert outside the misses view.
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('p'))), Intent::None);
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control")]);
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('p'))),
            Intent::UmuPick {
                key: "egs:Control".into()
            }
        );
    }

    #[test]
    fn o_asks_for_an_online_lookup_only_in_the_misses_view() {
        let mut app = app_with_rows();
        // Inert outside the misses view.
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('o'))), Intent::None);
        app.view = View::Misses;
        // And inert without a selected entry.
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('o'))), Intent::None);
        app.set_misses(vec![sample_miss("Control")]);
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('o'))),
            Intent::UmuOnline {
                key: "egs:Control".into()
            }
        );
    }

    /// Enter must dispatch on the candidate's kind - a database row records
    /// a verdict, everything else an identity, and an egs offer without a
    /// Windows build fires the builds request instead of committing its
    /// (lowercase, wrong) namespace.
    #[test]
    fn enter_in_pick_mode_dispatches_per_candidate_kind() {
        use super::super::heroic_library::LibraryGame;
        use super::super::umu_misses::{EgsBuild, EgsOffer, GogProduct};

        let mut app = app_with_rows();
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control")]);

        let cases: Vec<(PickCandidate, Intent)> = vec![
            (
                sample_candidate("egs", "Calluna", "umu-870780"),
                Intent::UmuPickEntry {
                    key: "egs:Control".into(),
                    store: "egs".into(),
                    codename: "Calluna".into(),
                    umu_id: "umu-870780".into(),
                },
            ),
            (
                PickCandidate::Library(LibraryGame {
                    title: "Control".into(),
                    store: "egs".into(),
                    codename: "Calluna".into(),
                }),
                Intent::UmuSetIdentity {
                    key: "egs:Control".into(),
                    store: Some("egs".into()),
                    codename: "Calluna".into(),
                    source: "your Heroic library".into(),
                },
            ),
            // A Lutris library pick commits the same identity, from the
            // other launcher's own records.
            (
                PickCandidate::Lutris(crate::setup::lutris_library::LutrisGame {
                    name: "Control".into(),
                    slug: "control".into(),
                    store: "egs".into(),
                    codename: "Calluna".into(),
                    runner: None,
                    directory: None,
                }),
                Intent::UmuSetIdentity {
                    key: "egs:Control".into(),
                    store: Some("egs".into()),
                    codename: "Calluna".into(),
                    source: "your Lutris library".into(),
                },
            ),
            (
                PickCandidate::GogProduct(GogProduct {
                    id: "2049187585".into(),
                    title: "Control Ultimate Edition".into(),
                    product_type: "game".into(),
                }),
                Intent::UmuSetIdentity {
                    key: "egs:Control".into(),
                    store: None,
                    codename: "2049187585".into(),
                    source: "the GOG catalog".into(),
                },
            ),
            (
                // The by-id record corrects the TITLE - the codename it was
                // fetched by was already right.
                PickCandidate::GogById {
                    id: "1660194629".into(),
                    title: "Project Hospital".into(),
                    game_type: "game".into(),
                },
                Intent::UmuSetTitle {
                    key: "egs:Control".into(),
                    title: "Project Hospital".into(),
                    source: "GOG product 1660194629".into(),
                },
            ),
            (
                // The search hit already knew the Windows build.
                PickCandidate::EgsOffer(EgsOffer {
                    title: "Control".into(),
                    namespace: "calluna".into(),
                    offer_type: "BASE_GAME".into(),
                    windows_app_name: Some("Calluna".into()),
                }),
                Intent::UmuSetIdentity {
                    key: "egs:Control".into(),
                    store: None,
                    codename: "Calluna".into(),
                    source: "the egdata offer's Windows build".into(),
                },
            ),
            (
                // No Windows build: the builds request, NEVER the namespace.
                PickCandidate::EgsOffer(EgsOffer {
                    title: "Control".into(),
                    namespace: "calluna".into(),
                    offer_type: "BASE_GAME".into(),
                    windows_app_name: None,
                }),
                Intent::UmuEgsBuilds {
                    key: "egs:Control".into(),
                    namespace: "calluna".into(),
                },
            ),
            (
                PickCandidate::EgsBuild(EgsBuild {
                    app_name: "Calluna".into(),
                    label_name: "Live".into(),
                    platform: "Windows".into(),
                }),
                Intent::UmuSetIdentity {
                    key: "egs:Control".into(),
                    store: None,
                    codename: "Calluna".into(),
                    source: "the egdata builds list".into(),
                },
            ),
        ];
        for (candidate, expected) in cases {
            app.pick = Some(Pick {
                key: "egs:Control".into(),
                candidates: vec![candidate],
                selected: 0,
                stale: None,
            });
            assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), expected);
            assert!(app.pick.is_none(), "pick mode stayed open after commit");
        }
    }

    #[test]
    fn pick_mode_owns_the_keyboard_and_commits_the_chosen_candidate() {
        let mut app = app_with_rows();
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control")]);
        app.pick = Some(Pick {
            key: "egs:Control".into(),
            candidates: vec![
                sample_candidate("egs", "Calluna", "umu-870780"),
                sample_candidate("gog", "2049187585", "umu-870780"),
            ],
            selected: 0,
            stale: None,
        });
        // q must NOT quit while the list is up (v is the deliberate
        // exception - it abandons the pick to refresh, tested separately).
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('q'))), Intent::None);
        // ↓/j and ↑/k move, wrapping at both ends.
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.pick.as_ref().unwrap().selected, 1);
        handle_key(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.pick.as_ref().unwrap().selected, 0, "wrap-around broke");
        handle_key(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.pick.as_ref().unwrap().selected, 1, "wrap-around broke");
        // Enter commits the highlighted candidate for the remembered key.
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            Intent::UmuPickEntry {
                key: "egs:Control".into(),
                store: "gog".into(),
                codename: "2049187585".into(),
                umu_id: "umu-870780".into(),
            }
        );
        assert!(app.pick.is_none(), "pick mode stayed open after commit");

        // Esc cancels without an intent, and the pane keys are back.
        app.pick = Some(Pick {
            key: "egs:Control".into(),
            candidates: vec![sample_candidate("egs", "Calluna", "umu-870780")],
            selected: 0,
            stale: None,
        });
        assert_eq!(handle_key(&mut app, key(KeyCode::Esc)), Intent::None);
        assert!(app.pick.is_none());
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('q'))), Intent::Quit);
    }

    #[test]
    fn a_refresh_that_drops_the_picked_miss_cancels_pick_mode() {
        let mut app = app_with_rows();
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control"), sample_miss("Brotato")]);
        app.pick = Some(Pick {
            key: "egs:Control".into(),
            candidates: vec![sample_candidate("egs", "Calluna", "umu-870780")],
            selected: 0,
            stale: None,
        });
        // A refresh that keeps the miss keeps the mode.
        app.set_misses(vec![sample_miss("Brotato"), sample_miss("Control")]);
        assert!(app.pick.is_some());
        // One that drops it cancels - the pick has nothing to land on.
        app.set_misses(vec![sample_miss("Brotato")]);
        assert!(app.pick.is_none(), "pick mode outlived its miss");
    }

    fn sample_miss(title: &str) -> (String, Miss) {
        (
            format!("egs:{title}"),
            Miss {
                title: Some(title.to_string()),
                store: "egs".into(),
                codename: None,
                umu_id: "umu-0".into(),
                title_source: Some("heroic-config".into()),
                confidence: None,
                executable: None,
                first_seen: "2026-08-07".into(),
                last_seen: "2026-08-07".into(),
                launcher: None,
                launcher_name: None,
                launcher_dir: None,
                codename_source: None,
                runner: None,
                verification: None,
                drafted_id: None,
                possible_pr: None,
                fix: None,
                store_override: None,
                codename_override: None,
                codename_override_source: None,
                title_override: None,
                dismissed: None,
                umu_promoted: None,
            },
        )
    }

    fn auth_app() -> App {
        use super::super::auth::{ClientRow, Settings};
        let mut app = App {
            view: View::Auth,
            ..App::default()
        };
        app.set_auth(AuthView {
            state: Some(Ok(())),
            settings: Some(Settings::default()),
            clients: vec![
                ClientRow {
                    name: "beisl-compat".into(),
                    fingerprint: "aa".into(),
                    status: ClientStatus::Remembered,
                },
                ClientRow {
                    name: "agent".into(),
                    fingerprint: "bb".into(),
                    status: ClientStatus::Waiting {
                        date: "2026-09-27".into(),
                        from: "claude".into(),
                        replaces: false,
                    },
                },
            ],
            ..AuthView::default()
        });
        app
    }

    fn type_str(app: &mut App, text: &str) {
        for c in text.chars() {
            assert_eq!(handle_key(app, key(KeyCode::Char(c))), Intent::None);
        }
    }

    #[test]
    fn an_owner_action_asks_for_the_passphrase_and_owns_the_keyboard() {
        let mut app = auth_app();
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('a'))), Intent::None);
        assert!(app.pass_input.is_some());
        // q is a passphrase character now, not quit, and Tab does not
        // switch the view away from a half-typed passphrase.
        type_str(&mut app, "q");
        assert_eq!(handle_key(&mut app, key(KeyCode::Tab)), Intent::None);
        type_str(&mut app, "pw");
        assert_eq!(app.view, View::Auth);
        assert_eq!(app.pass_input.as_ref().unwrap().buffer, "qpw");
        match handle_key(&mut app, key(KeyCode::Enter)) {
            Intent::AuthOwner { op, passphrase } => {
                assert_eq!(op, OwnerOp::Approve("beisl-compat".into()));
                assert_eq!(passphrase.0, "qpw");
                assert!(!format!("{passphrase:?}").contains("qpw"), "Debug leaks it");
            }
            other => panic!("{other:?}"),
        }
        assert!(app.pass_input.is_none(), "the buffer is gone after Enter");
    }

    #[test]
    fn esc_abandons_the_prompt_and_an_empty_passphrase_does_nothing() {
        let mut app = auth_app();
        handle_key(&mut app, key(KeyCode::Char('s')));
        type_str(&mut app, "secret");
        assert_eq!(handle_key(&mut app, key(KeyCode::Esc)), Intent::None);
        assert!(app.pass_input.is_none());
        handle_key(&mut app, key(KeyCode::Char('m')));
        assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), Intent::None);
    }

    #[test]
    fn the_policy_keys_propose_the_next_setting() {
        use super::super::auth::Settings;
        let mut app = auth_app();
        handle_key(&mut app, key(KeyCode::Char('m')));
        assert_eq!(
            app.pass_input.as_ref().unwrap().op,
            OwnerOp::SetSettings(Settings {
                registration: Registration::ExplicitApproval,
                strategy: Strategy::McpEdits,
            })
        );
        app.pass_input = None;
        handle_key(&mut app, key(KeyCode::Char('s')));
        assert_eq!(
            app.pass_input.as_ref().unwrap().op,
            OwnerOp::SetSettings(Settings {
                registration: Registration::TrustOnFirstUse,
                strategy: Strategy::Everything,
            })
        );
    }

    #[test]
    fn forgetting_a_new_waiting_client_is_not_offered() {
        let mut app = auth_app();
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.selected_client().unwrap().name, "agent");
        handle_key(&mut app, key(KeyCode::Char('x')));
        assert!(app.pass_input.is_none(), "nothing on record to forget");
        handle_key(&mut app, key(KeyCode::Char('a')));
        assert_eq!(
            app.pass_input.as_ref().unwrap().op,
            OwnerOp::Approve("agent".into())
        );
    }

    #[test]
    fn before_init_the_auth_tab_offers_no_owner_actions() {
        let mut app = App {
            view: View::Auth,
            ..App::default()
        };
        for c in ['a', 'x', 'm', 's'] {
            handle_key(&mut app, key(KeyCode::Char(c)));
            assert!(app.pass_input.is_none(), "{c} opened a prompt");
        }
        assert_eq!(footer_keys(&app), "tab view · q quit");
    }

    fn draw(app: &mut App) -> String {
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn the_auth_tab_draws_the_prompt_as_dots_never_the_passphrase() {
        let mut app = auth_app();
        handle_key(&mut app, key(KeyCode::Char('a')));
        type_str(&mut app, "hunter22");
        let screen = draw(&mut app);
        assert!(
            screen.contains("••••••••"),
            "the prompt shows its length as dots"
        );
        assert!(
            !screen.contains("hunter22"),
            "the passphrase reached the screen"
        );
        assert!(screen.contains("approve beisl-compat"));
    }

    #[test]
    fn the_audit_tab_draws_a_real_ledger() {
        let paths = super::super::auth::test_paths("audit-draw");
        super::super::auth::fast_init(&paths, "pw");
        let mut app = App {
            view: View::Audit,
            ..App::default()
        };
        app.set_auth(super::super::auth::view(&paths));
        let screen = draw(&mut app);
        assert!(screen.contains("Ledger started"));
        assert!(
            !screen.contains("\"action\""),
            "raw JSON reached the audit tab"
        );
        assert!(screen.contains("Ledger verified"));
        app.view = View::Auth;
        assert!(draw(&mut app).contains("No MCP clients yet"));
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn hints_wrap_without_breaking_paths() {
        let lines = wrap("the daemon can only be started explicitly", 20);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| l.chars().count() <= 20), "{lines:?}");

        // A path longer than the pane stays intact on its own line: half a
        // path is worse than a line that overflows.
        let long = "/home/tester/.local/share/dbus-1/services/org.gamebus.Presence.v1.service";
        assert_eq!(wrap(long, 20), vec![long.to_string()]);
    }

    #[test]
    fn the_output_pane_keeps_only_the_recent_lines() {
        let mut app = App::default();
        for i in 0..500 {
            app.log(format!("line {i}"));
        }
        assert_eq!(app.output.len(), 200);
    }
}
