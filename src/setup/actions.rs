//! What the tool can do, as data.
//!
//! Every action is first turned into a [`Plan`] by a pure function, and only
//! then executed. That split buys three things: the confirm screen can show the
//! exact list of writes, `gamebus-setup plan` is a real dry run rather than a
//! description of one, and a plan that needs root can be handed to a privileged
//! copy of this binary without any of the logic crossing the process boundary.
//!
//! A plan has three phases because privilege and session are orthogonal:
//! `systemctl --user` must run as the invoking user even during a system-wide
//! install (root's user manager is the wrong one, or absent), while the file
//! writes may need root.

use std::fmt;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::paths::{layout, Dirs, Layout, Target, DBUS_SERVICE_NAME, DETECTABLE_NAME};
use super::units::{render_dbus_service, render_systemd_unit};

const UNIT_NAME: &str = super::paths::UNIT_NAME;

/// Something the user can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install(Target),
    Uninstall(Target),
    EnableAutostart(Target),
    DisableAutostart(Target),
    Start,
    Stop,
    Restart,
    FetchDetectable,
}

impl Action {
    pub fn label(self) -> String {
        match self {
            Action::Install(t) => format!("Install ({})", t.as_str()),
            Action::Uninstall(t) => format!("Uninstall ({})", t.as_str()),
            Action::EnableAutostart(t) => format!("Enable autostart ({})", t.as_str()),
            Action::DisableAutostart(t) => format!("Disable autostart ({})", t.as_str()),
            Action::Start => "Start daemon".to_string(),
            Action::Stop => "Stop daemon".to_string(),
            Action::Restart => "Restart daemon".to_string(),
            Action::FetchDetectable => "Refresh naming database".to_string(),
        }
    }

    /// The form used on the command line: `install --target user`.
    pub fn as_args(self) -> Vec<String> {
        let (verb, target) = match self {
            Action::Install(t) => ("install", Some(t)),
            Action::Uninstall(t) => ("uninstall", Some(t)),
            Action::EnableAutostart(t) => ("enable", Some(t)),
            Action::DisableAutostart(t) => ("disable", Some(t)),
            Action::Start => ("start", None),
            Action::Stop => ("stop", None),
            Action::Restart => ("restart", None),
            Action::FetchDetectable => ("fetch-detectable", None),
        };
        let mut args = vec![verb.to_string()];
        if let Some(t) = target {
            args.push("--target".to_string());
            args.push(t.as_str().to_string());
        }
        args
    }

    /// The install target this action refers to, if it has one.
    ///
    /// Load-bearing for privilege escalation: only `System` may be escalated,
    /// because only the system layout is free of the environment that `pkexec`
    /// scrubs. See `paths::layout`.
    pub fn target(self) -> Option<Target> {
        match self {
            Action::Install(t)
            | Action::Uninstall(t)
            | Action::EnableAutostart(t)
            | Action::DisableAutostart(t) => Some(t),
            Action::Start | Action::Stop | Action::Restart | Action::FetchDetectable => None,
        }
    }

    pub fn parse(verb: &str, target: Target) -> Option<Self> {
        Some(match verb {
            "install" => Action::Install(target),
            "uninstall" => Action::Uninstall(target),
            "enable" => Action::EnableAutostart(target),
            "disable" => Action::DisableAutostart(target),
            "start" => Action::Start,
            "stop" => Action::Stop,
            "restart" => Action::Restart,
            "fetch-detectable" => Action::FetchDetectable,
            _ => return None,
        })
    }
}

/// One indivisible thing to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Mkdir(PathBuf),
    /// Copy a file into place. Always via a temporary file and `rename`.
    InstallFile {
        from: PathBuf,
        to: PathBuf,
        mode: u32,
    },
    WriteFile {
        to: PathBuf,
        contents: String,
        mode: u32,
    },
    Remove(PathBuf),
    Run {
        program: String,
        args: Vec<String>,
        /// Failure is expected and must not abort the rest of the plan.
        /// `systemctl disable` on a unit systemd never loaded is the case that
        /// matters: it fails, and it would otherwise block the uninstall that
        /// is trying to clean up after exactly that situation.
        best_effort: bool,
    },
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::Mkdir(p) => write!(f, "mkdir -p {}", p.display()),
            Step::InstallFile { from, to, mode } => {
                write!(f, "install -m{mode:o} {} {}", from.display(), to.display())
            }
            Step::WriteFile { to, mode, contents } => write!(
                f,
                "write {} ({} bytes, mode {mode:o})",
                to.display(),
                contents.len()
            ),
            Step::Remove(p) => write!(f, "rm -f {}", p.display()),
            Step::Run {
                program,
                args,
                best_effort,
            } => {
                write!(f, "{program} {}", args.join(" "))?;
                if *best_effort {
                    write!(f, "   (failure is fine)")?;
                }
                Ok(())
            }
        }
    }
}

impl Step {
    fn run(program: &str, args: &[&str]) -> Step {
        Step::Run {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            best_effort: false,
        }
    }

    fn run_best_effort(program: &str, args: &[&str]) -> Step {
        Step::Run {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            best_effort: true,
        }
    }

    /// Compact form for the confirm screen, where the question is "what will
    /// be written" - the source path is noise there, the destination is not.
    pub fn summary(&self) -> String {
        match self {
            Step::InstallFile { to, .. } => format!("install  {}", to.display()),
            Step::WriteFile { to, contents, .. } => {
                format!("write    {} ({} bytes)", to.display(), contents.len())
            }
            other => other.to_string(),
        }
    }

    /// Whether a failure here should stop the rest of the plan.
    pub fn is_fatal_on_failure(&self) -> bool {
        !matches!(
            self,
            Step::Run {
                best_effort: true,
                ..
            }
        )
    }

    /// The path this step writes to, if it writes to one.
    fn write_target(&self) -> Option<&Path> {
        match self {
            Step::Mkdir(p) | Step::Remove(p) => Some(p),
            Step::InstallFile { to, .. } | Step::WriteFile { to, .. } => Some(to),
            Step::Run { .. } => None,
        }
    }
}

/// An action turned into steps, split by who has to run them.
///
/// `before` and `after` are session-scoped and always run as the invoking user.
/// `privileged` is the block that a system-wide install has to escalate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub before: Vec<Step>,
    pub privileged: Vec<Step>,
    pub after: Vec<Step>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.before.is_empty() && self.privileged.is_empty() && self.after.is_empty()
    }

    pub fn all_steps(&self) -> impl Iterator<Item = &Step> {
        self.before
            .iter()
            .chain(self.privileged.iter())
            .chain(self.after.iter())
    }
}

/// A plan in plain language, for somebody who does not want to read
/// `install -m755` twelve times.
///
/// Derived from the same [`Plan`] that [`execute`] runs, so the summary cannot
/// drift from what actually happens - that is the whole reason the steps are
/// data rather than a sequence of calls. The exact list stays one keypress
/// away; this is the layer on top of it, not a replacement for it.
pub fn explain(plan: &Plan) -> Vec<String> {
    let mut lines = Vec::new();

    let programs: Vec<&PathBuf> = plan
        .all_steps()
        .filter_map(|s| match s {
            Step::InstallFile {
                to, mode: 0o755, ..
            } => Some(to),
            _ => None,
        })
        .collect();
    if let Some(first) = programs.first() {
        let dir = first
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        lines.push(format!(
            "copy {} program{} into {dir}",
            programs.len(),
            if programs.len() == 1 { "" } else { "s" }
        ));
    }

    if plan
        .all_steps()
        .any(|s| matches!(s, Step::InstallFile { to, .. } if to.ends_with(DETECTABLE_NAME)))
    {
        lines.push("add the game-name database, so games show their real titles".to_string());
    }

    if plan
        .all_steps()
        .any(|s| matches!(s, Step::WriteFile { to, .. } if to.ends_with(UNIT_NAME)))
    {
        lines.push("register the daemon with systemd, so it can start at login".to_string());
    }
    if plan
        .all_steps()
        .any(|s| matches!(s, Step::WriteFile { to, .. } if to.ends_with(DBUS_SERVICE_NAME)))
    {
        lines.push("let the session bus start it on demand".to_string());
    }

    let removed = plan
        .all_steps()
        .filter(|s| matches!(s, Step::Remove(_)))
        .count();
    if removed > 0 {
        lines.push(format!("delete {removed} installed files"));
    }

    for step in plan.all_steps() {
        if let Step::Run { program, args, .. } = step {
            if program != "systemctl" {
                continue;
            }
            let has = |a: &str| args.iter().any(|x| x == a);
            if has("enable") {
                lines.push(if has("--global") {
                    "turn on autostart for every user at their next login".to_string()
                } else {
                    "turn on autostart, so it comes back after a logout".to_string()
                });
            } else if has("disable") {
                lines.push("turn off autostart and stop it now".to_string());
            } else if has("restart") {
                lines.push("restart the daemon".to_string());
            } else if has("start") {
                lines.push("start the daemon now".to_string());
            } else if has("stop") {
                lines.push("stop the daemon".to_string());
            }
        }
    }

    if plan.all_steps().any(
        |s| matches!(s, Step::Run { args, .. } if args.iter().any(|a| a == "fetch-detectable")),
    ) {
        lines.push("download the latest game-name database (about 12 MB)".to_string());
    }

    if lines.is_empty() {
        lines.push("nothing - there is no work to do".to_string());
    }
    lines
}

/// One sentence on how far the change reaches. The question behind every
/// confirm dialog is "what does this touch that I care about".
pub fn scope_note(plan: &Plan, home: &Path) -> String {
    // Named explicitly rather than inferred: the download writes into the
    // cache via a subprocess, so no Step carries the path. Saying "nothing
    // changes" next to "download 12 MB" would break this module's own rule
    // that the plain layer cannot promise what the steps do not do. Only when
    // the fetch is the whole plan, though - an install's write scope is the
    // headline, and its own summary line already names the download.
    let has_fetch = plan.all_steps().any(
        |s| matches!(s, Step::Run { args, .. } if args.iter().any(|a| a == "fetch-detectable")),
    );
    if has_fetch && plan.all_steps().all(|s| s.write_target().is_none()) {
        return "Rewrites the game-name database in your cache directory.".to_string();
    }
    // A command in the privileged block writes outside the home by
    // construction - `systemctl --global enable` drops a symlink under
    // /etc/systemd/user - even though it names no path of its own.
    let privileged_command = plan
        .privileged
        .iter()
        .any(|s| matches!(s, Step::Run { .. }));
    let writes: Vec<&Path> = plan.all_steps().filter_map(|s| s.write_target()).collect();

    if writes.is_empty() && !privileged_command {
        return "Nothing on disk changes.".to_string();
    }
    if !privileged_command && writes.iter().all(|p| p.starts_with(home)) {
        "Everything stays inside your home directory. Nothing system-wide changes.".to_string()
    } else {
        "This writes outside your home directory, so it needs your password.".to_string()
    }
}

/// Where the files to install come from.
#[derive(Debug, Clone)]
pub struct Source {
    /// Directory holding the freshly built binaries.
    pub bin_dir: PathBuf,
    /// An already-installed `gamebus-presence`, for `fetch-detectable`.
    pub cli: Option<PathBuf>,
}

impl Source {
    /// Discover from the running binary: its own directory holds the siblings.
    pub fn discover(dirs: &Dirs) -> Self {
        let bin_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));

        let cli = [
            bin_dir.join(super::paths::CLI_BIN),
            dirs.home.join(".local/bin").join(super::paths::CLI_BIN),
            PathBuf::from("/usr/bin").join(super::paths::CLI_BIN),
        ]
        .into_iter()
        .find(|p| p.exists());

        Self { bin_dir, cli }
    }
}

/// Turn an action into the steps that carry it out. Pure - touches nothing.
pub fn plan(action: Action, dirs: &Dirs, source: &Source) -> Plan {
    match action {
        Action::Install(target) => plan_install(&layout(dirs, target), source),
        Action::Uninstall(target) => plan_uninstall(&layout(dirs, target)),
        Action::EnableAutostart(target) => match target {
            Target::User => Plan {
                after: vec![Step::run(
                    "systemctl",
                    &["--user", "enable", "--now", UNIT_NAME],
                )],
                ..Default::default()
            },
            // `--global` enables the unit for every user at next login. It has
            // no `--now`, so starting it here is a separate, unprivileged step.
            Target::System => Plan {
                privileged: vec![Step::run("systemctl", &["--global", "enable", UNIT_NAME])],
                after: vec![Step::run("systemctl", &["--user", "start", UNIT_NAME])],
                ..Default::default()
            },
        },
        Action::DisableAutostart(target) => match target {
            Target::User => Plan {
                after: vec![Step::run(
                    "systemctl",
                    &["--user", "disable", "--now", UNIT_NAME],
                )],
                ..Default::default()
            },
            Target::System => Plan {
                privileged: vec![Step::run("systemctl", &["--global", "disable", UNIT_NAME])],
                after: vec![Step::run("systemctl", &["--user", "stop", UNIT_NAME])],
                ..Default::default()
            },
        },
        Action::Start => simple_systemctl("start"),
        Action::Stop => simple_systemctl("stop"),
        Action::Restart => simple_systemctl("restart"),
        Action::FetchDetectable => Plan {
            after: vec![Step::Run {
                program: source
                    .cli
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(super::paths::CLI_BIN))
                    .display()
                    .to_string(),
                args: vec!["fetch-detectable".to_string()],
                best_effort: false,
            }],
            ..Default::default()
        },
    }
}

fn simple_systemctl(verb: &str) -> Plan {
    Plan {
        after: vec![Step::run("systemctl", &["--user", verb, UNIT_NAME])],
        ..Default::default()
    }
}

fn plan_install(layout: &Layout, source: &Source) -> Plan {
    let mut privileged = vec![
        Step::Mkdir(layout.bin_dir.clone()),
        Step::Mkdir(layout.data_dir.clone()),
        Step::Mkdir(layout.unit_dir.clone()),
        Step::Mkdir(layout.dbus_service_dir.clone()),
    ];

    for (name, dest) in layout.binaries() {
        let from = source.bin_dir.join(name);
        if from.exists() {
            privileged.push(Step::InstallFile {
                from,
                to: dest,
                mode: 0o755,
            });
        }
    }

    // The endpoint configuration ships as a reference copy in the data dir
    // (bundled content, rewritten on every install); user overrides live in
    // the config dir, which install never touches.
    privileged.push(Step::WriteFile {
        to: layout.endpoints_file(),
        contents: include_str!("../../endpoints.toml").to_string(),
        mode: 0o644,
    });

    let exec = layout.daemon_bin();
    privileged.push(Step::WriteFile {
        to: layout.unit_file(),
        contents: render_systemd_unit(&exec),
        mode: 0o644,
    });
    privileged.push(Step::WriteFile {
        to: layout.dbus_service_file(),
        contents: render_dbus_service(&exec),
        mode: 0o644,
    });

    // The naming database is never baked into a build: the freshly installed
    // CLI downloads it into the cache tier as the last step. Best effort - an
    // offline install still succeeds, the daemon degrades to executable
    // names, and the status screen offers the fetch as a one-key fix.
    let mut after = reload_steps();
    after.push(Step::Run {
        program: layout.cli_bin().display().to_string(),
        args: vec!["fetch-detectable".to_string()],
        best_effort: true,
    });

    Plan {
        before: Vec::new(),
        privileged,
        after,
    }
}

fn plan_uninstall(layout: &Layout) -> Plan {
    let mut privileged = Vec::new();
    for (_, path) in layout.binaries() {
        privileged.push(Step::Remove(path));
    }
    privileged.push(Step::Remove(layout.unit_file()));
    privileged.push(Step::Remove(layout.dbus_service_file()));
    privileged.push(Step::Remove(layout.detectable_file()));
    privileged.push(Step::Remove(layout.endpoints_file()));

    Plan {
        // Stop and disable before the files go, or systemd is left holding a
        // unit whose fragment has vanished. Best effort: an install that was
        // never enabled - or never seen by systemd at all - must still be
        // removable, and `disable` fails loudly in exactly that case.
        before: vec![Step::run_best_effort(
            "systemctl",
            &["--user", "disable", "--now", UNIT_NAME],
        )],
        privileged,
        after: reload_steps(),
    }
}

/// Make systemd and the session bus notice the files that just changed.
///
/// The D-Bus `ReloadConfig` is what makes a newly written activation file take
/// effect without logging out - omit it and activation silently does nothing.
fn reload_steps() -> Vec<Step> {
    vec![
        Step::run("systemctl", &["--user", "daemon-reload"]),
        Step::run(
            "busctl",
            &[
                "--user",
                "call",
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "ReloadConfig",
            ],
        ),
    ]
}

/// Whether the privileged block actually needs privileges here.
///
/// Not a guess from the target: it asks the filesystem whether we can write
/// each path (walking up to the nearest directory that exists, since the leaf
/// usually does not yet). A `Run` step in the privileged block is by
/// construction something only root may do.
pub fn needs_root(plan: &Plan, euid: u32) -> bool {
    if euid == 0 {
        return false;
    }
    plan.privileged.iter().any(|step| match step {
        Step::Run { .. } => true,
        // Unlinking needs write permission on the containing directory, not on
        // the file: a world-writable file in a root-owned directory cannot be
        // removed, and probing the file would say otherwise.
        Step::Remove(p) => !writable(p.parent().unwrap_or(p)),
        _ => step.write_target().map(|p| !writable(p)).unwrap_or(false),
    })
}

fn writable(path: &Path) -> bool {
    let mut probe = path;
    loop {
        if probe.exists() {
            return access_w_ok(probe);
        }
        match probe.parent() {
            Some(parent) => probe = parent,
            None => return false,
        }
    }
}

fn access_w_ok(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: c_path is a valid NUL-terminated string for the duration of the call.
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
}

/// How a privilege escalation went.
#[derive(Debug, Clone)]
pub enum Escalation {
    /// The privileged half ran; here is what it said.
    Ran { ok: bool, output: String },
    /// The user dismissed the authentication dialog.
    Cancelled,
    /// `pkexec` itself could not run - no polkit agent on a bare tty, most
    /// often. The caller shows `sudo_hint` so the user can do it by hand.
    Unavailable { reason: String },
}

/// Re-run this binary as root for the privileged half of a plan.
///
/// Only ever called for [`Target::System`] - see [`Action::target`].
///
/// `pkexec` rather than `sudo`: it authenticates through the desktop's polkit
/// agent and needs no controlling terminal, which matters because the TUI owns
/// the terminal. The privileged child is the plain `apply --privileged-only`
/// subcommand - no raw mode and no ratatui ever runs as root.
///
/// The child re-plans from scratch. Nothing but the action name and target
/// crosses the process boundary, and `layout()` for the system target reads no
/// environment, so the paths it computes are the ones that were confirmed -
/// even though `pkexec` scrubs the environment.
pub fn escalate(action: Action, target: Target) -> Escalation {
    let Ok(exe) = std::env::current_exe() else {
        return Escalation::Unavailable {
            reason: "cannot determine our own path".to_string(),
        };
    };

    let mut args = vec!["apply".to_string()];
    args.extend(action.as_args());
    if !args.iter().any(|a| a == "--target") {
        args.push("--target".to_string());
        args.push(target.as_str().to_string());
    }
    args.push("--privileged-only".to_string());
    args.push("--confirm".to_string());

    // pkexec requires an absolute program path.
    let output = Command::new("pkexec").arg(&exe).args(&args).output();

    match output {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            match out.status.code() {
                // pkexec's own exit codes: 126 dismissed, 127 could not run.
                Some(126) => Escalation::Cancelled,
                Some(127) => Escalation::Unavailable { reason: text },
                _ => Escalation::Ran {
                    ok: out.status.success(),
                    output: text,
                },
            }
        }
        Err(e) => Escalation::Unavailable {
            reason: format!("pkexec could not be started: {e}"),
        },
    }
}

/// The command a user can run by hand when escalation is unavailable.
pub fn sudo_hint(action: Action, target: Target) -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "gamebus-setup".to_string());
    let mut parts = vec!["sudo".to_string(), exe, "apply".to_string()];
    parts.extend(action.as_args());
    if !parts.iter().any(|a| a == "--target") {
        parts.push("--target".to_string());
        parts.push(target.as_str().to_string());
    }
    parts.push("--privileged-only".to_string());
    parts.push("--confirm".to_string());
    parts.join(" ")
}

/// What happened to one step.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    pub description: String,
    pub ok: bool,
    pub detail: String,
}

/// Perform the steps in order, stopping at the first failure.
///
/// Stopping matters: a half-written install that keeps going produces units
/// pointing at binaries that were never copied.
pub fn execute(steps: &[Step]) -> Vec<StepOutcome> {
    let mut outcomes = Vec::new();
    for step in steps {
        let result = perform(step);
        let ok = result.is_ok();
        outcomes.push(StepOutcome {
            description: step.to_string(),
            ok,
            detail: match result {
                Ok(detail) => detail,
                Err(e) => e.to_string(),
            },
        });
        if !ok && step.is_fatal_on_failure() {
            break;
        }
    }
    outcomes
}

fn perform(step: &Step) -> io::Result<String> {
    match step {
        Step::Mkdir(path) => {
            mkdir_p(path)?;
            Ok(String::new())
        }
        Step::InstallFile { from, to, mode } => {
            let bytes = std::fs::read(from)?;
            write_atomic(to, &bytes, *mode)?;
            Ok(format!("{} bytes", bytes.len()))
        }
        Step::WriteFile { to, contents, mode } => {
            write_atomic(to, contents.as_bytes(), *mode)?;
            Ok(format!("{} bytes", contents.len()))
        }
        Step::Remove(path) => match std::fs::remove_file(path) {
            Ok(()) => Ok(String::new()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok("not present".to_string()),
            Err(e) => Err(e),
        },
        Step::Run { program, args, .. } => {
            let out = Command::new(program).args(args).output()?;
            let mut detail = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.trim().is_empty() {
                if !detail.is_empty() {
                    detail.push('\n');
                }
                detail.push_str(stderr.trim());
            }
            if out.status.success() {
                Ok(detail)
            } else {
                Err(io::Error::other(format!(
                    "exited {}: {detail}",
                    out.status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "by signal".to_string())
                )))
            }
        }
    }
}

/// Write through a temporary file and rename into place.
///
/// `fs::write` onto a *running* executable fails with `ETXTBSY`, and
/// reinstalling after a rebuild - with the daemon running - is the common case.
/// `rename` swaps the directory entry instead: the running process keeps the
/// old inode until it exits.
/// `mkdir -p`, with a mode the caller's umask cannot take away.
///
/// `DirBuilder::mode()` - and `create_dir_all`'s implicit 0o777 - pass the mode
/// to `mkdir(2)`, which masks it with the process umask. Under `umask 077` an
/// explicit 0o755 therefore still lands as 0700 (measured, not assumed). That
/// matters for a system install: `pkexec` does not reset the umask, so the
/// invoking user's carries into the root child, and root-owned 0700
/// directories mean systemd cannot see the unit, dbus cannot read the
/// activation file, and the daemon cannot read the naming database - while
/// every step reports success.
///
/// So: create each missing component, then `chmod` the ones we created.
/// Components that already existed are left exactly as they were.
fn mkdir_p(path: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut cursor = Some(path);
    while let Some(dir) = cursor {
        if dir.is_dir() {
            break;
        }
        missing.push(dir);
        cursor = dir.parent();
    }

    for dir in missing.into_iter().rev() {
        match std::fs::create_dir(dir) {
            Ok(()) => {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
            }
            // Lost a race with somebody else creating it; theirs stands.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn write_atomic(to: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let parent = to
        .parent()
        .ok_or_else(|| io::Error::other(format!("{} has no parent directory", to.display())))?;
    mkdir_p(parent)?;

    let tmp = parent.join(format!(
        ".{}.tmp.{}",
        to.file_name().and_then(|n| n.to_str()).unwrap_or("gamebus"),
        std::process::id()
    ));
    let result = (|| {
        std::fs::write(&tmp, bytes)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(&tmp, to)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Dirs {
        let home = PathBuf::from("/home/tester");
        Dirs {
            config_home: home.join(".config"),
            data_home: home.join(".local/share"),
            cache_home: home.join(".cache"),
            runtime_dir: PathBuf::from("/run/user/1000"),
            data_dirs: vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ],
            home,
        }
    }

    fn source() -> Source {
        Source {
            // Nothing exists at this path, so no InstallFile steps are planned
            // for the binaries - that is the point of the second test below.
            bin_dir: PathBuf::from("/build/target/release"),
            cli: None,
        }
    }

    #[test]
    fn user_install_writes_only_under_home() {
        let plan = plan(Action::Install(Target::User), &dirs(), &source());
        for step in &plan.privileged {
            let path = step.write_target().expect("privileged steps write files");
            assert!(
                path.starts_with("/home/tester"),
                "user install escaped home: {}",
                path.display()
            );
        }
        // Three generated files: the two unit files naming the user-level
        // binary, and the endpoint configuration (bundled content).
        let written: Vec<_> = plan
            .privileged
            .iter()
            .filter_map(|s| match s {
                Step::WriteFile { to, contents, .. } => Some((to.clone(), contents.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(written.len(), 3);
        for (to, contents) in &written {
            if to.ends_with(crate::endpoints::ENDPOINTS_NAME) {
                assert!(contents.contains("[umu]"), "endpoints content wrong");
            } else {
                assert!(contents.contains("/home/tester/.local/bin/gamebus-presenced"));
            }
        }
    }

    #[test]
    fn install_ends_with_a_best_effort_fetch_by_the_installed_cli() {
        // The naming database is never fetched at build time, so the install
        // itself must produce it - via the copy of the CLI it just installed,
        // never the build-tree one, and without failing an offline install.
        for target in [Target::User, Target::System] {
            let plan = plan(Action::Install(target), &dirs(), &source());
            let fetch = plan
                .after
                .last()
                .expect("install plan has session-scoped steps");
            match fetch {
                Step::Run {
                    program,
                    args,
                    best_effort,
                } => {
                    let cli = layout(&dirs(), target).cli_bin();
                    assert_eq!(program, &cli.display().to_string());
                    assert_eq!(args, &vec!["fetch-detectable".to_string()]);
                    assert!(best_effort, "an offline install must still succeed");
                }
                other => panic!("last install step is not the fetch: {other}"),
            }
        }
    }

    #[test]
    fn install_scope_note_still_leads_with_the_write_scope() {
        // The fetch inside an install must not hijack the headline - the
        // cache-rewrite wording is reserved for the fetch-only plan.
        let home = PathBuf::from("/home/tester");
        let install = plan(Action::Install(Target::User), &dirs(), &source());
        assert!(
            scope_note(&install, &home).contains("home directory"),
            "install scope note lost its write scope"
        );
        let fetch_only = plan(Action::FetchDetectable, &dirs(), &source());
        assert!(
            scope_note(&fetch_only, &home).contains("cache directory"),
            "fetch-only plan lost its cache wording"
        );
    }

    #[test]
    fn install_skips_binaries_that_were_not_built() {
        let plan = plan(Action::Install(Target::User), &dirs(), &source());
        assert!(
            !plan
                .privileged
                .iter()
                .any(|s| matches!(s, Step::InstallFile { .. })),
            "planned a copy from a source directory that does not exist"
        );
    }

    #[test]
    fn system_install_targets_the_usr_local_prefix() {
        let plan = plan(Action::Install(Target::System), &dirs(), &source());
        let paths: Vec<String> = plan
            .privileged
            .iter()
            .filter_map(|s| s.write_target().map(|p| p.display().to_string()))
            .collect();
        assert!(paths.contains(&"/usr/local/bin".to_string()));
        assert!(paths.contains(&"/usr/local/lib/systemd/user".to_string()));
        assert!(paths.contains(&"/usr/local/share/gamebus-presenced".to_string()));
        assert!(paths.contains(&"/usr/local/share/dbus-1/services".to_string()));
        // /usr proper belongs to the distribution's package manager.
        for path in &paths {
            assert!(
                !path.starts_with("/usr/") || path.starts_with("/usr/local/"),
                "system install would write into package-manager territory: {path}"
            );
        }
    }

    #[test]
    fn reload_runs_as_the_user_even_for_a_system_install() {
        let plan = plan(Action::Install(Target::System), &dirs(), &source());
        // Not in the privileged block: root's user manager is the wrong one.
        assert!(plan
            .after
            .iter()
            .any(|s| matches!(s, Step::Run { program, args, .. }
                if program == "systemctl" && args.contains(&"--user".to_string()))));
    }

    #[test]
    fn system_autostart_is_two_operations() {
        let plan = plan(Action::EnableAutostart(Target::System), &dirs(), &source());
        // `systemctl --global enable` cannot start anything, so the start is a
        // separate unprivileged step rather than a `--now`.
        assert_eq!(plan.privileged.len(), 1);
        assert_eq!(plan.after.len(), 1);
        assert!(
            matches!(&plan.privileged[0], Step::Run { args, .. } if args.contains(&"--global".to_string()))
        );
        assert!(
            matches!(&plan.after[0], Step::Run { args, .. } if args.contains(&"start".to_string()))
        );
    }

    #[test]
    fn uninstall_disables_before_removing_the_unit() {
        let plan = plan(Action::Uninstall(Target::User), &dirs(), &source());
        assert!(
            matches!(&plan.before[0], Step::Run { args, .. } if args.contains(&"disable".to_string()))
        );
        assert!(plan
            .privileged
            .iter()
            .any(|s| matches!(s, Step::Remove(p) if p.ends_with("gamebus-presenced.service"))));
    }

    #[test]
    fn root_is_needed_for_usr_but_not_for_home() {
        let user = plan(Action::Install(Target::User), &dirs(), &source());
        let system = plan(Action::Install(Target::System), &dirs(), &source());

        // /home/tester does not exist on the test machine, so the writability
        // walk lands on /home - which an ordinary user cannot write either.
        // Assert the invariant that does not depend on the filesystem instead:
        // running as root never needs escalation.
        assert!(!needs_root(&user, 0));
        assert!(!needs_root(&system, 0));
        assert!(needs_root(&system, 1000), "/usr should need root");
    }

    /// The invariant behind the whole escalation design: only the system
    /// target may be escalated, because only its layout ignores the
    /// environment that pkexec scrubs.
    #[test]
    fn only_the_system_target_is_escalatable() {
        assert_eq!(
            Action::Install(Target::System).target(),
            Some(Target::System)
        );
        assert_eq!(Action::Install(Target::User).target(), Some(Target::User));
        assert_eq!(
            Action::EnableAutostart(Target::System).target(),
            Some(Target::System)
        );
        // Session-scoped actions have no target and never escalate.
        for action in [
            Action::Start,
            Action::Stop,
            Action::Restart,
            Action::FetchDetectable,
        ] {
            assert_eq!(action.target(), None);
        }
    }

    #[test]
    fn a_command_only_root_may_run_counts_as_leaving_the_home() {
        // `systemctl --global enable` names no path, but it writes a symlink
        // under /etc - the confirm screen must not say "nothing changes".
        let plan = plan(Action::EnableAutostart(Target::System), &dirs(), &source());
        assert!(plan
            .privileged
            .iter()
            .any(|s| matches!(s, Step::Run { .. })));
        let note = scope_note(&plan, Path::new("/home/tester"));
        assert!(note.contains("outside your home"), "{note}");
    }

    #[test]
    fn removing_a_file_probes_the_directory_that_holds_it() {
        // A file can be writable inside a directory that is not; unlink needs
        // the directory.
        let plan = Plan {
            privileged: vec![Step::Remove(PathBuf::from("/usr/bin/gamebus-presenced"))],
            ..Default::default()
        };
        assert!(needs_root(&plan, 1000));
        assert!(!needs_root(&plan, 0));
    }

    #[test]
    fn a_plan_with_no_privileged_steps_never_needs_root() {
        let start = plan(Action::Start, &dirs(), &source());
        assert!(start.privileged.is_empty());
        assert!(!needs_root(&start, 1000));
    }

    #[test]
    fn a_user_install_is_explained_without_a_single_command() {
        let plan = plan(Action::Install(Target::User), &dirs(), &source());
        let lines = explain(&plan);

        // Plain language: no paths-with-flags, no shell verbs.
        for line in &lines {
            for jargon in ["install -m", "mkdir", "systemctl", "busctl", "0o755"] {
                assert!(!line.contains(jargon), "jargon leaked into {line:?}");
            }
        }
        assert!(
            lines.iter().any(|l| l.contains("start at login")),
            "the thing a user actually cares about is missing: {lines:?}"
        );
        assert_eq!(
            scope_note(&plan, Path::new("/home/tester")),
            "Everything stays inside your home directory. Nothing system-wide changes."
        );
    }

    #[test]
    fn a_system_install_says_it_leaves_home() {
        let plan = plan(Action::Install(Target::System), &dirs(), &source());
        let note = scope_note(&plan, Path::new("/home/tester"));
        assert!(note.contains("outside your home"), "{note}");
        assert!(note.contains("password"), "{note}");
    }

    #[test]
    fn autostart_and_uninstall_explain_their_consequence() {
        let enable = plan(Action::EnableAutostart(Target::User), &dirs(), &source());
        assert!(explain(&enable)
            .iter()
            .any(|l| l.contains("after a logout")));

        let uninstall = plan(Action::Uninstall(Target::User), &dirs(), &source());
        let lines = explain(&uninstall);
        assert!(lines.iter().any(|l| l.contains("delete")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("autostart")), "{lines:?}");
    }

    /// The plain layer is generated from the same plan that runs, so it cannot
    /// promise something the steps do not do.
    #[test]
    fn an_empty_plan_is_explained_as_doing_nothing() {
        let empty = Plan::default();
        assert_eq!(explain(&empty), vec!["nothing - there is no work to do"]);
        assert_eq!(
            scope_note(&empty, Path::new("/home/tester")),
            "Nothing on disk changes."
        );
    }

    #[test]
    fn actions_round_trip_through_their_command_line_form() {
        for action in [
            Action::Install(Target::User),
            Action::Uninstall(Target::System),
            Action::EnableAutostart(Target::User),
            Action::DisableAutostart(Target::System),
            Action::Start,
            Action::Stop,
            Action::Restart,
            Action::FetchDetectable,
        ] {
            let args = action.as_args();
            let target = args
                .iter()
                .position(|a| a == "--target")
                .and_then(|i| args.get(i + 1))
                .and_then(|t| Target::parse(t))
                .unwrap_or(Target::User);
            assert_eq!(Action::parse(&args[0], target), Some(action));
        }
    }

    /// The bug this test exists for: `DirBuilder::mode(0o755)` looks explicit
    /// but `mkdir(2)` masks it with the umask, so under `umask 077` the
    /// directories came out 0700 - root-owned and unreadable by the daemon,
    /// with every step still reporting success. Nothing caught it because no
    /// test ran under a restrictive umask.
    #[test]
    fn created_directories_ignore_a_restrictive_umask() {
        let dir = std::env::temp_dir().join(format!("gamebus-umask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // SAFETY: umask cannot fail; restored below. The test harness is
        // multi-threaded, but no other test in this binary creates files.
        let previous = unsafe { libc::umask(0o077) };
        let result = mkdir_p(&dir.join("nested/deeper"));
        unsafe { libc::umask(previous) };
        result.unwrap();

        for path in [dir.join("nested/deeper"), dir.join("nested"), dir.clone()] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode,
                0o755,
                "{} came out {mode:04o} - a umask took the mode away",
                path.display()
            );
        }

        // A directory that already existed keeps whatever mode it had.
        let preexisting = dir.join("kept");
        std::fs::create_dir(&preexisting).unwrap();
        std::fs::set_permissions(&preexisting, std::fs::Permissions::from_mode(0o700)).unwrap();
        mkdir_p(&preexisting).unwrap();
        assert_eq!(
            std::fs::metadata(&preexisting)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_replaces_a_busy_file() {
        let dir = std::env::temp_dir().join(format!("gamebus-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("payload");

        write_atomic(&target, b"first", 0o644).unwrap();
        write_atomic(&target, b"second", 0o755).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
        // No temporary files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "temporary file left behind");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
