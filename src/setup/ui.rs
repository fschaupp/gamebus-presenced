//! The terminal interface: three views, a confirm modal, and an output log.
//!
//! Rendering is a pure function of [`App`]; nothing here does I/O beyond
//! drawing. Everything slow — probing, subprocesses, the 12 MB download —
//! happens on other tasks and arrives as a [`Msg`].

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use super::actions::{Action, Plan};
use super::paths::Target;
use super::status::{self, Health, Row, Status};
use super::umu_misses::{basis_label, confidence_label};
use crate::client::ActivityView;
use crate::umu_report::{Miss, VerificationState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Status,
    Monitor,
    /// The umu-database miss stash (S9b): what the daemon collected, what
    /// verification made of it. The pane drives the flows on explicit
    /// keypresses — `v` fetches+verifies (network, and the footer says so),
    /// `a` assigns an id (collision-checked before it saves), `s` corrects
    /// the store guess, `d` dismisses/restores an entry.
    Misses,
}

impl View {
    /// Tab-bar order; `Tab` cycles it.
    pub const ALL: [View; 3] = [View::Status, View::Monitor, View::Misses];

    pub fn title(self) -> &'static str {
        match self {
            View::Status => "status",
            View::Monitor => "monitor",
            View::Misses => "umu misses",
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
    pub action: Action,
    pub plan: Plan,
    pub needs_root: bool,
    pub explanation: Vec<String>,
    pub scope: String,
    /// Whether the exact command list is currently on screen.
    pub details: bool,
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
            id_input: None,
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
        self.rows = status::rows(&status);
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
            // On a check row, Enter runs that row's remedy — the shortest path
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

    /// Replace the miss list, keeping the selection on the same entry (by
    /// stash key): the once-a-second refresh may reorder rows — a bump of
    /// `last_seen`, a new miss on top — and a bare index would silently
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
    }

    fn move_selection(&mut self, delta: isize) {
        let action_count = self.action_list().len();
        let (state, len) = match (self.view, self.focus) {
            (View::Monitor, _) => (&mut self.monitor, self.activities.len()),
            (View::Misses, _) => (&mut self.miss_list, self.misses.len()),
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
    /// Misses pane `v`: fetch the database dump and verify the stash.
    /// Network — the footer labels the key as such.
    UmuVerify,
    /// Misses pane, Enter in id-entry mode: collision-check `id` against
    /// the local database and save it on the entry when it survives.
    UmuAssign {
        key: String,
        id: String,
    },
    /// Misses pane `s`: cycle the selected entry's store correction.
    UmuStore {
        key: String,
    },
    /// Misses pane `d`: dismiss the selected entry (parked, out of the
    /// exports) or restore it — a toggle, not a deletion.
    UmuDismiss {
        key: String,
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

    // Id-entry mode owns printable keys next: typing "status" into the id
    // field must not switch views or quit.
    if app.view == View::Misses && app.id_input.is_some() {
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
                match app.selected_miss_key() {
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

    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Intent::Quit,
        KeyCode::Char('q') | KeyCode::Esc => Intent::Quit,
        KeyCode::Char('r') => Intent::Refresh,
        KeyCode::Tab => {
            let idx = View::ALL.iter().position(|v| *v == app.view).unwrap_or(0);
            app.view = View::ALL[(idx + 1) % View::ALL.len()];
            Intent::None
        }
        // The misses pane's own verbs.
        KeyCode::Char('v') if app.view == View::Misses => Intent::UmuVerify,
        KeyCode::Char('a') if app.view == View::Misses => {
            if app.selected_miss_key().is_some() {
                // Prefill with the existing draft so a small correction is
                // an edit, not a retype.
                let current = app
                    .miss_list
                    .selected()
                    .and_then(|i| app.misses.get(i))
                    .and_then(|(_, m)| m.drafted_id.as_ref())
                    .map(|d| d.id.clone())
                    .unwrap_or_else(|| "umu-".to_string());
                app.id_input = Some(current);
            }
            Intent::None
        }
        KeyCode::Char('s') if app.view == View::Misses => match app.selected_miss_key() {
            Some(key) => Intent::UmuStore { key },
            None => Intent::None,
        },
        KeyCode::Char('d') if app.view == View::Misses => match app.selected_miss_key() {
            Some(key) => Intent::UmuDismiss { key },
            None => Intent::None,
        },
        // Pane focus, install target, and Enter act on the Status pane's
        // selections — which are invisible from the other views. Gated on
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
    }
    render_output(f, chunks[3], app);
    render_footer(f, chunks[4], app);

    if app.confirm.is_some() {
        render_confirm(f, app);
    }
}

/// Where am I, and what else is there — the answer Tab cycles through.
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
        Some(s) => status::overall(s),
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

fn render_status(f: &mut Frame, area: Rect, app: &mut App) {
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(area);

    // Hints carry the actionable half of a check, so they wrap rather than
    // being truncated at the pane edge like a List would do by default.
    const INDENT: usize = 14;
    let hint_width = (panes[0].width as usize).saturating_sub(INDENT + 3);

    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| {
            let mut lines = vec![Line::from(vec![
                Span::styled(
                    format!("{} ", row.health.marker()),
                    health_style(row.health),
                ),
                Span::styled(
                    format!("{:<11} ", row.label),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(row.value.clone()),
            ])];
            if let Some(hint) = &row.hint {
                for line in wrap(hint, hint_width) {
                    lines.push(Line::from(Span::styled(
                        format!("{:INDENT$}{line}", ""),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            ListItem::new(lines)
        })
        .collect();

    let checks = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(focus_title(" Checks ", app.focus == Focus::Checks)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(checks, panes[0], &mut app.checks);

    let action_items: Vec<ListItem> = app
        .action_list()
        .into_iter()
        .map(|a| ListItem::new(Line::from(a.label())))
        .collect();
    let actions = List::new(action_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(focus_title(" Actions ", app.focus == Focus::Actions)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(actions, panes[1], &mut app.actions);
}

/// Greedy word wrap. Words longer than the width are left alone rather than
/// broken — they are paths, and a broken path is worse than a long line.
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

fn focus_title(text: &str, focused: bool) -> Span<'_> {
    if focused {
        Span::styled(
            text.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw(text.to_string())
    }
}

fn render_monitor(f: &mut Frame, area: Rect, app: &mut App) {
    if app.activities.is_empty() {
        let text = match &app.status {
            Some(s) if s.bus.owned => "Nothing is playing.",
            Some(_) => "The daemon is not running.",
            None => "…",
        };
        f.render_widget(
            Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(" Monitor ")),
            area,
        );
        return;
    }

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let items: Vec<ListItem> = app
        .activities
        .iter()
        .map(|a| {
            ListItem::new(vec![
                Line::from(Span::styled(
                    a.display_name().to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!("  {} · pid {}", a.sources.join("+"), a.pid),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();
    if app.monitor.selected().is_none() {
        app.monitor.select(Some(0));
    }
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Activities "))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, panes[0], &mut app.monitor);

    let selected = app
        .monitor
        .selected()
        .and_then(|i| app.activities.get(i))
        .cloned()
        .unwrap_or_default();

    let mut detail = vec![
        field("Name", selected.display_name()),
        field("Kind", &selected.kind),
        field("Sources", &selected.sources.join(", ")),
        field("PID", &selected.pid.to_string()),
    ];
    for (label, value) in [
        ("Executable", &selected.executable),
        ("Details", &selected.details),
        ("State", &selected.state),
    ] {
        if !value.is_empty() {
            detail.push(field(label, value));
        }
    }
    if let Some(elapsed) = selected.elapsed() {
        detail.push(field("Playing", &elapsed));
    }
    if !selected.app_ids.is_empty() {
        let ids: Vec<String> = selected
            .app_ids
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        detail.push(field("AppIds", &ids.join(", ")));
    }
    detail.push(field("Object", &selected.path));

    f.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" Detail ")),
        panes[1],
    );
}

/// The umu-miss review pane: what the daemon collected, one entry per game,
/// with the verification verdict and drafted id once verification ran.
/// Its verbs act only on explicit keypresses the footer labels — `v` is the
/// one that reaches the network, and says so. Exporting stays a CLI
/// invocation: a submission wants a shell, not a raw-mode terminal.
fn render_misses(f: &mut Frame, area: Rect, app: &mut App) {
    if app.misses.is_empty() {
        let text = "No umu-database misses recorded.\n\n\
            The daemon writes one entry per game that umu launched without a\n\
            database entry (GAMEID=umu-0), together with the title it resolved.\n\
            Review them here — v fetches the database and verifies, s corrects\n\
            a store guess, a assigns an id by hand (collision-checked).\n\
            Export a submission from the shell (to stdout, or to a file if\n\
            you name one):\n\n\
            \u{20}   gamebus-setup umu-misses --export-md [file]";
        f.render_widget(
            Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL).title(" umu misses ")),
            area,
        );
        return;
    }

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let items: Vec<ListItem> = app
        .misses
        .iter()
        .map(|(_, m)| {
            let (glyph, style) = miss_state(m);
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(format!("{glyph} "), style),
                    Span::styled(
                        m.title.clone().unwrap_or_else(|| "(unresolved)".into()),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    format!(
                        "  {} · {} · seen {}",
                        m.effective_store(),
                        confidence_label(m.confidence),
                        m.last_seen
                    ),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();
    if app.miss_list.selected().is_none() {
        app.miss_list.select(Some(0));
    }
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" umu misses "))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, panes[0], &mut app.miss_list);

    let Some((_, m)) = app.miss_list.selected().and_then(|i| app.misses.get(i)) else {
        return;
    };
    let mut detail = vec![field("Title", m.title.as_deref().unwrap_or("(unresolved)"))];
    // The lines that change what a submission means go FIRST: at small
    // terminal sizes the pane clips from the bottom, and a clipped warning
    // is a warning that never happened.
    if let Some(when) = &m.dismissed {
        detail.push(Line::from(Span::styled(
            format!(
                "{:<11} {when} — out of the exports (d restores)",
                "Dismissed"
            ),
            Style::default().fg(Color::DarkGray),
        )));
    }
    if let Some(pr) = &m.possible_pr {
        detail.push(Line::from(vec![
            Span::styled(
                format!("{:<11} ", "Submitted?"),
                Style::default().fg(Color::Magenta),
            ),
            Span::styled(pr.clone(), Style::default().fg(Color::Magenta)),
        ]));
    }
    let store_line = match &m.store_override {
        Some(over) => format!("{over} (corrected by you; daemon guessed {})", m.store),
        None => format!("{} (guessed — s cycles)", m.store),
    };
    detail.extend([
        field("Store", &store_line),
        field("Codename", m.codename.as_deref().unwrap_or("-")),
        field("Reported", &m.umu_id),
    ]);
    if let Some(source) = &m.title_source {
        detail.push(field(
            "Resolved by",
            &format!("{source} ({} confidence)", confidence_label(m.confidence)),
        ));
    }
    if let Some(exe) = &m.executable {
        detail.push(field("Executable", exe));
    }
    detail.push(field(
        "Seen",
        &format!("{} – {}", m.first_seen, m.last_seen),
    ));
    match &m.verification {
        Some(v) => {
            let verdict = match v.state {
                VerificationState::AlreadyInDatabase => format!(
                    "already in the database as {} — the launcher missed, not the database",
                    v.umu_id.as_deref().unwrap_or("?")
                ),
                VerificationState::CrossStoreId => format!(
                    "known under another store as {}",
                    v.umu_id.as_deref().unwrap_or("?")
                ),
                VerificationState::ConfirmedMissing => "missing from the database".into(),
            };
            detail.push(field(
                "Verified",
                &format!("{verdict} (checked {})", v.checked),
            ));
            if let Some(note) = &v.note {
                detail.push(field("Note", note));
            }
        }
        None => detail.push(field("Verified", "not yet — press v to fetch + verify")),
    }
    if let Some(d) = &m.drafted_id {
        detail.push(field(
            "Drafted id",
            &format!(
                "{} — from {}, collision-checked {}",
                d.id,
                basis_label(d.basis),
                d.collision_checked
            ),
        ));
    }
    if let Some(buffer) = &app.id_input {
        detail.push(Line::from(vec![
            Span::styled(
                format!("{:<11} ", "Assign id"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{buffer}▏"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  ⏎ check+save · esc cancel",
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    f.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" Detail ")),
        panes[1],
    );
}

/// One glyph summarizing where an entry stands, for the list column.
fn miss_state(m: &Miss) -> (&'static str, Style) {
    if m.dismissed.is_some() {
        return ("✗", Style::default().fg(Color::DarkGray));
    }
    if m.possible_pr.is_some() {
        return ("↷", Style::default().fg(Color::Magenta));
    }
    match &m.verification {
        Some(v) => match v.state {
            VerificationState::AlreadyInDatabase => ("✓", Style::default().fg(Color::Green)),
            VerificationState::CrossStoreId => ("≈", Style::default().fg(Color::Cyan)),
            VerificationState::ConfirmedMissing if m.drafted_id.is_some() => {
                ("+", Style::default().fg(Color::Yellow))
            }
            VerificationState::ConfirmedMissing => ("∅", Style::default().fg(Color::Yellow)),
        },
        None => ("·", Style::default().fg(Color::DarkGray)),
    }
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
    let keys = match app.view {
        View::Status => {
            "↑↓ select · ←→ pane · ⏎ run · u/s target · tab next view · r refresh · q quit"
        }
        View::Monitor => "↑↓ select · tab next view · r refresh · q quit",
        // The export hint lives in the verify output and the empty state —
        // this line carries the pane's own verbs.
        View::Misses if app.id_input.is_some() => "type the id · ⏎ check+save · esc cancel",
        View::Misses => "↑↓ · tab view · v verify (net) · a assign · s store · d dismiss · q quit",
    };
    f.render_widget(
        Paragraph::new(Span::styled(keys, Style::default().fg(Color::DarkGray))),
        area,
    );
}

fn render_confirm(f: &mut Frame, app: &App) {
    let Some(confirm) = &app.confirm else {
        return;
    };

    let width = 88.min(f.area().width.saturating_sub(4));
    let inner = width.saturating_sub(4) as usize;

    let mut lines = vec![
        Line::from(Span::styled(
            confirm.action.label(),
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

    let body: Vec<String> = if confirm.details {
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

    // How far the change reaches — the question behind the dialog.
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
            "Some steps need root — you will be asked to authenticate. \
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
        if confirm.details {
            "y to proceed · d for the plain summary · any other key to cancel"
        } else {
            "y to proceed · d for the exact commands · any other key to cancel"
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
            action: Action::Install(Target::User),
            plan: Plan::default(),
            needs_root: false,
            explanation: vec!["copy 3 programs".to_string()],
            scope: "Everything stays inside your home directory.".to_string(),
            details: false,
        });
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('y'))),
            Intent::ConfirmYes
        );
        // Enter must NOT confirm: it is the same key that opened the dialog,
        // so accepting it would let one key repeat run the action unread.
        assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), Intent::ConfirmNo);
        // Even quit does not escape the modal — it cancels it.
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
            action: Action::Install(Target::User),
            plan: Plan::default(),
            needs_root: false,
            explanation: vec!["copy 3 programs".to_string()],
            scope: "Everything stays inside your home directory.".to_string(),
            details: false,
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
        assert_eq!(app.view, View::Status);
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('q'))), Intent::Quit);
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
    fn status_pane_keys_are_inert_outside_the_status_view() {
        let mut app = app_with_rows();
        app.focus = Focus::Actions; // an Install row is highlighted underneath
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control")]);
        // Enter must NOT fire the hidden Status-pane action from the
        // read-only misses pane — nor may ←→/u/s mutate invisible state.
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

    #[test]
    fn id_entry_mode_owns_the_keyboard_and_commits_on_enter() {
        let mut app = app_with_rows();
        app.view = View::Misses;
        app.set_misses(vec![sample_miss("Control")]);
        handle_key(&mut app, key(KeyCode::Down));

        // 'a' opens the input, prefilled with "umu-".
        handle_key(&mut app, key(KeyCode::Char('a')));
        assert_eq!(app.id_input.as_deref(), Some("umu-"));
        // Printable keys type — including q and v, which must NOT quit or
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
                verification: None,
                drafted_id: None,
                possible_pr: None,
                store_override: None,
                dismissed: None,
            },
        )
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
