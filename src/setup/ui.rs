//! The terminal interface: two views, a confirm modal, and an output log.
//!
//! Rendering is a pure function of [`App`]; nothing here does I/O beyond
//! drawing. Everything slow - probing, subprocesses, the 12 MB download -
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
use crate::client::ActivityView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Status,
    Monitor,
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

    fn move_selection(&mut self, delta: isize) {
        let action_count = self.action_list().len();
        let (state, len) = match (self.view, self.focus) {
            (View::Monitor, _) => (&mut self.monitor, self.activities.len()),
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

    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Intent::Quit,
        KeyCode::Char('q') | KeyCode::Esc => Intent::Quit,
        KeyCode::Char('r') => Intent::Refresh,
        KeyCode::Tab => {
            app.view = match app.view {
                View::Status => View::Monitor,
                View::Monitor => View::Status,
            };
            Intent::None
        }
        KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') => {
            app.focus = match app.focus {
                Focus::Checks => Focus::Actions,
                Focus::Actions => Focus::Checks,
            };
            Intent::None
        }
        KeyCode::Char('u') => {
            app.target = Target::User;
            app.target_pinned = true;
            Intent::None
        }
        KeyCode::Char('s') => {
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
        KeyCode::Enter => match app.selected_action() {
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
            Constraint::Min(8),
            Constraint::Length(8),
            Constraint::Length(1),
        ])
        .split(f.area());

    render_header(f, chunks[0], app);
    match app.view {
        View::Status => render_status(f, chunks[1], app),
        View::Monitor => render_monitor(f, chunks[1], app),
    }
    render_output(f, chunks[2], app);
    render_footer(f, chunks[3], app);

    if app.confirm.is_some() {
        render_confirm(f, app);
    }
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
            "↑↓ select · ←→ pane · ⏎ run · u/s target · tab monitor · r refresh · q quit"
        }
        View::Monitor => "↑↓ select · tab status · r refresh · q quit",
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
    fn tab_toggles_the_view_and_q_quits() {
        let mut app = app_with_rows();
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.view, View::Monitor);
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.view, View::Status);
        assert_eq!(handle_key(&mut app, key(KeyCode::Char('q'))), Intent::Quit);
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
