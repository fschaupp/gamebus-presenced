//! The umu-misses tab: the entry list, the detail pane, and pick mode.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use super::super::umu_misses::{basis_label, confidence_label, PickCandidate};
use super::{field, App};
use crate::umu_report::{Miss, VerificationState};

/// The umu-miss review pane: what the daemon collected, one entry per game,
/// with the verification verdict and drafted id once verification ran.
/// Its verbs act only on explicit keypresses the footer labels — `v` is the
/// one that reaches the network, and says so. Exporting stays a CLI
/// invocation: a submission wants a shell, not a raw-mode terminal.
pub(super) fn render_misses(f: &mut Frame, area: Rect, app: &mut App) {
    if app.misses.is_empty() {
        let text = "No umu-database misses recorded.\n\n\
            The daemon writes one entry per game that umu launched without a\n\
            database entry (GAMEID=umu-0), together with the title it resolved.\n\
            Review them here — v fetches the database and verifies, s corrects\n\
            a store guess, t corrects a title, a assigns an id by hand\n\
            (collision-checked), p picks a match from the local database or\n\
            your Heroic library, and o looks the title up at its store\n\
            (network).\n\
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
                        m.effective_title().unwrap_or("(unresolved)").to_string(),
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

    // Pick mode replaces the detail with the candidate list. It is bound to
    // the stash key it was opened for, not to the (frozen) selection. The
    // candidates arrive grouped by kind; section headers are rows of their
    // own and can never be selected — `selected` indexes candidates, and
    // [`pick_display_index`] maps it onto the row list at render time.
    if let Some(pick) = &app.pick {
        // A library identity from a different store than the one already on
        // the miss still works (picking it changes the store), but it is
        // probably not the row the user is after — greyed, not hidden.
        let miss_store = app
            .misses
            .iter()
            .find(|(k, _)| k == &pick.key)
            .map(|(_, m)| m.effective_store().to_string());
        let mut last_section: Option<String> = None;
        let mut items: Vec<ListItem> = Vec::new();
        for c in &pick.candidates {
            let section = c.section_label();
            if last_section.as_deref() != Some(section.as_str()) {
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("— {section} —"),
                    Style::default().fg(Color::Cyan),
                ))));
                last_section = Some(section);
            }
            let (name, detail) = candidate_row(c);
            let (name_style, detail_style) = if cross_store_library(c, miss_store.as_deref()) {
                let dim = Style::default().fg(Color::DarkGray);
                (dim, dim)
            } else {
                (
                    Style::default().add_modifier(Modifier::BOLD),
                    Style::default().fg(Color::DarkGray),
                )
            };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(name, name_style),
                Span::styled(format!("  {detail}"), detail_style),
            ])));
        }
        let mut state = ListState::default();
        state.select(Some(pick_display_index(&pick.candidates, pick.selected)));
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Pick a match ")
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        f.render_stateful_widget(list, panes[1], &mut state);
        return;
    }

    let Some((_, m)) = app.miss_list.selected().and_then(|i| app.misses.get(i)) else {
        return;
    };
    let title_line = match &m.title_override {
        Some(over) => format!(
            "{over} (set by you; resolver said {})",
            m.title.as_deref().unwrap_or("-")
        ),
        None => m.title.clone().unwrap_or_else(|| "(unresolved)".into()),
    };
    let mut detail = vec![field("Title", &title_line)];
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
    let codename_line = match &m.codename_override {
        Some(over) => format!(
            "{over} (set by you; launcher reported {})",
            m.codename.as_deref().unwrap_or("-")
        ),
        None => m.codename.clone().unwrap_or_else(|| "-".into()),
    };
    detail.extend([
        field("Store", &store_line),
        field("Codename", &codename_line),
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
    if let Some(buffer) = &app.title_input {
        detail.push(Line::from(vec![
            Span::styled(
                format!("{:<11} ", "Set title"),
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
            // The buffer starts empty; the label carries what it would
            // replace (typing that very title back clears the override).
            Span::styled(
                format!(
                    "  now: {} · ⏎ save · esc cancel",
                    m.effective_title().unwrap_or("(unresolved)")
                ),
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

/// Where candidate `selected` lands in the rendered row list, counting the
/// section-header rows inserted before it. Headers are display-only: the
/// selection moves over candidates, so a header row can never be selected.
fn pick_display_index(candidates: &[PickCandidate], selected: usize) -> usize {
    let mut last_section: Option<String> = None;
    let mut headers = 0;
    for c in candidates.iter().take(selected + 1) {
        let section = c.section_label();
        if last_section.as_deref() != Some(section.as_str()) {
            last_section = Some(section);
            headers += 1;
        }
    }
    selected + headers
}

/// A Heroic library row whose store differs from the one the miss already
/// carries. Only library identities grey this way: database rows are
/// cross-store on purpose, and the online sections were store-dispatched
/// to begin with.
fn cross_store_library(c: &PickCandidate, miss_store: Option<&str>) -> bool {
    match (c, miss_store) {
        (PickCandidate::Library(g), Some(store)) => store != "none" && g.store != store,
        _ => false,
    }
}

/// One pick row: what to print bold, and the gray detail beside it.
fn candidate_row(c: &PickCandidate) -> (String, String) {
    match c {
        PickCandidate::Db(e) => (
            e.title.clone(),
            format!("{} · {} · {}", e.store, e.codename, e.umu_id),
        ),
        PickCandidate::Library(g) => (g.title.clone(), format!("{} · {}", g.store, g.codename)),
        PickCandidate::GogProduct(p) => (p.title.clone(), format!("{} · {}", p.id, p.product_type)),
        PickCandidate::GogById {
            id,
            title,
            game_type,
        } => (title.clone(), format!("{id} · {game_type}")),
        PickCandidate::EgsOffer(o) => (
            o.title.clone(),
            format!("{} · {}", o.namespace, o.offer_type),
        ),
        PickCandidate::EgsBuild(b) => (
            b.app_name.clone(),
            format!("{} · {}", b.label_name, b.platform),
        ),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_candidate(store: &str, codename: &str, umu_id: &str) -> PickCandidate {
        PickCandidate::Db(crate::umu_report::UmuEntry {
            title: "Control".into(),
            store: store.into(),
            codename: codename.into(),
            umu_id: umu_id.into(),
        })
    }

    #[test]
    fn section_headers_are_rows_of_their_own_and_never_selected() {
        // Two sections: db rows then a library row. Headers occupy display
        // rows, so candidate 0 renders at row 1 (after its header), and the
        // library candidate at row 4 (two headers before it).
        let candidates = vec![
            sample_candidate("egs", "Calluna", "umu-870780"),
            sample_candidate("gog", "2049187585", "umu-870780"),
            PickCandidate::Library(crate::setup::heroic_library::LibraryGame {
                title: "Control".into(),
                store: "gog".into(),
                codename: "2049187585".into(),
            }),
        ];
        assert_eq!(pick_display_index(&candidates, 0), 1);
        assert_eq!(pick_display_index(&candidates, 1), 2);
        assert_eq!(pick_display_index(&candidates, 2), 4);
    }

    #[test]
    fn library_rows_from_another_store_grey_out_but_stay_pickable() {
        let gog_lib = PickCandidate::Library(crate::setup::heroic_library::LibraryGame {
            title: "Control".into(),
            store: "gog".into(),
            codename: "2049187585".into(),
        });
        // The miss already says egs: a gog library row is probably not the
        // one — greyed. Same store, store none, or a db row: full color.
        assert!(cross_store_library(&gog_lib, Some("egs")));
        assert!(!cross_store_library(&gog_lib, Some("gog")));
        assert!(!cross_store_library(&gog_lib, Some("none")));
        assert!(!cross_store_library(
            &sample_candidate("gog", "2049187585", "umu-870780"),
            Some("egs")
        ));
    }
}
