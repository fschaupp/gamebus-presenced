//! The gamedb tab: the candidate list, the detail pane, and the one-line
//! index header.
//!
//! The misses tab beside it shows the stash as the daemon recorded it - one
//! row per launch identity. This one shows the same stash as the data set
//! would carry it: one row per GAME, every store folded in, checked against
//! what gamebus-gamedb already publishes. The two disagree on purpose, and
//! the difference is exactly what a page contributes.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::super::gamedb::{GamedbRow, RowState};
use super::{field, App};

pub(super) fn render_gamedb(f: &mut Frame, area: Rect, app: &mut App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(3)])
        .split(area);
    render_header(f, rows[0], app);

    if app.gamedb.is_empty() {
        let text = "Nothing to contribute yet.\n\n\
            This tab folds the umu-miss stash into gamebus-gamedb pages: one\n\
            page per game, with every store it appeared on nested inside, and\n\
            the executables that identify it where no store does.\n\
            It is where the knowledge goes that umu-database declines - a game\n\
            that needs no protonfix is out of scope there, but its Epic app\n\
            name still belongs somewhere.\n\n\
            r fetches what the data set already carries (network), e writes\n\
            the ready pages, d says where they go.";
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).title(" gamedb ")),
            rows[1],
        );
        return;
    }

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(rows[1]);

    let items: Vec<ListItem> = app
        .gamedb
        .iter()
        .map(|row| {
            let (glyph, style) = row_state(row);
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(format!("{glyph} "), style),
                    Span::styled(
                        row.title.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    format!("  {}", summary(row)),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();
    if app.gamedb_list.selected().is_none() {
        app.gamedb_list.select(Some(0));
    }
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" pages "))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, panes[0], &mut app.gamedb_list);

    let Some(row) = app.gamedb_list.selected().and_then(|i| app.gamedb.get(i)) else {
        return;
    };
    let mut detail = vec![field("Title", &row.title)];
    // Why a page will or will not be written goes first: at small terminal
    // sizes the pane clips from the bottom, and a clipped hold-back reason
    // is a reason nobody read.
    match &row.state {
        RowState::InGamedb(id) => detail.push(Line::from(vec![
            Span::styled(
                format!("{:<11} ", "Held back"),
                Style::default().fg(Color::Green),
            ),
            Span::styled(
                format!("already in gamebus-gamedb as {id}"),
                Style::default().fg(Color::Green),
            ),
        ])),
        RowState::Incomplete(reason) => detail.push(Line::from(vec![
            Span::styled(
                format!("{:<11} ", "Held back"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(reason.clone(), Style::default().fg(Color::DarkGray)),
        ])),
        RowState::Ready => detail.push(field("File", &format!("games/{}", row.file))),
    }
    detail.push(field(
        "gamedb id",
        row.id
            .as_deref()
            .unwrap_or("none derivable - a minted id is a reviewer's call"),
    ));
    if let Some(appid) = row.steam {
        detail.push(field("Steam", &appid.to_string()));
    }
    if !row.stores.is_empty() {
        detail.push(field("Stores", &row.stores.join(", ")));
    }
    if !row.exes.is_empty() {
        detail.push(field("Executables", &row.exes.join(", ")));
    }
    detail.push(field(
        "Folded from",
        &format!(
            "{} stash {}",
            row.entries,
            if row.entries == 1 { "entry" } else { "entries" }
        ),
    ));
    if let Some(note) = &row.note {
        detail.push(field("Note", note));
    }
    if let Some(buffer) = &app.dir_input {
        detail.push(Line::from(vec![
            Span::styled(
                format!("{:<11} ", "Export to"),
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
                "  ⏎ save · esc cancel",
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

/// The two lines above the list: how much the index can be trusted, and
/// where an export would write.
fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let index_style = if app.gamedb_index_ok {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::Yellow)
    };
    let lines = vec![
        Line::from(vec![
            Span::styled(
                " gamebus-gamedb index: ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(app.gamedb_index.clone(), index_style),
        ]),
        Line::from(vec![
            Span::styled(
                " Export to:            ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(app.gamedb_dir.clone()),
        ]),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

/// The gray line under each title: what identifies the game.
fn summary(row: &GamedbRow) -> String {
    let mut parts = Vec::new();
    if let Some(id) = &row.id {
        parts.push(id.clone());
    }
    parts.extend(row.stores.iter().cloned());
    parts.extend(row.exes.iter().cloned());
    if parts.is_empty() {
        "nothing identifies it yet".to_string()
    } else {
        parts.join(" · ")
    }
}

/// One glyph summarizing where a page stands, for the list column.
fn row_state(row: &GamedbRow) -> (&'static str, Style) {
    match &row.state {
        RowState::InGamedb(_) => (
            "✓",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::DIM),
        ),
        RowState::Ready => ("●", Style::default().fg(Color::Cyan)),
        RowState::Incomplete(_) => ("○", Style::default().fg(Color::DarkGray)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: Option<&str>, stores: &[&str], exes: &[&str]) -> GamedbRow {
        GamedbRow {
            key: "k".into(),
            title: "Control".into(),
            file: "control.toml".into(),
            state: RowState::Ready,
            id: id.map(str::to_string),
            stores: stores.iter().map(|s| s.to_string()).collect(),
            exes: exes.iter().map(|s| s.to_string()).collect(),
            steam: None,
            note: None,
            entries: 1,
        }
    }

    #[test]
    fn the_list_line_names_whatever_identifies_the_game() {
        assert_eq!(
            summary(&row(
                Some("steam-870780"),
                &["egs/Calluna"],
                &["Control_DX12.exe"]
            )),
            "steam-870780 · egs/Calluna · Control_DX12.exe"
        );
        // A page with no derivable id still shows what it has.
        assert_eq!(summary(&row(None, &[], &["Game.exe"])), "Game.exe");
        // And one with nothing says so rather than showing a blank line.
        assert_eq!(summary(&row(None, &[], &[])), "nothing identifies it yet");
    }

    #[test]
    fn a_published_page_and_an_unfinished_one_never_share_a_glyph() {
        let published = GamedbRow {
            state: RowState::InGamedb("steam-870780".into()),
            ..row(Some("steam-870780"), &[], &[])
        };
        let unfinished = GamedbRow {
            state: RowState::Incomplete("nothing identifies it".into()),
            ..row(None, &[], &[])
        };
        let glyphs = [
            row_state(&published).0,
            row_state(&row(Some("steam-1"), &[], &[])).0,
            row_state(&unfinished).0,
        ];
        assert_eq!(glyphs, ["✓", "●", "○"]);
    }
}
