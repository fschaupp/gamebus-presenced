//! The Monitor tab: the live activity list and its detail pane.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::{field, App};

pub(super) fn render_monitor(f: &mut Frame, area: Rect, app: &mut App) {
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
