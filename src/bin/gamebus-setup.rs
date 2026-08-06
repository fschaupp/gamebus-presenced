//! gamebus-setup — install and status tool for gamebus-presenced.
//!
//! Run with no arguments on a terminal for the TUI. The non-interactive
//! subcommands exist so the tool can be scripted, tested, and re-executed under
//! `pkexec` for a system-wide install — a privileged process must never be the
//! one holding the terminal in raw mode.

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[path = "../client.rs"]
mod client;
// The daemon's naming database, compiled into this tool for the S9b umu-id
// drafting (title → Steam appid). Only that direction is live here — the
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
    eprintln!("  help                      Show this help");
    eprintln!();
    eprintln!("Actions:");
    eprintln!("  install | uninstall | enable | disable | start | stop | restart");
    eprintln!("  fetch-detectable");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --target user|system      Install target (default: user)");
    eprintln!("  --privileged-only         Run only the steps that need root");
    eprintln!("  --confirm                 Required by 'apply' — it writes to disk");
    eprintln!();
    eprintln!("umu-misses options:");
    eprintln!("  --verify                  Check every miss against the umu database");
    eprintln!("                            (local copy first, then the public API) and");
    eprintln!("                            draft collision-checked umu ids");
    eprintln!("  --fetch                   Refresh the cached database (one request)");
    eprintln!("  --db <file>               Database to verify against (CSV checkout or");
    eprintln!("                            JSON dump; also via GAMEBUS_UMU_DB)");
    eprintln!("  --export                  Submission-shaped CSV on stdout");
    eprintln!("  --export-md [file]        Ready-to-paste merge-request text");
    eprintln!("  --check-prs               Also scan open upstream merge requests for");
    eprintln!("                            already-submitted entries (best-effort)");
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
            eprintln!("HOME is not set — cannot work out where anything belongs.");
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
    println!("gamebus-presenced — {} {summary}", health.marker());
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
    Done(Action, Vec<actions::StepOutcome>),
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
            _ = monitor_tick.tick(), if app.view == ui::View::Monitor => {
                if monitor_conn.is_none() {
                    monitor_conn = tokio::time::timeout(BUS_TIMEOUT, zbus::Connection::session())
                        .await
                        .ok()
                        .and_then(|r| r.ok());
                }
                spawn_activities(&tx, monitor_conn.clone());
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
/// Bounded like every other bus call — a wedged daemon must not accumulate one
/// hung task per second — and reusing one connection rather than opening a
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
/// takes. Sleeping a fixed interval and hoping — which is what this used to do
/// — draws the interface on top of the password prompt.
///
/// Ordering matters and differs from the unprivileged path only in where the
/// escalation sits: `before` (stop and disable) must precede the file removals,
/// and `after` (daemon-reload) must follow the writes — and must not run at all
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
            "Stopped before asking for privileges — a step failed.",
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
            "Cancelled — nothing privileged was written.",
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
            // second mutating action, and probes land on their own schedule —
            // including the one every finished action kicks off.
            app.probing = false;
        }
        Msg::Activities(activities) => app.activities = activities,
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
                        explanation: actions::explain(action, &plan),
                        scope: actions::scope_note(&plan, &dirs.home),
                        details: false,
                        action,
                        plan,
                        needs_root,
                    });
                }
                ui::Intent::ConfirmYes => {
                    if let Some(confirm) = app.confirm.take() {
                        app.log_styled(
                            confirm.action.label(),
                            Style::default().add_modifier(ratatui::style::Modifier::BOLD),
                        );
                        match (confirm.needs_root, confirm.action.target()) {
                            // Escalation is offered for the system target only.
                            // pkexec resets the environment, and only the system
                            // layout is environment-free — escalating a *user*
                            // action would resolve $HOME to /root in the child
                            // and install somewhere the user never confirmed.
                            (true, Some(Target::System)) => {
                                app.busy = Some(confirm.action.label());
                                run_escalated(
                                    app,
                                    confirm.action,
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
                                app.busy = Some(confirm.action.label());
                                spawn_action(tx, confirm.action, dirs);
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
    for line in actions::explain(action, &plan) {
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
            println!("Root is only offered for the system target — check the ownership");
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
            eprintln!("Root is only offered for the system target — check the ownership");
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
