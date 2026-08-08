//! The Status tab: the check rows and the actions pane.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem};
use ratatui::Frame;

use super::{health_style, wrap, App, Focus};

pub(super) fn render_status(f: &mut Frame, area: Rect, app: &mut App) {
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
