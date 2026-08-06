//! What is actually going on: probe the system, then judge it.
//!
//! [`probe`] does all the I/O and returns plain data. [`rows`] and [`overall`]
//! are pure functions from that data to what gets displayed, including the
//! remedy offered for each problem. All the judgement therefore unit-tests
//! without a bus, a terminal, or root.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use super::actions::Action;
use super::paths::{
    detectable_candidates, layout, DetectableTier, Dirs, Layout, Target, CLI_BIN, DAEMON_BIN,
    SETUP_BIN, UNIT_NAME,
};
use super::units::parse_exec;

pub const BUS_NAME: &str = "org.gamebus.Presence.v1";
const GAMEMODE_NAME: &str = "com.feralinteractive.GameMode";
/// Every bus call is bounded: a wedged daemon must not wedge the status view.
pub const BUS_TIMEOUT: Duration = Duration::from_secs(2);

/// How a single check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Health {
    Ok,
    Unknown,
    Warn,
    Bad,
}

impl Health {
    pub fn marker(self) -> &'static str {
        match self {
            Health::Ok => "●",
            Health::Warn => "▲",
            Health::Bad => "✕",
            Health::Unknown => "?",
        }
    }
}

/// One line in the status view.
#[derive(Debug, Clone)]
pub struct Row {
    pub label: &'static str,
    pub health: Health,
    pub value: String,
    pub hint: Option<String>,
    pub remedy: Option<Action>,
}

/// What `stat(2)` says about one path.
#[derive(Debug, Clone, Default)]
pub struct FileFact {
    pub path: PathBuf,
    pub exists: bool,
    pub len: u64,
    pub mtime: Option<SystemTime>,
    pub executable: bool,
}

pub fn stat(path: &Path) -> FileFact {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(md) => FileFact {
            path: path.to_path_buf(),
            exists: true,
            len: md.len(),
            mtime: md.modified().ok(),
            executable: md.permissions().mode() & 0o111 != 0,
        },
        Err(_) => FileFact {
            path: path.to_path_buf(),
            ..Default::default()
        },
    }
}

/// `systemctl is-enabled`, as far as we care.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnabledState {
    Enabled,
    Disabled,
    NotFound,
    Masked,
    Static,
    Other(String),
    /// systemctl could not be run at all — not the same as "disabled".
    Unavailable(String),
}

/// `systemctl is-active`, as far as we care.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveState {
    Active,
    Inactive,
    Failed,
    Activating,
    Other(String),
    Unavailable(String),
}

/// Parse `systemctl is-enabled` output.
///
/// **Deliberately ignores the exit code.** systemd's codes are not stable
/// across versions — this machine returns 4 for `not-found` where the
/// documentation suggests 2 — but the word on stdout has been stable for years.
pub fn parse_is_enabled(stdout: &str, _code: Option<i32>) -> EnabledState {
    match stdout.trim() {
        "" => EnabledState::Unavailable("no output from systemctl".to_string()),
        "enabled" | "enabled-runtime" => EnabledState::Enabled,
        "disabled" => EnabledState::Disabled,
        "not-found" => EnabledState::NotFound,
        "masked" | "masked-runtime" => EnabledState::Masked,
        "static" | "indirect" | "generated" | "transient" | "alias" => EnabledState::Static,
        other => EnabledState::Other(other.to_string()),
    }
}

/// Parse `systemctl is-active` output. Same reasoning as above.
pub fn parse_is_active(stdout: &str, _code: Option<i32>) -> ActiveState {
    match stdout.trim() {
        "" => ActiveState::Unavailable("no output from systemctl".to_string()),
        "active" => ActiveState::Active,
        "inactive" => ActiveState::Inactive,
        "failed" => ActiveState::Failed,
        "activating" | "reloading" | "deactivating" => ActiveState::Activating,
        other => ActiveState::Other(other.to_string()),
    }
}

/// Who holds a Discord IPC socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketOwner {
    /// Nothing at that path.
    Absent,
    /// A socket file whose listener is gone — what a SIGKILLed daemon leaves.
    Stale,
    /// Somebody is listening, and this is who.
    Listening { pid: u32, comm: String },
}

/// Read the *listening* process's credentials by connecting to it.
///
/// `SO_PEERCRED` on the client side reports the server, which is the mirror of
/// what `src/sources/discord/mod.rs` does to identify its clients. It needs no
/// privileges and no `ss`/`lsof` subprocess.
pub fn probe_socket_owner(path: &Path) -> SocketOwner {
    use std::os::unix::io::AsRawFd;
    if !path.exists() {
        return SocketOwner::Absent;
    }
    let Ok(stream) = std::os::unix::net::UnixStream::connect(path) else {
        return SocketOwner::Stale;
    };

    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are correctly sized for SO_PEERCRED, and the fd
    // is owned by `stream` for the duration of the call.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 || cred.pid <= 0 {
        return SocketOwner::Listening {
            pid: 0,
            comm: "unknown".to_string(),
        };
    }
    let pid = cred.pid as u32;
    SocketOwner::Listening {
        pid,
        comm: comm_of(pid).unwrap_or_else(|| "unknown".to_string()),
    }
}

fn comm_of(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

/// What the Discord source will end up doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscordMode {
    /// We hold `discord-ipc-0` and no upstream exists: games talk to us.
    Standalone,
    /// We hold `discord-ipc-0` and a real Discord is upstream: we pass through.
    Proxy { pid: u32, comm: String },
    /// Somebody else holds `discord-ipc-0`. The daemon returns silently from
    /// its listener in this case — no signal, no log after startup, no bus
    /// state. Nothing else on the system will tell the user.
    Blocked { pid: u32, comm: String },
    /// Nobody holds it, including us — normal when the daemon is not running.
    Idle,
}

#[derive(Debug, Clone)]
pub struct DiscordFacts {
    pub ipc0: SocketOwner,
    pub upstream: Option<(u32, String)>,
    pub mode: DiscordMode,
}

/// Whether GameMode can be reached. It is D-Bus activatable, so "not currently
/// running" is not the same as "not available".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameModeState {
    Running(u32),
    Activatable,
    Absent,
}

#[derive(Debug, Clone, Default)]
pub struct BusFacts {
    pub session_bus: bool,
    pub owned: bool,
    pub owner_pid: Option<u32>,
    pub owner_exe: Option<PathBuf>,
    pub version: Option<u64>,
    pub has_activity: bool,
    pub activities: usize,
    pub error: Option<String>,
}

/// What is installed for one target.
#[derive(Debug, Clone)]
pub struct InstallFacts {
    pub target: Target,
    pub layout: Layout,
    pub daemon: FileFact,
    pub cli: FileFact,
    pub setup: FileFact,
    pub unit: FileFact,
    pub unit_exec: Option<String>,
    pub dbus_service: FileFact,
    pub dbus_exec: Option<String>,
    /// Whether the binary the activation file names actually exists. Resolved
    /// during the probe so that `rows()` stays a pure function.
    pub dbus_exec_exists: bool,
}

impl InstallFacts {
    /// An install counts as present once the daemon binary is in place.
    pub fn installed(&self) -> bool {
        self.daemon.exists
    }
}

#[derive(Debug, Clone)]
pub struct UnitFacts {
    pub enabled: EnabledState,
    pub active: ActiveState,
    /// Which unit file systemd actually loaded — the way to see one install
    /// shadowing another.
    pub fragment_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub euid: u32,
    pub user: InstallFacts,
    pub system: InstallFacts,
    /// What `$PATH` resolves `gamebus-presenced` to, if anything.
    pub path_resolution: Option<PathBuf>,
    /// The freshly built daemon, when running from a build tree.
    pub built_daemon: Option<FileFact>,
    pub unit: UnitFacts,
    pub bus: BusFacts,
    pub gamemode: GameModeState,
    pub detectable: Option<(DetectableTier, FileFact)>,
    pub discord: DiscordFacts,
}

impl Status {
    /// The install the system is actually using, preferring whichever one
    /// provides the daemon binary that is on `$PATH`.
    pub fn primary(&self) -> &InstallFacts {
        match &self.path_resolution {
            Some(p) if p.starts_with(&self.system.layout.bin_dir) => &self.system,
            Some(_) => &self.user,
            None if self.system.installed() && !self.user.installed() => &self.system,
            None => &self.user,
        }
    }
}

/// Gather everything. All the I/O in one place.
pub async fn probe(dirs: &Dirs) -> Status {
    let user = install_facts(dirs, Target::User);
    let system = install_facts(dirs, Target::System);

    // SAFETY: geteuid cannot fail and touches no memory we own.
    let euid = unsafe { libc::geteuid() };

    let (bus, gamemode) = probe_bus().await;

    Status {
        euid,
        path_resolution: which(DAEMON_BIN),
        built_daemon: built_daemon(),
        unit: probe_unit(),
        bus,
        gamemode,
        detectable: probe_detectable(dirs),
        discord: probe_discord(dirs),
        user,
        system,
    }
}

fn install_facts(dirs: &Dirs, target: Target) -> InstallFacts {
    let layout = layout(dirs, target);
    let unit = stat(&layout.unit_file());
    let dbus_service = stat(&layout.dbus_service_file());
    let dbus_exec = read_exec(&dbus_service);
    InstallFacts {
        unit_exec: read_exec(&unit),
        dbus_exec_exists: dbus_exec
            .as_ref()
            .map(|e| Path::new(e).exists())
            .unwrap_or(false),
        dbus_exec,
        daemon: stat(&layout.daemon_bin()),
        cli: stat(&layout.cli_bin()),
        setup: stat(&layout.setup_bin()),
        target,
        layout,
        unit,
        dbus_service,
    }
}

fn read_exec(file: &FileFact) -> Option<String> {
    if !file.exists {
        return None;
    }
    std::fs::read_to_string(&file.path)
        .ok()
        .and_then(|c| parse_exec(&c))
}

/// First match for `name` on `$PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| stat(candidate).executable)
}

/// The daemon in this build tree, if we are running out of one.
fn built_daemon() -> Option<FileFact> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = stat(&dir.join(DAEMON_BIN));
    candidate.exists.then_some(candidate)
}

/// Run systemctl, keeping stdout and stderr apart.
///
/// They must not be merged: the state words the parsers look for come from
/// stdout, and folding stderr in when stdout is empty feeds an error message
/// (`Failed to connect to bus`, on a machine with no user manager) into the
/// state parser, which then reports it as an exotic *state* instead of as
/// "could not ask systemd".
fn systemctl(args: &[&str]) -> Result<(String, String, Option<i32>), String> {
    let out = Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    Ok((
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
        out.status.code(),
    ))
}

fn probe_unit() -> UnitFacts {
    let enabled = match systemctl(&["--user", "is-enabled", UNIT_NAME]) {
        Ok((out, err, _)) if out.trim().is_empty() => {
            EnabledState::Unavailable(if err.is_empty() {
                "no output from systemctl".to_string()
            } else {
                err
            })
        }
        Ok((out, _, code)) => parse_is_enabled(&out, code),
        Err(e) => EnabledState::Unavailable(e),
    };
    let active = match systemctl(&["--user", "is-active", UNIT_NAME]) {
        Ok((out, err, _)) if out.trim().is_empty() => ActiveState::Unavailable(if err.is_empty() {
            "no output from systemctl".to_string()
        } else {
            err
        }),
        Ok((out, _, code)) => parse_is_active(&out, code),
        Err(e) => ActiveState::Unavailable(e),
    };
    let fragment_path = systemctl(&["--user", "show", "-p", "FragmentPath", "--value", UNIT_NAME])
        .ok()
        .map(|(out, _, _)| out.trim().to_string())
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);

    UnitFacts {
        enabled,
        active,
        fragment_path,
    }
}

async fn probe_bus() -> (BusFacts, GameModeState) {
    use zbus::fdo;

    let conn = match tokio::time::timeout(BUS_TIMEOUT, zbus::Connection::session()).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            return (
                BusFacts {
                    error: Some(e.to_string()),
                    ..Default::default()
                },
                GameModeState::Absent,
            )
        }
        Err(_) => {
            return (
                BusFacts {
                    error: Some("timed out connecting to the session bus".to_string()),
                    ..Default::default()
                },
                GameModeState::Absent,
            )
        }
    };

    let mut facts = BusFacts {
        session_bus: true,
        ..Default::default()
    };

    let Ok(Ok(dbus)) = tokio::time::timeout(BUS_TIMEOUT, fdo::DBusProxy::new(&conn)).await else {
        facts.error = Some("could not reach the bus daemon".to_string());
        return (facts, GameModeState::Absent);
    };

    let name = match zbus::names::BusName::try_from(BUS_NAME) {
        Ok(n) => n,
        Err(e) => {
            facts.error = Some(e.to_string());
            return (facts, GameModeState::Absent);
        }
    };
    facts.owned = matches!(
        tokio::time::timeout(BUS_TIMEOUT, dbus.name_has_owner(name.clone())).await,
        Ok(Ok(true))
    );

    if facts.owned {
        // Which binary is on the bus — the only way to catch "I installed to
        // ~/.local/bin but the daemon running is the one from /usr/bin".
        if let Ok(Ok(pid)) = tokio::time::timeout(
            BUS_TIMEOUT,
            dbus.get_connection_unix_process_id(name.clone()),
        )
        .await
        {
            facts.owner_pid = Some(pid);
            facts.owner_exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
        }
        match tokio::time::timeout(BUS_TIMEOUT, super::super::client::snapshot(&conn)).await {
            Ok(Ok((version, has_activity, activities))) => {
                facts.version = Some(version);
                facts.has_activity = has_activity;
                facts.activities = activities.len();
            }
            Ok(Err(e)) => facts.error = Some(e.to_string()),
            Err(_) => facts.error = Some("timed out reading the daemon's state".to_string()),
        }
    }

    let gamemode = probe_gamemode(&dbus).await;
    (facts, gamemode)
}

async fn probe_gamemode(dbus: &zbus::fdo::DBusProxy<'_>) -> GameModeState {
    let Ok(name) = zbus::names::BusName::try_from(GAMEMODE_NAME) else {
        return GameModeState::Absent;
    };
    if let Ok(Ok(true)) = tokio::time::timeout(BUS_TIMEOUT, dbus.name_has_owner(name.clone())).await
    {
        if let Ok(Ok(pid)) =
            tokio::time::timeout(BUS_TIMEOUT, dbus.get_connection_unix_process_id(name)).await
        {
            return GameModeState::Running(pid);
        }
        return GameModeState::Running(0);
    }
    // Not running is fine: gamemoded is activatable, and the daemon's
    // NameOwnerChanged watch picks it up whenever it appears.
    match tokio::time::timeout(BUS_TIMEOUT, dbus.list_activatable_names()).await {
        Ok(Ok(names)) if names.iter().any(|n| n.as_str() == GAMEMODE_NAME) => {
            GameModeState::Activatable
        }
        _ => GameModeState::Absent,
    }
}

fn probe_detectable(dirs: &Dirs) -> Option<(DetectableTier, FileFact)> {
    let out_dir = PathBuf::from(env!("OUT_DIR"));
    detectable_candidates(dirs, Some(&out_dir))
        .into_iter()
        .map(|(tier, path)| (tier, stat(&path)))
        .find(|(_, fact)| fact.exists)
}

fn probe_discord(dirs: &Dirs) -> DiscordFacts {
    let ipc0 = probe_socket_owner(&dirs.runtime_dir.join("discord-ipc-0"));
    let upstream = (1..=9).find_map(|n| {
        match probe_socket_owner(&dirs.runtime_dir.join(format!("discord-ipc-{n}"))) {
            SocketOwner::Listening { pid, comm } => Some((pid, comm)),
            _ => None,
        }
    });

    // Whether ipc-0 is *ours* is decided by the daemon's own pid, which the
    // caller knows; `mode` is refined in `rows()` where the bus facts are at
    // hand. Here we only record what is observable at the socket.
    let mode = match (&ipc0, &upstream) {
        (SocketOwner::Listening { .. }, Some((pid, comm))) => DiscordMode::Proxy {
            pid: *pid,
            comm: comm.clone(),
        },
        (SocketOwner::Listening { .. }, None) => DiscordMode::Standalone,
        _ => DiscordMode::Idle,
    };

    DiscordFacts {
        ipc0,
        upstream,
        mode,
    }
}

/// Resolve the Discord mode against the daemon's own pid.
fn discord_mode(facts: &DiscordFacts, daemon_pid: Option<u32>) -> DiscordMode {
    match (&facts.ipc0, daemon_pid) {
        // Somebody is listening and it demonstrably is not us.
        (SocketOwner::Listening { pid, comm }, Some(daemon)) if *pid != daemon => {
            DiscordMode::Blocked {
                pid: *pid,
                comm: comm.clone(),
            }
        }
        // Somebody is listening and the daemon is not running at all, so it
        // cannot be us either. Reporting this as a healthy standalone listener
        // is precisely the false clean bill of health this tool exists to
        // avoid: the socket is taken, and the daemon will find it taken.
        (SocketOwner::Listening { pid, comm }, None) => DiscordMode::Blocked {
            pid: *pid,
            comm: comm.clone(),
        },
        _ => facts.mode.clone(),
    }
}

fn age(mtime: Option<SystemTime>) -> Option<Duration> {
    mtime.and_then(|t| t.elapsed().ok())
}

fn human_age(d: Duration) -> String {
    let days = d.as_secs() / 86_400;
    match days {
        0 => "today".to_string(),
        1 => "1 day old".to_string(),
        d if d < 90 => format!("{d} days old"),
        d => format!("{} months old", d / 30),
    }
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} kB", bytes as f64 / 1024.0)
    }
}

/// Turn probed facts into display rows with remedies. Pure.
pub fn rows(s: &Status) -> Vec<Row> {
    let install = s.primary();
    let target = install.target;
    let mut rows = Vec::new();

    // --- Daemon ------------------------------------------------------------
    rows.push(if !s.bus.session_bus {
        Row {
            label: "Daemon",
            health: Health::Unknown,
            value: "no session bus".to_string(),
            hint: s.bus.error.clone(),
            remedy: None,
        }
    } else if s.bus.owned {
        let exe = s
            .bus
            .owner_exe
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown binary".to_string());
        Row {
            label: "Daemon",
            health: Health::Ok,
            value: format!(
                "running, pid {}, v{}",
                s.bus.owner_pid.unwrap_or(0),
                s.bus.version.unwrap_or(0)
            ),
            hint: Some(exe),
            remedy: None,
        }
    } else {
        Row {
            label: "Daemon",
            health: Health::Bad,
            value: "not running".to_string(),
            hint: (!install.installed()).then(|| "nothing is installed yet".to_string()),
            remedy: Some(if install.installed() {
                Action::Start
            } else {
                Action::Install(target)
            }),
        }
    });

    // --- Autostart ---------------------------------------------------------
    rows.push(match &s.unit.enabled {
        EnabledState::Enabled => Row {
            label: "Autostart",
            health: Health::Ok,
            value: "enabled".to_string(),
            hint: None,
            remedy: None,
        },
        EnabledState::Disabled => Row {
            label: "Autostart",
            health: Health::Warn,
            value: "disabled".to_string(),
            hint: Some("the daemon will not come back after a logout".to_string()),
            remedy: Some(Action::EnableAutostart(target)),
        },
        // A unit file on disk that systemd does not know about is a different
        // problem from no unit file at all, and has a different fix.
        EnabledState::NotFound if install.unit.exists => Row {
            label: "Autostart",
            health: Health::Warn,
            value: "unit file not loaded".to_string(),
            hint: Some(format!(
                "{} exists but systemd has not picked it up — reinstall runs daemon-reload",
                install.unit.path.display()
            )),
            remedy: Some(Action::Install(target)),
        },
        EnabledState::NotFound => Row {
            label: "Autostart",
            health: Health::Bad,
            value: "no unit installed".to_string(),
            hint: Some("systemd has never heard of gamebus-presenced.service".to_string()),
            remedy: Some(Action::Install(target)),
        },
        EnabledState::Masked => Row {
            label: "Autostart",
            health: Health::Bad,
            value: "masked".to_string(),
            hint: Some("unmask it by hand: systemctl --user unmask".to_string()),
            remedy: None,
        },
        EnabledState::Static => Row {
            label: "Autostart",
            health: Health::Warn,
            value: "static (activation only)".to_string(),
            hint: Some("started on demand via D-Bus, not at login".to_string()),
            remedy: None,
        },
        EnabledState::Other(o) => Row {
            label: "Autostart",
            health: Health::Warn,
            value: o.clone(),
            hint: None,
            remedy: None,
        },
        EnabledState::Unavailable(e) => Row {
            label: "Autostart",
            health: Health::Unknown,
            value: "cannot ask systemd".to_string(),
            hint: Some(e.clone()),
            remedy: None,
        },
    });

    // --- Service state -----------------------------------------------------
    rows.push(match &s.unit.active {
        ActiveState::Active => Row {
            label: "Service",
            health: Health::Ok,
            value: "active".to_string(),
            hint: s
                .unit
                .fragment_path
                .as_ref()
                .map(|p| p.display().to_string()),
            remedy: None,
        },
        ActiveState::Failed => Row {
            label: "Service",
            health: Health::Bad,
            value: "failed".to_string(),
            hint: Some("journalctl --user -u gamebus-presenced.service".to_string()),
            remedy: Some(Action::Restart),
        },
        ActiveState::Activating => Row {
            label: "Service",
            health: Health::Unknown,
            value: "starting".to_string(),
            hint: None,
            remedy: None,
        },
        ActiveState::Inactive => Row {
            label: "Service",
            health: if s.bus.owned {
                Health::Ok
            } else {
                Health::Warn
            },
            value: if s.bus.owned {
                "not under systemd".to_string()
            } else {
                "inactive".to_string()
            },
            hint: s
                .bus
                .owned
                .then(|| "running, but started by hand rather than by the unit".to_string()),
            remedy: (!s.bus.owned).then_some(Action::Start),
        },
        ActiveState::Other(o) => Row {
            label: "Service",
            health: Health::Warn,
            value: o.clone(),
            hint: None,
            remedy: None,
        },
        ActiveState::Unavailable(e) => Row {
            label: "Service",
            health: Health::Unknown,
            value: "cannot ask systemd".to_string(),
            hint: Some(e.clone()),
            remedy: None,
        },
    });

    // --- Binaries ----------------------------------------------------------
    rows.push(binaries_row(s, install));

    // --- Activation file ---------------------------------------------------
    rows.push(activation_row(install));

    // --- Naming database ---------------------------------------------------
    rows.push(match &s.detectable {
        Some((DetectableTier::BuildDir, fact)) => Row {
            label: "Naming DB",
            health: Health::Warn,
            value: format!("build tree only ({})", human_size(fact.len)),
            hint: Some(
                "only found because this is running from the build directory; \
                 an installed daemon will have no game names"
                    .to_string(),
            ),
            remedy: Some(Action::Install(target)),
        },
        Some((tier, fact)) => {
            let stale = age(fact.mtime)
                .filter(|d| d.as_secs() > 90 * 86_400)
                .map(human_age);
            Row {
                label: "Naming DB",
                health: if stale.is_some() {
                    Health::Warn
                } else {
                    Health::Ok
                },
                value: format!("{} ({})", tier.as_str(), human_size(fact.len)),
                hint: stale.map(|a| format!("{a} — Discord adds titles continuously")),
                remedy: Some(Action::FetchDetectable),
            }
        }
        None => Row {
            label: "Naming DB",
            health: Health::Warn,
            value: "missing".to_string(),
            hint: Some("games will be named after their executable".to_string()),
            remedy: Some(Action::FetchDetectable),
        },
    });

    // --- GameMode ----------------------------------------------------------
    rows.push(match &s.gamemode {
        GameModeState::Running(pid) => Row {
            label: "GameMode",
            health: Health::Ok,
            value: format!("running, pid {pid}"),
            hint: None,
            remedy: None,
        },
        GameModeState::Activatable => Row {
            label: "GameMode",
            health: Health::Ok,
            value: "available on demand".to_string(),
            hint: Some("gamemoded starts when a game registers".to_string()),
            remedy: None,
        },
        GameModeState::Absent => Row {
            label: "GameMode",
            health: Health::Warn,
            value: "not installed".to_string(),
            hint: Some("without it, only Discord and Steam contribute".to_string()),
            remedy: None,
        },
    });

    // --- Discord -----------------------------------------------------------
    rows.push(match discord_mode(&s.discord, s.bus.owner_pid) {
        DiscordMode::Blocked { pid, comm } => Row {
            label: "Discord",
            health: Health::Warn,
            value: format!("socket held by {comm} (pid {pid})"),
            hint: Some(format!(
                "gamebus-presenced must bind discord-ipc-0 first: quit that program, \
                 restart the daemon, then start it again{}",
                match &s.discord.upstream {
                    Some((_, comm)) => format!(" ({comm} is already upstream on a later slot)"),
                    None => String::new(),
                }
            )),
            remedy: Some(Action::Restart),
        },
        DiscordMode::Proxy { comm, pid } => Row {
            label: "Discord",
            health: Health::Ok,
            value: format!("proxying to {comm} (pid {pid})"),
            hint: None,
            remedy: None,
        },
        DiscordMode::Standalone => Row {
            label: "Discord",
            health: Health::Ok,
            value: "standalone listener".to_string(),
            hint: None,
            remedy: None,
        },
        DiscordMode::Idle => Row {
            label: "Discord",
            health: if s.bus.owned {
                Health::Warn
            } else {
                Health::Unknown
            },
            value: "socket not bound".to_string(),
            hint: s
                .bus
                .owned
                .then(|| "the daemon is up but nothing is listening on discord-ipc-0".to_string()),
            remedy: s.bus.owned.then_some(Action::Restart),
        },
    });

    rows
}

fn binaries_row(s: &Status, install: &InstallFacts) -> Row {
    // Checked before "not installed": a daemon on $PATH from somewhere this
    // tool does not manage — ~/.cargo/bin after `cargo install --path .`, a
    // Nix profile, a hand `install -D` — is installed, just not here. Saying
    // "nothing at ~/.local/bin" would be true and useless.
    if let Some(resolved) = &s.path_resolution {
        if resolved != &install.layout.daemon_bin() {
            return Row {
                label: "Binaries",
                health: Health::Warn,
                value: format!("$PATH resolves to {}", resolved.display()),
                hint: Some(format!(
                    "this target installs to {}",
                    install.layout.daemon_bin().display()
                )),
                remedy: None,
            };
        }
    }

    if !install.installed() {
        return Row {
            label: "Binaries",
            health: Health::Bad,
            value: "not installed".to_string(),
            hint: Some(format!("nothing at {}", install.layout.bin_dir.display())),
            remedy: Some(Action::Install(install.target)),
        };
    }

    let missing: Vec<&str> = [
        (DAEMON_BIN, &install.daemon),
        (CLI_BIN, &install.cli),
        (SETUP_BIN, &install.setup),
    ]
    .into_iter()
    .filter(|(_, f)| !f.exists)
    .map(|(n, _)| n)
    .collect();

    // Installed copy older than the build tree: someone rebuilt and forgot.
    if let (Some(built), Some(installed)) = (
        s.built_daemon.as_ref().and_then(|f| f.mtime),
        install.daemon.mtime,
    ) {
        if built > installed {
            return Row {
                label: "Binaries",
                health: Health::Warn,
                value: "older than the build tree".to_string(),
                hint: Some(
                    "the build directory has a newer daemon than the installed one".to_string(),
                ),
                remedy: Some(Action::Install(install.target)),
            };
        }
    }

    if missing.is_empty() {
        Row {
            label: "Binaries",
            health: Health::Ok,
            value: install.layout.bin_dir.display().to_string(),
            hint: None,
            remedy: None,
        }
    } else {
        Row {
            label: "Binaries",
            health: Health::Warn,
            value: format!("missing {}", missing.join(", ")),
            hint: Some(install.layout.bin_dir.display().to_string()),
            remedy: Some(Action::Install(install.target)),
        }
    }
}

fn activation_row(install: &InstallFacts) -> Row {
    if !install.dbus_service.exists {
        return Row {
            label: "Activation",
            health: Health::Warn,
            value: "no D-Bus activation file".to_string(),
            hint: Some("the daemon can only be started explicitly".to_string()),
            remedy: Some(Action::Install(install.target)),
        };
    }

    // The activation file names an absolute path and cannot expand anything,
    // so a moved binary leaves it silently pointing at nothing.
    match &install.dbus_exec {
        Some(exec) if !install.dbus_exec_exists => Row {
            label: "Activation",
            health: Health::Bad,
            value: "points at a missing binary".to_string(),
            hint: Some(format!("Exec={exec}")),
            remedy: Some(Action::Install(install.target)),
        },
        Some(exec) if install.unit_exec.as_deref().is_some_and(|u| u != exec) => Row {
            label: "Activation",
            health: Health::Warn,
            value: "disagrees with the unit".to_string(),
            hint: Some(format!(
                "activation Exec={exec}, unit ExecStart={}",
                install.unit_exec.clone().unwrap_or_default()
            )),
            remedy: Some(Action::Install(install.target)),
        },
        _ => Row {
            label: "Activation",
            health: Health::Ok,
            value: install.dbus_service.path.display().to_string(),
            hint: None,
            remedy: None,
        },
    }
}

/// One-line verdict for the whole system.
pub fn overall(s: &Status) -> (Health, String) {
    let rows = rows(s);
    let worst = rows
        .iter()
        .map(|r| r.health)
        .max()
        .unwrap_or(Health::Unknown);
    let message = match worst {
        Health::Ok => "everything is in order".to_string(),
        Health::Unknown => "could not determine the state".to_string(),
        _ => {
            let n = rows.iter().filter(|r| r.health >= Health::Warn).count();
            let first = rows
                .iter()
                .find(|r| r.health == worst)
                .map(|r| r.label.to_lowercase())
                .unwrap_or_default();
            if n == 1 {
                format!("1 problem: {first}")
            } else {
                format!("{n} problems, worst: {first}")
            }
        }
    };
    (worst, message)
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

    fn facts(target: Target, installed: bool) -> InstallFacts {
        let layout = layout(&dirs(), target);
        let present = |p: PathBuf| FileFact {
            exists: installed,
            len: if installed { 1024 } else { 0 },
            executable: installed,
            path: p,
            mtime: None,
        };
        InstallFacts {
            daemon: present(layout.daemon_bin()),
            cli: present(layout.cli_bin()),
            setup: present(layout.setup_bin()),
            unit: present(layout.unit_file()),
            unit_exec: installed.then(|| layout.daemon_bin().display().to_string()),
            dbus_service: present(layout.dbus_service_file()),
            dbus_exec: installed.then(|| layout.daemon_bin().display().to_string()),
            dbus_exec_exists: installed,
            target,
            layout,
        }
    }

    fn status(installed: bool) -> Status {
        Status {
            euid: 1000,
            user: facts(Target::User, installed),
            system: facts(Target::System, false),
            path_resolution: None,
            built_daemon: None,
            unit: UnitFacts {
                enabled: if installed {
                    EnabledState::Enabled
                } else {
                    EnabledState::NotFound
                },
                active: if installed {
                    ActiveState::Active
                } else {
                    ActiveState::Inactive
                },
                fragment_path: None,
            },
            bus: BusFacts {
                session_bus: true,
                owned: installed,
                owner_pid: installed.then_some(4242),
                version: installed.then_some(1),
                ..Default::default()
            },
            gamemode: GameModeState::Running(1),
            detectable: Some((
                DetectableTier::UserData,
                FileFact {
                    exists: true,
                    len: 12_000_000,
                    mtime: Some(SystemTime::now()),
                    ..Default::default()
                },
            )),
            discord: DiscordFacts {
                ipc0: SocketOwner::Listening {
                    pid: 4242,
                    // /proc/<pid>/comm is truncated to 15 characters, so the
                    // daemon reads back as "gamebus-presenc".
                    comm: "gamebus-presenc".to_string(),
                },
                upstream: None,
                mode: DiscordMode::Standalone,
            },
        }
    }

    fn row<'a>(rows: &'a [Row], label: &str) -> &'a Row {
        rows.iter().find(|r| r.label == label).expect("row missing")
    }

    #[test]
    fn systemctl_output_is_read_from_stdout_not_the_exit_code() {
        // This machine returns 4 where the documentation suggests 2. Both must
        // read as "not-found", and neither as "disabled".
        assert_eq!(
            parse_is_enabled("not-found\n", Some(4)),
            EnabledState::NotFound
        );
        assert_eq!(
            parse_is_enabled("not-found\n", Some(2)),
            EnabledState::NotFound
        );
        assert_eq!(
            parse_is_enabled("enabled\n", Some(0)),
            EnabledState::Enabled
        );
        assert_eq!(
            parse_is_enabled("disabled\n", Some(1)),
            EnabledState::Disabled
        );
        assert_eq!(parse_is_enabled("masked\n", Some(1)), EnabledState::Masked);
        // No output at all is "we could not tell", never "disabled".
        assert!(matches!(
            parse_is_enabled("", None),
            EnabledState::Unavailable(_)
        ));
        assert_eq!(parse_is_active("active\n", Some(0)), ActiveState::Active);
        assert_eq!(
            parse_is_active("inactive\n", Some(4)),
            ActiveState::Inactive
        );
        assert!(matches!(
            parse_is_active("", None),
            ActiveState::Unavailable(_)
        ));
    }

    #[test]
    fn a_healthy_system_reports_no_problems() {
        let (health, message) = overall(&status(true));
        assert_eq!(health, Health::Ok, "{message}");
    }

    #[test]
    fn nothing_installed_is_bad_and_offers_to_install() {
        let s = status(false);
        let rows = rows(&s);
        assert_eq!(overall(&s).0, Health::Bad);
        assert_eq!(row(&rows, "Daemon").health, Health::Bad);
        assert_eq!(
            row(&rows, "Daemon").remedy,
            Some(Action::Install(Target::User))
        );
        assert_eq!(
            row(&rows, "Autostart").remedy,
            Some(Action::Install(Target::User))
        );
    }

    #[test]
    fn installed_but_disabled_offers_to_enable_autostart() {
        let mut s = status(true);
        s.unit.enabled = EnabledState::Disabled;
        let rows = rows(&s);
        let autostart = row(&rows, "Autostart");
        assert_eq!(autostart.health, Health::Warn);
        assert_eq!(
            autostart.remedy,
            Some(Action::EnableAutostart(Target::User))
        );
        // The daemon itself is still fine — only autostart is the problem.
        assert_eq!(row(&rows, "Daemon").health, Health::Ok);
    }

    #[test]
    fn a_held_socket_with_no_daemon_is_not_reported_as_healthy() {
        let mut s = status(true);
        // Daemon down, but somebody is on discord-ipc-0: the daemon would find
        // it taken, so this must not read as a working standalone listener.
        s.bus.owned = false;
        s.bus.owner_pid = None;
        s.discord.ipc0 = SocketOwner::Listening {
            pid: 99,
            comm: "Discord".to_string(),
        };
        let rows = rows(&s);
        assert_eq!(row(&rows, "Discord").health, Health::Warn);
    }

    #[test]
    fn a_foreign_holder_of_the_discord_socket_is_surfaced() {
        let mut s = status(true);
        s.discord.ipc0 = SocketOwner::Listening {
            pid: 99,
            comm: "Discord".to_string(),
        };
        let rows = rows(&s);
        let discord = row(&rows, "Discord");
        assert_eq!(discord.health, Health::Warn);
        assert!(discord.value.contains("Discord"), "{}", discord.value);
        assert_eq!(discord.remedy, Some(Action::Restart));
    }

    #[test]
    fn the_build_tree_naming_database_is_a_warning_not_a_pass() {
        let mut s = status(true);
        s.detectable = Some((
            DetectableTier::BuildDir,
            FileFact {
                exists: true,
                len: 12_000_000,
                ..Default::default()
            },
        ));
        let rows = rows(&s);
        let db = row(&rows, "Naming DB");
        assert_eq!(db.health, Health::Warn);
        assert_eq!(db.remedy, Some(Action::Install(Target::User)));
    }

    #[test]
    fn an_activation_file_pointing_at_nothing_is_bad() {
        let mut s = status(true);
        s.user.dbus_exec = Some("/nonexistent/gamebus-presenced".to_string());
        s.user.dbus_exec_exists = false;
        let rows = rows(&s);
        let activation = row(&rows, "Activation");
        assert_eq!(activation.health, Health::Bad);
        assert_eq!(activation.remedy, Some(Action::Install(Target::User)));
    }

    /// A daemon on $PATH that this tool did not install is reported as such,
    /// not as "nothing is installed".
    #[test]
    fn a_daemon_elsewhere_on_path_is_named_rather_than_called_missing() {
        let mut s = status(false);
        s.path_resolution = Some(PathBuf::from("/home/tester/.cargo/bin/gamebus-presenced"));
        let rows = rows(&s);
        let binaries = row(&rows, "Binaries");
        assert_eq!(binaries.health, Health::Warn);
        assert!(binaries.value.contains(".cargo/bin"), "{}", binaries.value);
    }

    #[test]
    fn a_stale_installed_binary_is_flagged_against_the_build_tree() {
        let mut s = status(true);
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let new = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000);
        s.user.daemon.mtime = Some(old);
        s.built_daemon = Some(FileFact {
            exists: true,
            mtime: Some(new),
            ..Default::default()
        });
        let rows = rows(&s);
        assert_eq!(row(&rows, "Binaries").health, Health::Warn);
        assert_eq!(
            row(&rows, "Binaries").remedy,
            Some(Action::Install(Target::User))
        );
    }

    #[test]
    fn a_system_install_on_path_is_the_primary_one() {
        let mut s = status(true);
        s.system = facts(Target::System, true);
        s.path_resolution = Some(PathBuf::from("/usr/local/bin/gamebus-presenced"));
        assert_eq!(s.primary().target, Target::System);
        // And the remedies offered then target the system install.
        s.unit.enabled = EnabledState::Disabled;
        let rows = rows(&s);
        assert_eq!(
            row(&rows, "Autostart").remedy,
            Some(Action::EnableAutostart(Target::System))
        );
    }

    #[test]
    fn no_session_bus_is_unknown_rather_than_broken() {
        let mut s = status(true);
        s.bus = BusFacts {
            session_bus: false,
            error: Some("no bus".to_string()),
            ..Default::default()
        };
        let rows = rows(&s);
        assert_eq!(row(&rows, "Daemon").health, Health::Unknown);
        assert!(row(&rows, "Daemon").remedy.is_none());
    }
}
