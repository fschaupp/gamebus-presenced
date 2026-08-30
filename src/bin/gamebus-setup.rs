//! gamebus-setup - install and status tool for gamebus-presenced.
//!
//! Run with no arguments on a terminal for the TUI. The non-interactive
//! subcommands exist so the tool can be scripted, tested, and re-executed under
//! `pkexec` for a system-wide install - a privileged process must never be the
//! one holding the terminal in raw mode.

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[path = "../client.rs"]
mod client;
#[path = "../endpoints.rs"]
mod endpoints;
// The daemon's naming database, compiled into this tool for the umu-id
// drafting (title → Steam appid). Only that direction is live here - the
// rest of the shared module is the daemon's, hence the module-wide allow.
#[allow(dead_code)]
#[path = "../naming.rs"]
mod naming;
#[path = "../setup/mod.rs"]
mod setup;
#[path = "../umu_report.rs"]
mod umu_report;

use setup::actions::{self, Action, Plan, Source, Step};
use setup::paths::{Dirs, Target};
use setup::status::{self, Status, BUS_TIMEOUT};
use setup::ui;

fn usage() {
    eprintln!("Usage: gamebus-setup [command]");
    eprintln!();
    eprintln!("With no command on a terminal, starts the interactive setup TUI.");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  status                    Print system status as plain text");
    eprintln!("  plan <action> [options]   Print what an action would do, change nothing");
    eprintln!("  apply <action> [options]  Perform an action non-interactively");
    eprintln!("  umu-misses [options]      Games umu had no database entry for, and what");
    eprintln!("                            this machine resolved them to");
    eprintln!("  gamedb [options]          Those same games as gamebus-gamedb pages: one");
    eprintln!("                            page per game, the store codenames and");
    eprintln!("                            executables that identify it");
    eprintln!("  help                      Show this help");
    eprintln!();
    eprintln!("Actions:");
    eprintln!("  install | uninstall | enable | disable | start | stop | restart");
    eprintln!("  fetch-detectable");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --target user|system      Install target (default: user)");
    eprintln!("  --privileged-only         Run only the steps that need root");
    eprintln!("  --confirm                 Required by 'apply' - it writes to disk");
    eprintln!();
    eprintln!("umu-misses options:");
    eprintln!("  --verify                  Check every miss against the umu database");
    eprintln!("                            (local copy first, then the public API),");
    eprintln!("                            draft collision-checked umu ids, and see");
    eprintln!("                            whether the game needs a protonfix at all");
    eprintln!("  --fetch                   Refresh the cached database and protonfix");
    eprintln!("                            list (one request each)");
    eprintln!("  --db <file>               Database to verify against (CSV checkout or");
    eprintln!("                            JSON dump; also via GAMEBUS_UMU_DB)");
    eprintln!("  --export                  Submission-shaped CSV on stdout - only games");
    eprintln!("                            that need a protonfix; the database takes no");
    eprintln!("                            others (GAMEBUS_UMU_PROTONFIXES points the");
    eprintln!("                            check at a local umu-protonfixes checkout)");
    eprintln!("  --export-md [file]        Ready-to-paste merge-request text, written");
    eprintln!("                            to <file> - without one, printed to stdout");
    eprintln!("  --check-prs               Also scan open upstream merge requests for");
    eprintln!("                            already-submitted entries (best-effort)");
    eprintln!();
    eprintln!("gamedb options:");
    eprintln!("  --fetch                   Refresh the cached index of what gamebus-gamedb");
    eprintln!("                            already carries (one request)");
    eprintln!("  --export                  Write a page per ready game to <dir>/games/,");
    eprintln!("                            never overwriting one that is already there");
    eprintln!("  --out <dir>               Where --export writes (a checkout of");
    eprintln!("                            gamebus-gamedb is the useful thing to name)");
    eprintln!("  --documents               Write to <documents>/gamebus-gamedb instead");
    eprintln!("  --stale-ok                Export against an index older than 7 days");
    eprintln!("                            (GAMEBUS_GAMEDB_INDEX points the check at a");
    eprintln!("                            local identities.json)");
    eprintln!();
    eprintln!("Every remote endpoint the tools talk to is configured in endpoints.toml,");
    eprintln!("and shared-helpers.txt lists helper executables that never name a game");
    eprintln!("(~/.config/gamebus-presenced/ overrides the installed copies in the data");
    eprintln!("directory - endpoint keys replace, helper entries add on top; see each");
    eprintln!("file for the full order and the defaults).");
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags = Flags::parse(&args);

    match args.first().map(|s| s.as_str()) {
        Some("status") => cmd_status().await,
        Some("plan") => cmd_plan(&args, &flags),
        Some("apply") => cmd_apply(&args, &flags),
        Some("umu-misses") => setup::umu_misses::run(&args),
        Some("gamedb") => setup::gamedb::run(&args),
        Some("help") | Some("--help") | Some("-h") => {
            usage();
            ExitCode::SUCCESS
        }
        Some(cmd) => {
            eprintln!("Unknown command: {cmd}");
            eprintln!("Run 'gamebus-setup help' for usage.");
            ExitCode::FAILURE
        }
        None if std::io::stdout().is_terminal() => cmd_tui().await,
        // No terminal means no TUI: a piped or scripted invocation gets the
        // plain-text status rather than an attempt to enter raw mode.
        None => cmd_status().await,
    }
}

struct Flags {
    target: Target,
    confirm: bool,
    privileged_only: bool,
}

impl Flags {
    fn parse(args: &[String]) -> Self {
        let target = args
            .iter()
            .position(|a| a == "--target")
            .and_then(|i| args.get(i + 1))
            .and_then(|t| Target::parse(t))
            .unwrap_or(Target::User);
        Self {
            target,
            confirm: args.iter().any(|a| a == "--confirm"),
            privileged_only: args.iter().any(|a| a == "--privileged-only"),
        }
    }
}

fn dirs_or_exit() -> Dirs {
    match Dirs::from_env() {
        Some(d) => d,
        None => {
            eprintln!("HOME is not set - cannot work out where anything belongs.");
            std::process::exit(1);
        }
    }
}

async fn cmd_status() -> ExitCode {
    let dirs = dirs_or_exit();
    let status = status::probe(&dirs).await;
    print_status(&status);
    // Always zero: "something is wrong with your install" is the normal thing
    // for this command to report, not a failure of the command.
    ExitCode::SUCCESS
}

fn print_status(s: &Status) {
    let (health, summary) = status::overall(s);
    println!("gamebus-presenced - {} {summary}", health.marker());
    println!();
    for row in status::rows(s) {
        println!("  {} {:<11} {}", row.health.marker(), row.label, row.value);
        if let Some(hint) = row.hint {
            println!("                  {hint}");
        }
    }
    println!();
    println!("Install target: {}", s.primary().target.as_str());
}

/// Messages into the event loop. Everything slow happens off the render path
/// and arrives here.
enum Msg {
    Input(ratatui::crossterm::event::Event),
    Probed(Box<Status>),
    Activities(Vec<client::ActivityView>),
    Misses(Vec<(String, umu_report::Miss)>),
    /// The gamedb pane's rows, the index state and the export directory,
    /// loaded off the render path like the miss list.
    Gamedb(Box<setup::gamedb::GamedbView>),
    Done(Action, Vec<actions::StepOutcome>),
    /// A umu flow (verify / assign / pick / store cycle) finished: its log
    /// lines and whether it completed. Clears `busy` and refreshes the pane.
    UmuOutcome(Vec<String>, bool),
    /// A candidate search answered - the local pick (`p`) or an online
    /// lookup (`o`), kind-tagged either way. Clears `busy` and opens pick
    /// mode (or logs that nothing matched). `stale` warns when candidates
    /// came from an aging fetch cache (or an absent one).
    UmuCandidates {
        key: String,
        candidates: Vec<setup::umu_misses::PickCandidate>,
        stale: Option<String>,
    },
    Tick,
}

async fn cmd_tui() -> ExitCode {
    let dirs = dirs_or_exit();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Msg>(64);

    // Set while another process owns the terminal, so the reader stops
    // consuming stdin. Without it, a pkexec password prompt on a bare tty
    // races this thread for the user's keystrokes.
    let reader_paused = Arc::new(AtomicBool::new(false));

    // A blocking reader on its own thread rather than crossterm's EventStream:
    // that needs the `event-stream` feature, which would mean depending on
    // crossterm directly instead of through ratatui's re-export.
    {
        let tx = tx.clone();
        let paused = reader_paused.clone();
        std::thread::spawn(move || {
            use ratatui::crossterm::event;
            loop {
                if paused.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
                match event::poll(std::time::Duration::from_millis(200)) {
                    Ok(true) => match event::read() {
                        Ok(ev) => {
                            if tx.blocking_send(Msg::Input(ev)).is_err() {
                                return;
                            }
                        }
                        Err(_) => return,
                    },
                    Ok(false) => {}
                    Err(_) => return,
                }
            }
        });
    }

    let mut terminal = ratatui::init();
    let mut app = ui::App::default();
    app.log("Probing…");
    app.probing = true;
    spawn_probe(&tx, &dirs);
    spawn_misses(&tx);
    spawn_gamedb(&tx);

    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(250));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut monitor_tick = tokio::time::interval(std::time::Duration::from_secs(1));
    monitor_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Opened on first use and kept, rather than rebuilt every tick.
    let mut monitor_conn: Option<zbus::Connection> = None;

    while !app.should_quit {
        if terminal.draw(|f| ui::render(f, &mut app)).is_err() {
            break;
        }

        let msg = tokio::select! {
            msg = rx.recv() => match msg {
                Some(m) => m,
                None => break,
            },
            _ = ticker.tick() => Msg::Tick,
            _ = monitor_tick.tick(),
                if matches!(
                    app.view,
                    ui::View::Monitor | ui::View::Misses | ui::View::Gamedb
                ) =>
            {
                if app.view == ui::View::Gamedb {
                    // The same stash the misses pane watches, folded into
                    // pages - and the same convention: no I/O on the render
                    // path, so it arrives as a message.
                    spawn_gamedb(&tx);
                } else if app.view == ui::View::Monitor {
                    if monitor_conn.is_none() {
                        monitor_conn =
                            tokio::time::timeout(BUS_TIMEOUT, zbus::Connection::session())
                                .await
                                .ok()
                                .and_then(|r| r.ok());
                    }
                    spawn_activities(&tx, monitor_conn.clone());
                } else {
                    // The stash is a small local file, but the convention
                    // holds: nothing on the render path does I/O, so it too
                    // arrives as a message. Refreshed while watched - the
                    // daemon appends on its own schedule.
                    spawn_misses(&tx);
                }
                Msg::Tick
            }
        };

        handle(&mut app, msg, &tx, &dirs, &mut terminal, &reader_paused).await;
    }

    ratatui::restore();
    ExitCode::SUCCESS
}

fn spawn_probe(tx: &tokio::sync::mpsc::Sender<Msg>, dirs: &Dirs) {
    let tx = tx.clone();
    let dirs = dirs.clone();
    tokio::spawn(async move {
        let status = status::probe(&dirs).await;
        let _ = tx.send(Msg::Probed(Box::new(status))).await;
    });
}

/// Refresh the monitor view.
///
/// Bounded like every other bus call - a wedged daemon must not accumulate one
/// hung task per second - and reusing one connection rather than opening a
/// fresh one each tick.
fn spawn_activities(tx: &tokio::sync::mpsc::Sender<Msg>, conn: Option<zbus::Connection>) {
    let tx = tx.clone();
    tokio::spawn(async move {
        let conn = match conn {
            Some(c) => c,
            None => return,
        };
        if let Ok(Ok((_, _, activities))) =
            tokio::time::timeout(BUS_TIMEOUT, client::snapshot(&conn)).await
        {
            let _ = tx.send(Msg::Activities(activities)).await;
        }
    });
}

/// Refresh the umu-miss pane from the stash file. Rows carry their stash
/// key so the UI can keep the selection on the same game across reorders,
/// and the key is the final sort tie-break - same-day unresolved entries
/// would otherwise land in HashMap iteration order, which reshuffles on
/// every load.
fn spawn_misses(tx: &tokio::sync::mpsc::Sender<Msg>) {
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let report = umu_report::UmuReport::load();
        let mut misses: Vec<(String, umu_report::Miss)> = report
            .entries()
            .iter()
            .map(|(k, m)| (k.clone(), m.clone()))
            .collect();
        misses.sort_by(|a, b| {
            // Dismissed entries park at the bottom, out of the way.
            (a.1.dismissed.is_some())
                .cmp(&b.1.dismissed.is_some())
                .then_with(|| b.1.last_seen.cmp(&a.1.last_seen))
                .then_with(|| a.1.title.cmp(&b.1.title))
                .then_with(|| a.0.cmp(&b.0))
        });
        let _ = tx.blocking_send(Msg::Misses(misses));
    });
}

/// Refresh the gamedb pane: the stash folded into pages, the cached index,
/// and the configured export directory. Local files only - the pane's `r`
/// is the one key that reaches the network.
fn spawn_gamedb(tx: &tokio::sync::mpsc::Sender<Msg>) {
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let view = setup::gamedb::tui_view();
        let _ = tx.blocking_send(Msg::Gamedb(Box::new(view)));
    });
}

/// Run one of the misses pane's blocking flows (network and disk) off the
/// render path, delivering its log lines as a message.
fn spawn_umu_flow(
    tx: &tokio::sync::mpsc::Sender<Msg>,
    flow: impl FnOnce() -> (Vec<String>, bool) + Send + 'static,
) {
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let (lines, ok) = flow();
        let _ = tx.blocking_send(Msg::UmuOutcome(lines, ok));
    });
}

/// Start the gamedb pane's export. Reached twice: straight from `e` when
/// the index is fresh, and from the confirm dialog when it is not - which
/// is the only place `force` comes from.
fn start_gamedb_export(app: &mut ui::App, tx: &tokio::sync::mpsc::Sender<Msg>, force: bool) {
    if app.busy.is_some() {
        return;
    }
    app.busy = Some("writing gamedb pages".into());
    spawn_umu_flow(tx, move || setup::gamedb::tui_export(force));
}

/// Run a plan that needs no privileges, off the render path. Subprocesses and
/// the 12 MB download would otherwise freeze the interface for seconds at a
/// time.
fn spawn_action(tx: &tokio::sync::mpsc::Sender<Msg>, action: Action, dirs: &Dirs) {
    let tx = tx.clone();
    let dirs = dirs.clone();
    tokio::task::spawn_blocking(move || {
        let source = Source::discover(&dirs);
        let plan = actions::plan(action, &dirs, &source);
        let steps: Vec<Step> = plan.all_steps().cloned().collect();
        let outcomes = actions::execute(&steps);
        let _ = tx.blocking_send(Msg::Done(action, outcomes));
    });
}

/// Run a plan whose file writes need root, with the terminal handed over.
///
/// Deliberately synchronous: the TUI has left the alternate screen, so there is
/// nothing to render, and pkexec's authentication takes as long as the user
/// takes. Sleeping a fixed interval and hoping - which is what this used to do
/// - draws the interface on top of the password prompt.
///
/// Ordering matters and differs from the unprivileged path only in where the
/// escalation sits: `before` (stop and disable) must precede the file removals,
/// and `after` (daemon-reload) must follow the writes - and must not run at all
/// if the user dismissed the dialog, or a cancelled uninstall would still have
/// stopped their daemon.
async fn run_escalated(
    app: &mut ui::App,
    action: Action,
    target: Target,
    dirs: &Dirs,
    terminal: &mut ratatui::DefaultTerminal,
    reader_paused: &Arc<AtomicBool>,
) {
    use ratatui::style::{Color, Style};

    let dirs_owned = dirs.clone();
    let plan = {
        let source = Source::discover(dirs);
        actions::plan(action, dirs, &source)
    };

    reader_paused.store(true, Ordering::Relaxed);
    ratatui::restore();

    let before = plan.before.clone();
    let after = plan.after.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _ = &dirs_owned;
        let before_outcomes = actions::execute(&before);
        let aborted = before_outcomes
            .iter()
            .zip(before.iter())
            .any(|(o, s)| !o.ok && s.is_fatal_on_failure());
        if aborted {
            return (before_outcomes, None, Vec::new());
        }
        let escalation = actions::escalate(action, target);
        let proceed = matches!(&escalation, actions::Escalation::Ran { ok: true, .. });
        let after_outcomes = if proceed {
            actions::execute(&after)
        } else {
            Vec::new()
        };
        (before_outcomes, Some(escalation), after_outcomes)
    })
    .await;

    *terminal = ratatui::init();
    reader_paused.store(false, Ordering::Relaxed);

    let (before_outcomes, escalation, after_outcomes) = match result {
        Ok(r) => r,
        Err(e) => {
            app.log_styled(
                format!("Failed to run: {e}"),
                Style::default().fg(Color::Red),
            );
            return;
        }
    };

    for outcome in before_outcomes.iter().chain(after_outcomes.iter()) {
        let style = if outcome.ok {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Red)
        };
        app.log_styled(format!("  {}", outcome.description), style);
        for line in outcome.detail.lines() {
            app.log_styled(format!("      {line}"), style);
        }
    }

    match escalation {
        None => app.log_styled(
            "Stopped before asking for privileges - a step failed.",
            Style::default().fg(Color::Red),
        ),
        Some(actions::Escalation::Ran { ok, output }) => {
            for line in output.lines() {
                app.log_styled(
                    format!("  {line}"),
                    Style::default().fg(if ok { Color::Reset } else { Color::Red }),
                );
            }
        }
        Some(actions::Escalation::Cancelled) => app.log_styled(
            "Cancelled - nothing privileged was written.",
            Style::default().fg(Color::Yellow),
        ),
        Some(actions::Escalation::Unavailable { reason }) => {
            app.log_styled(
                format!("Could not ask for privileges: {reason}"),
                Style::default().fg(Color::Red),
            );
            app.log("Run this yourself:");
            app.log(format!("  {}", actions::sudo_hint(action, target)));
        }
    }
}

async fn handle(
    app: &mut ui::App,
    msg: Msg,
    tx: &tokio::sync::mpsc::Sender<Msg>,
    dirs: &Dirs,
    terminal: &mut ratatui::DefaultTerminal,
    reader_paused: &Arc<AtomicBool>,
) {
    use ratatui::crossterm::event::Event;
    use ratatui::style::{Color, Style};

    match msg {
        Msg::Tick => app.spinner = app.spinner.wrapping_add(1),
        Msg::Probed(status) => {
            app.set_status(*status);
            // Deliberately does NOT clear `busy`: that is the gate stopping a
            // second mutating action, and probes land on their own schedule -
            // including the one every finished action kicks off.
            app.probing = false;
        }
        Msg::Activities(activities) => app.activities = activities,
        Msg::Misses(misses) => app.set_misses(misses),
        Msg::Gamedb(view) => app.set_gamedb(*view),
        Msg::UmuOutcome(lines, ok) => {
            for line in lines {
                app.log_styled(
                    format!("  {line}"),
                    if ok {
                        Style::default().fg(Color::Reset)
                    } else {
                        Style::default().fg(Color::Red)
                    },
                );
            }
            app.busy = None;
            // Show what the flow changed without waiting for the next tick.
            spawn_misses(tx);
            spawn_gamedb(tx);
        }
        Msg::UmuCandidates {
            key,
            candidates,
            stale,
        } => {
            app.busy = None;
            // The staleness warning matters MOST when nothing matched: an
            // old cache missing a fresh entry reads exactly like "not in
            // the database".
            if let Some(warning) = &stale {
                app.log_styled(format!("  {warning}"), Style::default().fg(Color::Yellow));
            }
            if candidates.is_empty() {
                app.log(
                    "  No database or library title matches - v verifies against the live API too.",
                );
            } else if app.misses.iter().any(|(k, _)| k == &key) {
                app.log(format!(
                    "  {} candidate(s) - ↑↓ choose, Enter picks, Esc cancels.",
                    candidates.len()
                ));
                app.pick = Some(ui::Pick {
                    key,
                    candidates,
                    selected: 0,
                    stale,
                });
            }
            // A refresh dropped the miss while the search ran: nothing left
            // to pick for.
        }
        Msg::Done(action, outcomes) => {
            for outcome in &outcomes {
                let style = if outcome.ok {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Red)
                };
                app.log_styled(format!("  {}", outcome.description), style);
                for line in outcome.detail.lines() {
                    app.log_styled(format!("      {line}"), style);
                }
            }
            app.log_styled(
                format!("{} finished.", action.label()),
                Style::default().add_modifier(ratatui::style::Modifier::BOLD),
            );
            app.busy = None;
            // Never claim success: re-probe and show what is actually true now.
            spawn_probe(tx, dirs);
        }
        Msg::Input(Event::Key(key))
            if key.kind == ratatui::crossterm::event::KeyEventKind::Press =>
        {
            match ui::handle_key(app, key) {
                ui::Intent::Quit => app.should_quit = true,
                ui::Intent::Refresh => {
                    app.probing = true;
                    spawn_probe(tx, dirs);
                    spawn_misses(tx);
                    spawn_gamedb(tx);
                }
                // The misses pane's flows share the busy gate with the
                // install actions: one mutating thing at a time.
                ui::Intent::UmuVerify => {
                    if app.busy.is_some() {
                        return;
                    }
                    app.busy = Some("fetching + verifying umu misses".into());
                    app.log_styled(
                        "Fetching the umu database and verifying the stash…",
                        Style::default().add_modifier(ratatui::style::Modifier::BOLD),
                    );
                    spawn_umu_flow(tx, setup::umu_misses::tui_fetch_and_verify);
                }
                ui::Intent::UmuAssign { key, id } => {
                    if app.busy.is_some() {
                        return;
                    }
                    app.busy = Some("checking the assigned id".into());
                    spawn_umu_flow(tx, move || setup::umu_misses::tui_assign_id(&key, &id));
                }
                ui::Intent::UmuPick { key } => {
                    if app.busy.is_some() {
                        return;
                    }
                    // The query is the effective title - the user's
                    // correction when present; without any there is nothing
                    // to search for.
                    let title = app
                        .misses
                        .iter()
                        .find(|(k, _)| *k == key)
                        .and_then(|(_, m)| m.effective_title().map(str::to_string));
                    let Some(title) = title else {
                        app.log_styled(
                            "  No resolved title to search the database for.",
                            Style::default().fg(Color::Red),
                        );
                        return;
                    };
                    app.busy = Some("searching the local database and libraries".into());
                    // Local files only - `v` and `o` are the network keys.
                    let tx = tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let msg = match setup::umu_misses::tui_pick_candidates(&title) {
                            Ok((candidates, stale)) => Msg::UmuCandidates {
                                key,
                                candidates,
                                stale,
                            },
                            Err(e) => Msg::UmuOutcome(vec![e], false),
                        };
                        let _ = tx.blocking_send(msg);
                    });
                }
                ui::Intent::UmuOnline { key } => {
                    if app.busy.is_some() {
                        return;
                    }
                    // The effective title and codename: the user's
                    // corrections when present. The title may be absent  -
                    // a gog entry with a numeric codename looks up by id
                    // and needs none; the flow refuses the rest honestly.
                    let entry = app.misses.iter().find(|(k, _)| *k == key).map(|(_, m)| {
                        (
                            m.effective_title().map(str::to_string),
                            m.effective_store().to_string(),
                            m.effective_codename().map(str::to_string),
                        )
                    });
                    let Some((title, store, codename)) = entry else {
                        return;
                    };
                    app.busy = Some(format!("asking the {store} store"));
                    // One keypress, one request; the store dispatch (and the
                    // store-none refusal) lives in the flow itself.
                    let tx = tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let msg = match setup::umu_misses::tui_online_candidates(
                            &store,
                            title.as_deref(),
                            codename.as_deref(),
                        ) {
                            Ok(candidates) if candidates.is_empty() => Msg::UmuOutcome(
                                vec![format!(
                                    "No {store} hits{}.",
                                    title.map(|t| format!(" for '{t}'")).unwrap_or_default()
                                )],
                                true,
                            ),
                            Ok(candidates) => Msg::UmuCandidates {
                                key,
                                candidates,
                                stale: None,
                            },
                            Err(e) => Msg::UmuOutcome(vec![e], false),
                        };
                        let _ = tx.blocking_send(msg);
                    });
                }
                ui::Intent::UmuEgsBuilds { key, namespace } => {
                    if app.busy.is_some() {
                        return;
                    }
                    app.busy = Some(format!("fetching the {namespace} builds"));
                    let tx = tx.clone();
                    tokio::task::spawn_blocking(move || {
                        let msg = match setup::umu_misses::tui_egs_builds(&namespace) {
                            Ok(candidates) if candidates.is_empty() => Msg::UmuOutcome(
                                vec![format!("No builds listed for the {namespace} sandbox.")],
                                true,
                            ),
                            Ok(candidates) => Msg::UmuCandidates {
                                key,
                                candidates,
                                stale: None,
                            },
                            Err(e) => Msg::UmuOutcome(vec![e], false),
                        };
                        let _ = tx.blocking_send(msg);
                    });
                }
                ui::Intent::UmuSetIdentity {
                    key,
                    store,
                    codename,
                    source,
                } => {
                    if app.busy.is_some() {
                        return;
                    }
                    app.busy = Some("recording the identity".into());
                    spawn_umu_flow(tx, move || {
                        setup::umu_misses::tui_set_identity(
                            &key,
                            store.as_deref(),
                            &codename,
                            &source,
                        )
                    });
                }
                ui::Intent::UmuPickEntry {
                    key,
                    store,
                    codename,
                    umu_id,
                } => {
                    if app.busy.is_some() {
                        return;
                    }
                    app.busy = Some("recording the picked entry".into());
                    spawn_umu_flow(tx, move || {
                        setup::umu_misses::tui_pick_entry(&key, &store, &codename, &umu_id)
                    });
                }
                ui::Intent::UmuStore { key } => {
                    if app.busy.is_some() {
                        return;
                    }
                    app.busy = Some("updating the store".into());
                    spawn_umu_flow(tx, move || setup::umu_misses::tui_cycle_store(&key));
                }
                ui::Intent::UmuSetTitle { key, title, source } => {
                    if app.busy.is_some() {
                        return;
                    }
                    app.busy = Some("recording the title".into());
                    spawn_umu_flow(tx, move || {
                        setup::umu_misses::tui_set_title(&key, &title, &source)
                    });
                }
                ui::Intent::UmuDismiss { key } => {
                    if app.busy.is_some() {
                        return;
                    }
                    app.busy = Some("updating the entry".into());
                    spawn_umu_flow(tx, move || setup::umu_misses::tui_toggle_dismiss(&key));
                }
                ui::Intent::UmuPromote { key } => {
                    if app.busy.is_some() {
                        return;
                    }
                    app.busy = Some("updating the entry".into());
                    spawn_umu_flow(tx, move || setup::umu_misses::tui_toggle_promote(&key));
                }
                ui::Intent::GamedbFetch => {
                    if app.busy.is_some() {
                        return;
                    }
                    app.busy = Some("fetching the gamebus-gamedb index".into());
                    spawn_umu_flow(tx, setup::gamedb::tui_fetch);
                }
                ui::Intent::GamedbExport { force } => start_gamedb_export(app, tx, force),
                ui::Intent::GamedbSetDir { dir } => {
                    if app.busy.is_some() {
                        return;
                    }
                    app.busy = Some("saving the export directory".into());
                    spawn_umu_flow(tx, move || setup::gamedb::tui_set_dir(&dir));
                }
                ui::Intent::Run(action) => {
                    if app.busy.is_some() {
                        return;
                    }
                    let source = Source::discover(dirs);
                    let plan = actions::plan(action, dirs, &source);
                    // SAFETY: geteuid cannot fail and touches no memory we own.
                    let euid = app
                        .status
                        .as_ref()
                        .map(|s| s.euid)
                        .unwrap_or_else(|| unsafe { libc::geteuid() });
                    let needs_root = actions::needs_root(&plan, euid);
                    app.confirm = Some(ui::Confirm {
                        explanation: actions::explain(&plan),
                        scope: actions::scope_note(&plan, &dirs.home),
                        details: false,
                        action: Some(action),
                        plan,
                        needs_root,
                        on_yes: None,
                        label: None,
                    });
                }
                ui::Intent::ConfirmYes => {
                    let Some(confirm) = app.confirm.take() else {
                        return;
                    };
                    // A confirmation that runs an intent rather than an
                    // install plan: the gamedb export against an index the
                    // pane could not vouch for.
                    if let Some(intent) = confirm.on_yes {
                        if let ui::Intent::GamedbExport { force } = intent {
                            start_gamedb_export(app, tx, force);
                        }
                        return;
                    }
                    {
                        let Some(action) = confirm.action else {
                            return;
                        };
                        app.log_styled(
                            action.label(),
                            Style::default().add_modifier(ratatui::style::Modifier::BOLD),
                        );
                        match (confirm.needs_root, action.target()) {
                            // Escalation is offered for the system target only.
                            // pkexec resets the environment, and only the system
                            // layout is environment-free - escalating a *user*
                            // action would resolve $HOME to /root in the child
                            // and install somewhere the user never confirmed.
                            (true, Some(Target::System)) => {
                                app.busy = Some(action.label());
                                run_escalated(
                                    app,
                                    action,
                                    Target::System,
                                    dirs,
                                    terminal,
                                    reader_paused,
                                )
                                .await;
                                app.busy = None;
                                spawn_probe(tx, dirs);
                            }
                            (true, _) => {
                                app.log_styled(
                                    "Cannot do this: a path this action writes to is not \
                                     yours to write.",
                                    Style::default().fg(Color::Red),
                                );
                                app.log(
                                    "Root is only offered for the system target. Check the \
                                     ownership of the directories listed above.",
                                );
                            }
                            (false, _) => {
                                app.busy = Some(action.label());
                                spawn_action(tx, action, dirs);
                            }
                        }
                    }
                }
                ui::Intent::ConfirmNo => {
                    app.confirm = None;
                    app.log("Cancelled.");
                }
                ui::Intent::None => {}
            }
        }
        Msg::Input(_) => {}
    }
}

fn action_from(args: &[String], flags: &Flags) -> Option<Action> {
    Action::parse(args.get(1)?, flags.target)
}

fn cmd_plan(args: &[String], flags: &Flags) -> ExitCode {
    let Some(action) = action_from(args, flags) else {
        eprintln!("Usage: gamebus-setup plan <action> [--target user|system]");
        return ExitCode::FAILURE;
    };
    let dirs = dirs_or_exit();
    let source = Source::discover(&dirs);
    let plan = actions::plan(action, &dirs, &source);

    println!("{} will:", action.label());
    for line in actions::explain(&plan) {
        println!("  • {line}");
    }
    println!();
    println!("{}", actions::scope_note(&plan, &dirs.home));
    println!();
    println!("Exactly:");
    print_plan(&plan);

    // SAFETY: geteuid cannot fail and touches no memory we own.
    let euid = unsafe { libc::geteuid() };
    if actions::needs_root(&plan, euid) {
        println!();
        // Escalation is only ever offered for the system target: pkexec resets
        // the environment, and only the system layout is free of it. Printing
        // a `sudo … --target user` hint here would hand the user a command
        // that installs somewhere other than the plan they just read.
        if action.target() == Some(Target::System) {
            println!("This requires root. gamebus-setup asks via pkexec, or run:");
            println!("  {}", actions::sudo_hint(action, Target::System));
        } else {
            println!("A path this would write to is not yours to write.");
            println!("Root is only offered for the system target - check the ownership");
            println!("of the directories listed above.");
        }
    }
    println!();
    println!("Nothing was changed.");
    ExitCode::SUCCESS
}

fn print_plan(plan: &Plan) {
    // "files" is the block that may need root; "session" always runs as the
    // invoking user, because `systemctl --user` under root is the wrong manager.
    let phases = [
        ("session", &plan.before),
        ("files", &plan.privileged),
        ("session", &plan.after),
    ];
    for (who, steps) in phases {
        for step in steps.iter() {
            println!("  [{who:>7}] {step}");
        }
    }
    if plan.is_empty() {
        println!("  (nothing to do)");
    }
}

fn cmd_apply(args: &[String], flags: &Flags) -> ExitCode {
    let Some(action) = action_from(args, flags) else {
        eprintln!("Usage: gamebus-setup apply <action> [--target user|system] --confirm");
        return ExitCode::FAILURE;
    };
    if !flags.confirm {
        eprintln!("'apply' writes to disk. Re-run with --confirm, or use 'plan' to see the steps.");
        return ExitCode::FAILURE;
    }

    let dirs = dirs_or_exit();
    let source = Source::discover(&dirs);
    let plan = actions::plan(action, &dirs, &source);
    // SAFETY: geteuid cannot fail and touches no memory we own.
    let euid = unsafe { libc::geteuid() };

    // A privileged run must be a *system* run. This is the guard that makes
    // the invariant hold for the escalated child as well as the TUI: without
    // it, `pkexec … apply install --target user --privileged-only` resolves
    // $HOME to /root in the child and installs where nobody agreed to.
    if flags.privileged_only && action.target() != Some(Target::System) {
        eprintln!("--privileged-only is only valid with --target system.");
        eprintln!(
            "{} does not write outside your home directory.",
            action.label()
        );
        return ExitCode::FAILURE;
    }

    let steps: Vec<Step> = if flags.privileged_only {
        // The escalated half. Session commands are deliberately excluded:
        // `systemctl --user` as root talks to root's manager, not the user's.
        plan.privileged.clone()
    } else if actions::needs_root(&plan, euid) {
        eprintln!("{} needs root for some steps.", action.label());
        if action.target() == Some(Target::System) {
            eprintln!("Run:  {}", actions::sudo_hint(action, Target::System));
            eprintln!("or use the TUI, which asks via pkexec.");
        } else {
            eprintln!("Root is only offered for the system target - check the ownership");
            eprintln!("of the directories this would write to.");
        }
        return ExitCode::FAILURE;
    } else {
        plan.before
            .iter()
            .chain(plan.privileged.iter())
            .chain(plan.after.iter())
            .cloned()
            .collect()
    };

    let outcomes = actions::execute(&steps);
    let mut failed = false;
    for (outcome, step) in outcomes.iter().zip(steps.iter()) {
        let fatal = step.is_fatal_on_failure();
        let mark = match (outcome.ok, fatal) {
            (true, _) => "ok  ",
            // A best-effort step that failed is not an error: `systemctl
            // disable` on a unit systemd never loaded is the normal case.
            (false, false) => "skip",
            (false, true) => "ERR ",
        };
        println!("{mark}{}", outcome.description);
        if !outcome.detail.trim().is_empty() {
            for line in outcome.detail.lines() {
                println!("      {line}");
            }
        }
        failed |= !outcome.ok && fatal;
    }
    if outcomes.len() < steps.len() {
        println!("Stopped after {} of {} steps.", outcomes.len(), steps.len());
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
