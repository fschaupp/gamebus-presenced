//! Shared fixtures for the integration tests.
//!
//! Every test runs the daemon against a **private session bus and a private
//! runtime directory**, so nothing touches the live system: not the real
//! `org.gamebus.Presence.v1` name, not `$XDG_RUNTIME_DIR/discord-ipc-0`, not
//! the `gamemoded` that is managing actual games. Before this, a
//! gamebus-presenced installed and running on the developer's own session made
//! every integration test fail - the daemon under test lost the bus name to it.
//!
//! A private bus also gets its *own* `gamemoded`: `dbus-daemon --session` reads
//! the standard service directories, so `com.feralinteractive.GameMode` is
//! activated on demand, starting with an empty game list. That is strictly
//! better for tests than the shared one, whose registrations come and go with
//! whatever the developer is playing.
//!
//! Not a `tests/common.rs`: a file directly under `tests/` becomes its own test
//! binary. A directory module does not.

// Every item here is used by at least one of the six test binaries, but none
// of them uses all of it - so each binary alone reports the rest as dead. This
// was audited item by item (compile each `--test` target separately and
// intersect the warnings); redo that before assuming anything here is unused.
// The one item never read anywhere is `TestEnv::bus`, which exists for its
// `Drop` and is marked individually below.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use zbus::zvariant::OwnedObjectPath;
use zbus::{fdo, proxy, Connection};

pub const GAMEBUS_NAME: &str = "org.gamebus.Presence.v1";
pub const GAMEMODE_NAME: &str = "com.feralinteractive.GameMode";
pub const GAMEMODE_PATH: &str = "/com/feralinteractive/GameMode";

/// Client proxy for the Manager interface.
#[proxy(
    interface = "org.gamebus.Presence.v1.Manager",
    default_service = "org.gamebus.Presence.v1",
    default_path = "/org/gamebus/Presence/v1"
)]
pub trait Manager {
    fn list_activities(&self) -> zbus::Result<Vec<OwnedObjectPath>>;

    #[zbus(property)]
    fn has_activity(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn version(&self) -> zbus::Result<u64>;

    #[zbus(signal)]
    fn activity_added(&self, object_path: OwnedObjectPath) -> zbus::Result<()>;

    #[zbus(signal)]
    fn activity_removed(&self, object_path: OwnedObjectPath) -> zbus::Result<()>;
}

/// Client proxy for Activity objects (path set per-activity via the builder).
#[proxy(
    interface = "org.gamebus.Presence.v1.Activity",
    default_service = "org.gamebus.Presence.v1",
    assume_defaults = false
)]
pub trait ActivityProps {
    #[zbus(property)]
    fn sources(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn kind(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn details(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn process_id(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn executable(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn app_ids(&self) -> zbus::Result<HashMap<String, String>>;
    #[zbus(property)]
    fn since(&self) -> zbus::Result<u64>;
    #[zbus(property)]
    fn large_image(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn small_text(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn party_size(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn party_max(&self) -> zbus::Result<u32>;
}

/// Kill the wrapped process when it goes out of scope.
pub struct ChildGuard(pub Child);

impl ChildGuard {
    pub fn pid(&self) -> u32 {
        self.0.id()
    }

    /// SIGKILL and reap. Used where a test needs the daemon gone *now* - the
    /// restart-cache test depends on there being no graceful cleanup.
    pub fn kill_now(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A private session bus, torn down with the guard.
struct BusGuard(Child);

impl Drop for BusGuard {
    fn drop(&mut self) {
        // Killing the bus takes its activated services (gamemoded) with it:
        // they exit when the bus they were started for disappears.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// An isolated world for one test: private bus, private runtime directory.
pub struct TestEnv {
    pub bus_address: String,
    pub runtime_dir: PathBuf,
    /// Never read: held so the private bus outlives the test and is killed
    /// when the fixture drops. Removing it would tear the bus down at
    /// construction, and every test would then fail to connect.
    bus: BusGuard,
}

impl TestEnv {
    /// Start a private session bus and create a private runtime directory.
    ///
    /// Returns `None` when `dbus-daemon` is not available, so a test can skip
    /// rather than fail on a machine that cannot host one.
    pub fn new(name: &str) -> Option<Self> {
        let runtime_dir =
            std::env::temp_dir().join(format!("gamebus-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&runtime_dir);
        std::fs::create_dir_all(&runtime_dir).ok()?;

        // --nofork keeps it as our child so the guard can kill it; --session
        // picks up the standard config, which is what makes on-demand
        // activation of gamemoded work on this bus too.
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let stdout = child.stdout.take()?;
        let mut line = String::new();
        if BufReader::new(stdout).read_line(&mut line).is_err() || line.trim().is_empty() {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }

        Some(Self {
            bus_address: line.trim().to_string(),
            runtime_dir,
            bus: BusGuard(child),
        })
    }

    /// Point *this process* at the private runtime directory.
    ///
    /// Call this **only** from a test that drives an in-process
    /// `discord-rich-presence` client: that library resolves the socket from
    /// `XDG_RUNTIME_DIR` in its own process at connect time, so there is no
    /// other way to reach it. Everything else passes the values explicitly via
    /// [`TestEnv::command`] and must not call this.
    ///
    /// Mutating the environment in a test harness is racy, so the scope is
    /// deliberately narrow: the two callers (`discord_integration`,
    /// `correlator_integration`) contain exactly one `#[test]` each, and this
    /// runs before either does anything else. It is **not** called from
    /// `setup_cli`, which has eight tests running in parallel.
    pub fn export_to_process(&self) {
        std::env::set_var("XDG_RUNTIME_DIR", &self.runtime_dir);
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &self.bus_address);
    }

    /// Connect to the private bus.
    pub async fn connect(&self) -> zbus::Result<Connection> {
        zbus::ConnectionBuilder::address(self.bus_address.as_str())?
            .build()
            .await
    }

    /// A command pre-seeded with the private bus and runtime directory.
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new(program);
        cmd.env("DBUS_SESSION_BUS_ADDRESS", &self.bus_address)
            .env("XDG_RUNTIME_DIR", &self.runtime_dir);
        cmd
    }

    /// Spawn the daemon under test, logging to a file in the temp directory.
    pub fn spawn_daemon(&self, log_name: &str) -> ChildGuard {
        let log = std::env::temp_dir().join(log_name);
        let out = std::fs::File::create(&log).expect("failed to create daemon log");
        let err = out.try_clone().expect("failed to clone daemon log handle");
        let child = self
            .command(env!("CARGO_BIN_EXE_gamebus-presenced"))
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .env("RUST_LOG", "debug")
            .spawn()
            .expect("failed to spawn gamebus-presenced");
        ChildGuard(child)
    }

    pub fn socket_path(&self) -> PathBuf {
        self.runtime_dir.join("discord-ipc-0")
    }

    pub fn cache_file(&self) -> PathBuf {
        self.runtime_dir
            .join("gamebus-presenced")
            .join("cache.json")
    }

    /// Call a gamemoded method on the private bus. Returns whether the call
    /// itself succeeded.
    pub fn gamemoded_call(&self, method: &str, pid: u32) -> bool {
        self.command("busctl")
            .args([
                "--user",
                "call",
                GAMEMODE_NAME,
                GAMEMODE_PATH,
                GAMEMODE_NAME,
                method,
                "ii",
                &pid.to_string(),
                &pid.to_string(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Whether gamemoded can be reached on this bus at all.
    pub fn gamemoded_available(&self) -> bool {
        self.command("busctl")
            .args([
                "--user",
                "call",
                GAMEMODE_NAME,
                GAMEMODE_PATH,
                GAMEMODE_NAME,
                "ListGames",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.runtime_dir);
    }
}

/// The outcome of waiting for the bus name.
///
/// On a private bus a foreign owner should be impossible; keeping the case
/// means that if it ever happens the test says so instead of asserting against
/// somebody else's process.
pub enum NameWait {
    Ours,
    Foreign(u32),
    Never(String),
}

/// Poll until *our* daemon owns the bus name.
pub async fn wait_for_name(conn: &Connection, pid: u32, timeout: Duration) -> NameWait {
    let fdo = match fdo::DBusProxy::new(conn).await {
        Ok(p) => p,
        Err(e) => return NameWait::Never(format!("no bus proxy: {e}")),
    };
    let bus_name: zbus::names::BusName = match GAMEBUS_NAME.try_into() {
        Ok(n) => n,
        Err(_) => return NameWait::Never(format!("{GAMEBUS_NAME} is not a valid bus name")),
    };

    let start = std::time::Instant::now();
    let mut foreign = None;
    while start.elapsed() < timeout {
        if fdo.name_has_owner(bus_name.clone()).await.unwrap_or(false) {
            match fdo.get_connection_unix_process_id(bus_name.clone()).await {
                Ok(owner) if owner == pid => return NameWait::Ours,
                Ok(owner) => foreign = Some(owner),
                Err(_) => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    match foreign {
        Some(owner) => NameWait::Foreign(owner),
        None => NameWait::Never(format!(
            "the daemon under test (pid {pid}) never acquired {GAMEBUS_NAME}"
        )),
    }
}

/// Assert that the daemon under test took the bus name, with a useful message.
pub async fn expect_own_daemon(conn: &Connection, pid: u32, label: &str) {
    match wait_for_name(conn, pid, Duration::from_secs(5)).await {
        NameWait::Ours => {}
        NameWait::Foreign(owner) => panic!(
            "{label}: {GAMEBUS_NAME} on the *private* bus is owned by pid {owner}, \
             not the daemon under test (pid {pid})"
        ),
        NameWait::Never(why) => panic!("{label}: {why}"),
    }
}

/// Poll until the daemon's IPC socket accepts a connection (or give up).
///
/// The path existing is not a readiness signal: a SIGKILLed daemon leaves the
/// socket file behind, so the path can be there while nothing is listening.
pub async fn wait_for_socket(path: &Path, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if tokio::net::UnixStream::connect(path).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Poll until `cond` over the listed activity paths holds (or give up).
pub async fn wait_for_activities(
    manager: &ManagerProxy<'_>,
    timeout: Duration,
    mut cond: impl FnMut(&[OwnedObjectPath]) -> bool,
) -> Vec<OwnedObjectPath> {
    let start = std::time::Instant::now();
    loop {
        let list = manager.list_activities().await.unwrap_or_default();
        if cond(&list) || start.elapsed() > timeout {
            return list;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
