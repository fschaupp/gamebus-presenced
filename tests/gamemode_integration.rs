//! Integration test for the S1 GameMode source, end to end on the session bus.
//!
//! Spawns the real daemon binary, registers a real game via
//! `gamemoderun sleep 30`, and asserts the activity appears on
//! `org.gamebus.Presence.v1` with the right properties and signals, then
//! disappears when the game exits.
//!
//! Skips gracefully when there is no session bus or no gamemoded to talk to.

use std::process::{Child, Command, Stdio};
use std::time::Duration;
use zbus::export::futures_util::StreamExt;
use zbus::zvariant::OwnedObjectPath;
use zbus::{fdo, proxy, Connection};

const GAMEBUS_NAME: &str = "org.gamebus.Presence.v1";
const ADDED_TIMEOUT: Duration = Duration::from_secs(20);

/// Client proxy for the Manager interface under test.
#[proxy(
    interface = "org.gamebus.Presence.v1.Manager",
    default_service = "org.gamebus.Presence.v1",
    default_path = "/org/gamebus/Presence/v1"
)]
trait Manager {
    fn list_activities(&self) -> zbus::Result<Vec<OwnedObjectPath>>;

    #[zbus(property)]
    fn has_activity(&self) -> zbus::Result<bool>;

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
trait ActivityProps {
    #[zbus(property)]
    fn sources(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn kind(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn process_id(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn executable(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn since(&self) -> zbus::Result<u64>;
}

/// Minimal GameMode client used only for the availability check.
#[proxy(
    interface = "com.feralinteractive.GameMode",
    default_service = "com.feralinteractive.GameMode",
    default_path = "/com/feralinteractive/GameMode"
)]
trait GameMode {
    fn list_games(&self) -> zbus::Result<Vec<(i32, OwnedObjectPath)>>;
}

/// Kill the wrapped process when it goes out of scope.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Poll until the daemon owns its bus name (or give up).
async fn wait_for_name(conn: &Connection, name: &str, timeout: Duration) -> bool {
    let fdo = match fdo::DBusProxy::new(conn).await {
        Ok(p) => p,
        Err(_) => return false,
    };
    let name: zbus::names::BusName = match name.try_into() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if fdo.name_has_owner(name.clone()).await.unwrap_or(false) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn gamemode_registration_appears_on_bus() {
    // Skip without a session bus.
    let conn = match Connection::session().await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("SKIP: no session bus: {e}");
            return;
        }
    };

    // Skip without a usable gamemoded (this may D-Bus-activate it).
    match GameModeProxy::new(&conn).await {
        Ok(gm) => match gm.list_games().await {
            Ok(_) => {}
            Err(e) => {
                eprintln!("SKIP: gamemoded not usable: {e}");
                return;
            }
        },
        Err(e) => {
            eprintln!("SKIP: gamemoded not available: {e}");
            return;
        }
    }

    // Start the daemon under test. The daemon's tracing writes to stdout;
    // capture both streams to a temp file so failures are debuggable.
    let daemon_log = std::env::temp_dir().join("gamebus-presenced-test-daemon.log");
    let daemon_log_out = std::fs::File::create(&daemon_log).unwrap();
    let daemon_log_err = daemon_log_out.try_clone().unwrap();
    let daemon = Command::new(env!("CARGO_BIN_EXE_gamebus-presenced"))
        .stdout(Stdio::from(daemon_log_out))
        .stderr(Stdio::from(daemon_log_err))
        .env("RUST_LOG", "debug")
        .spawn()
        .expect("failed to spawn gamebus-presenced");
    let mut daemon = ChildGuard(daemon);

    assert!(
        wait_for_name(&conn, GAMEBUS_NAME, Duration::from_secs(5)).await,
        "daemon did not acquire its bus name"
    );

    let manager = ManagerProxy::new(&conn).await.unwrap();
    let mut added = manager.receive_activity_added().await.unwrap();
    let mut removed = manager.receive_activity_removed().await.unwrap();

    // Register a real "game". gamemoderun execs its command, so the child
    // pid IS the sleep process that libgamemodeauto registers.
    let game = Command::new("gamemoderun")
        .args(["sleep", "30"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn gamemoderun");
    let game_pid = game.id();
    let _game = ChildGuard(game);

    // Expect ActivityAdded for our pid.
    let expected_suffix = format!("pid_{game_pid}");
    let added_path = tokio::time::timeout(ADDED_TIMEOUT, async {
        while let Some(signal) = added.next().await {
            if let Ok(args) = signal.args() {
                if args.object_path.as_str().ends_with(&expected_suffix) {
                    return Some(args.object_path.clone());
                }
            }
        }
        None
    })
    .await
    .expect("timed out waiting for ActivityAdded")
    .expect("ActivityAdded stream ended unexpectedly");

    // ListActivities must contain the object, HasActivity must be true.
    let list = manager.list_activities().await.unwrap();
    assert!(
        list.iter().any(|p| p == &added_path),
        "ListActivities {list:?} missing {added_path}"
    );
    assert!(manager.has_activity().await.unwrap());

    // Read the activity's properties.
    let activity = ActivityPropsProxy::builder(&conn)
        .path(added_path.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    assert_eq!(activity.process_id().await.unwrap(), game_pid);
    assert_eq!(activity.kind().await.unwrap(), "game");
    assert_eq!(
        activity.sources().await.unwrap(),
        vec!["gamemode".to_string()]
    );
    assert_eq!(activity.name().await.unwrap(), "sleep");
    let executable = activity.executable().await.unwrap();
    assert!(
        executable.ends_with("sleep"),
        "unexpected executable: {executable}"
    );
    assert!(activity.since().await.unwrap() > 0);

    // Unregister the game explicitly. This is the design doc's "unhappy
    // path" tool: gamemoded emits GameUnregistered immediately, making the
    // test deterministic. (Removal via gamemoded's dead-client reaper after
    // SIGKILL takes up to ~20s and was verified manually instead.)
    let status = Command::new("busctl")
        .args([
            "--user",
            "call",
            "com.feralinteractive.GameMode",
            "/com/feralinteractive/GameMode",
            "com.feralinteractive.GameMode",
            "UnregisterGameByPID",
            "ii",
            &game_pid.to_string(),
            &game_pid.to_string(),
        ])
        .stdout(Stdio::null())
        .status()
        .expect("failed to run busctl");
    assert!(status.success(), "UnregisterGameByPID failed");

    let removed_path = match tokio::time::timeout(ADDED_TIMEOUT, async {
        while let Some(signal) = removed.next().await {
            if let Ok(args) = signal.args() {
                if args.object_path == added_path {
                    return Some(args.object_path.clone());
                }
            }
        }
        None
    })
    .await
    {
        Ok(Some(path)) => path,
        other => {
            // Daemon died? Surface its status and log before failing.
            let status = daemon.0.try_wait().map_err(|e| e.to_string());
            let log = std::fs::read_to_string(&daemon_log).unwrap_or_default();
            panic!(
                "timed out waiting for ActivityRemoved ({other:?}); daemon status: {status:?};\n--- daemon log ---\n{log}"
            );
        }
    };
    assert_eq!(removed_path, added_path);

    let list = manager.list_activities().await.unwrap();
    assert!(!list.iter().any(|p| p == &added_path));
    assert!(!manager.has_activity().await.unwrap());
}
