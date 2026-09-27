//! The auth and audit tabs.
//!
//! Auth is the owner's view of MCP client identities: the ledger's state
//! and who initialised it, the policy in plain words, and every client on
//! record or waiting. Each change is an owner action and asks for the
//! passphrase in a prompt that echoes nothing. Audit is the ledger itself,
//! read-only, newest first.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::super::auth::{describe, ClientStatus, Entry, Origin, OwnerOp, Registration, Strategy};
use super::{field, App};

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

fn state_line(app: &App) -> Line<'static> {
    match &app.auth.state {
        None => Line::from(Span::styled(
            "Not initialised: MCP clients can read but not edit. Run `gamebus-setup auth init` in a terminal.",
            Style::default().fg(Color::Yellow),
        )),
        Some(Err(e)) => Line::from(Span::styled(
            format!("BLOCKED: {e}. Every edit is refused until `gamebus-setup auth reset-ledger`."),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Some(Ok(())) => Line::from(vec![
            Span::styled("Ledger verified ", Style::default().fg(Color::Green)),
            Span::raw(format!(
                "{} (head {}, {} entries)",
                app.auth.ledger_id.as_deref().map(short).unwrap_or("?"),
                app.auth.head.as_deref().map(short).unwrap_or("?"),
                app.auth.entries.len()
            )),
        ]),
    }
}

fn describe_op(op: &OwnerOp) -> String {
    match op {
        OwnerOp::Approve(name) => format!("approve {name}"),
        OwnerOp::Forget(name) => format!("forget {name}"),
        OwnerOp::SetSettings(s) => format!(
            "set registration to {} and record {}",
            registration(s.registration),
            strategy(s.strategy)
        ),
    }
}

fn registration(r: Registration) -> &'static str {
    match r {
        Registration::TrustOnFirstUse => "first use remembered",
        Registration::ExplicitApproval => "owner approval required",
    }
}

fn strategy(s: Strategy) -> &'static str {
    match s {
        Strategy::McpEdits => "MCP and CLI edits",
        Strategy::OptIn => "no edits",
        Strategy::Everything => "MCP, CLI and TUI edits",
    }
}

pub(super) fn render_auth(f: &mut Frame, area: Rect, app: &mut App) {
    let prompt = app.pass_input.is_some() as u16;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(3),
            Constraint::Length(prompt * 3),
        ])
        .split(area);

    let mut header = vec![state_line(app)];
    if let Some(origin) = &app.auth.initialised {
        header.push(field("Initialised", origin));
    }
    if let Some(s) = app.auth.settings {
        header.push(Line::from(vec![
            Span::raw(format!("Registration: {}", registration(s.registration))),
            Span::styled(" (m) ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("  Recorded: {}", strategy(s.strategy))),
            Span::styled(" (s)", Style::default().fg(Color::DarkGray)),
        ]));
    }
    header.push(field("Directory", &app.auth.dir));
    f.render_widget(Paragraph::new(header), rows[0]);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(rows[1]);

    if app.auth.clients.is_empty() {
        let text = if app.auth.settings.is_some() {
            "No MCP clients yet.\n\nA client appears here the first time it connects \
             and authenticates - remembered on first use, or waiting for your approval, \
             per the registration mode."
        } else {
            "No clients: nothing can be on record before the ledger exists."
        };
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).title(" clients ")),
            rows[1],
        );
    } else {
        let items: Vec<ListItem> = app
            .auth
            .clients
            .iter()
            .map(|c| {
                let (glyph, style) = match &c.status {
                    ClientStatus::Approved => ("✓", Style::default().fg(Color::Green)),
                    ClientStatus::Remembered => ("·", Style::default().fg(Color::Reset)),
                    ClientStatus::Waiting { .. } => ("?", Style::default().fg(Color::Yellow)),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{glyph} "), style),
                    Span::styled(
                        c.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}", c.fingerprint),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect();
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" clients "))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        f.render_stateful_widget(list, panes[0], &mut app.client_list);

        let detail: Vec<Line> = match app.selected_client() {
            None => vec![],
            Some(c) => {
                let mut lines = vec![field("Client", &c.name), field("Key", &c.fingerprint)];
                lines.push(match &c.status {
                    ClientStatus::Approved => field("Status", "approved by you"),
                    ClientStatus::Remembered => field(
                        "Status",
                        "remembered on first use - a approves it explicitly",
                    ),
                    ClientStatus::Waiting {
                        date,
                        from,
                        replaces,
                    } => field(
                        "Status",
                        &if *replaces {
                            format!(
                                "a NEW key, waiting since {date} (from {from}): it would replace \
                                 the one on record. A reinstall does this; if you did not, x \
                                 forgets the client instead."
                            )
                        } else {
                            format!("waiting for your approval since {date} (from {from})")
                        },
                    ),
                });
                lines
            }
        };
        f.render_widget(
            Paragraph::new(detail)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).title(" detail ")),
            panes[1],
        );
    }

    if let Some(p) = &app.pass_input {
        let text = Line::from(vec![
            Span::raw(format!("Passphrase to {}: ", describe_op(&p.op))),
            Span::styled(
                "•".repeat(p.buffer.chars().count()),
                Style::default().fg(Color::Cyan),
            ),
        ]);
        f.render_widget(
            Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" owner action "),
            ),
            rows[2],
        );
    }
}

fn who(e: &Entry) -> String {
    match (&e.actor.origin, &e.actor.name) {
        (Origin::Client, Some(n)) => n.clone(),
        (Origin::Owner, _) => "owner".into(),
        (_, Some(n)) => n.clone(),
        (origin, None) => format!("{origin:?}").to_lowercase(),
    }
}

pub(super) fn render_audit(f: &mut Frame, area: Rect, app: &mut App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(area);
    f.render_widget(Paragraph::new(state_line(app)), rows[0]);

    if app.auth.entries.is_empty() {
        f.render_widget(
            Paragraph::new("The ledger is empty or does not exist yet.")
                .block(Block::default().borders(Borders::ALL).title(" audit ")),
            rows[1],
        );
        return;
    }

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(rows[1]);

    let items: Vec<ListItem> = app
        .auth
        .entries
        .iter()
        .map(|(e, signed)| {
            let mark = if *signed { "◆" } else { " " };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{mark}{:>4} ", e.seq),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{} ", e.date)),
                Span::styled(
                    format!("{:<14} ", who(e)),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(describe(e).0),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" ledger (◆ owner-signed) "),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, panes[0], &mut app.audit_list);

    let detail = match app
        .audit_list
        .selected()
        .and_then(|i| app.auth.entries.get(i))
    {
        None => vec![],
        Some((e, signed)) => {
            let mut lines = vec![
                field("Entry", &format!("{} on {}", e.seq, e.date)),
                field("By", &who(e)),
            ];
            if let Some(k) = &e.actor.key {
                lines.push(field("Key", &super::super::auth::fingerprint(k)));
            }
            let chain: Vec<String> = e
                .actor
                .process
                .iter()
                .map(|p| match &p.exe {
                    Some(exe) => format!("{} ({exe})", p.name),
                    None => p.name.clone(),
                })
                .collect();
            if !chain.is_empty() {
                lines.push(field("Via", &chain.join(" <- ")));
            }
            lines.push(field("Signed", if *signed { "by the owner" } else { "no" }));
            let (what, facts) = describe(e);
            lines.push(Line::from(Span::styled(
                what,
                Style::default().add_modifier(Modifier::BOLD),
            )));
            for (label, value) in facts {
                lines.push(field(label, &value));
            }
            lines
        }
    };
    f.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" entry ")),
        panes[1],
    );
}
