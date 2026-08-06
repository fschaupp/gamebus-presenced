//! Integration test for the S2 Discord IPC source, end to end.
//!
//! Spawns the real daemon binary (which binds `discord-ipc-0`), connects a
//! genuine `discord-rich-presence` RPC client to it, and asserts that
//! SET_ACTIVITY becomes a D-Bus activity object with the right properties,
//! that a second SET_ACTIVITY updates it in place, and that clearing the
//! activity removes it.
//!
//! Skips gracefully when there is no session bus, and when a real Discord
//! client is running (the proxy would forward the test's invalid client_id
//! to the upstream, which rejects it).

use discord_rich_presence::activity::{
    Activity as ClientActivity, ActivityType, Assets, Party, Timestamps,
};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use zbus::zvariant::OwnedObjectPath;
use zbus::{fdo, proxy, Connection};

const GAMEBUS_NAME: &str = "org.gamebus.Presence.v1";
const WAIT: Duration = Duration::from_secs(10);
const CLIENT_ID: &str = "test-client-42";

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
    fn details(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn process_id(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn executable(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn app_ids(&self) -> zbus::Result<std::collections::HashMap<String, String>>;
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

/// Poll until the daemon's IPC socket accepts a connection (or give up).
///
/// The path existing is not a readiness signal: a SIGKILLed daemon from an
/// earlier test leaves the socket file behind, so the path can be there while
/// nothing is listening yet. Only a successful connect proves the listener is
/// up, and by then the stale file has been unlinked and rebound.
async fn wait_for_socket(path: &std::path::Path, timeout: Duration) -> bool {
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
async fn wait_for_activities(
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

fn socket_path() -> std::path::PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("discord-ipc-0")
}

#[tokio::test(flavor = "multi_thread")]
async fn discord_set_activity_appears_on_bus() {
    // Skip without a session bus.
    let conn = match Connection::session().await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("SKIP: no session bus: {e}");
            return;
        }
    };

    // Skip when a real Discord client is running: the proxy would forward
    // the test's invalid client_id to the upstream, which rejects it.
    // `discord-ipc-0` is checked too - the daemon under test is not up yet, so
    // anything answering there is somebody else's; a file nobody answers on is
    // a corpse from an earlier SIGKILLed daemon and gets cleared out of the way.
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    for n in 0..=9 {
        let candidate = runtime_dir.join(format!("discord-ipc-{n}"));
        if !candidate.exists() {
            continue;
        }
        if std::os::unix::net::UnixStream::connect(&candidate).is_ok() {
            eprintln!(
                "SKIP: real Discord client running on {}; proxy would reject test client_id",
                candidate.display()
            );
            return;
        }
        if n == 0 {
            let _ = std::fs::remove_file(&candidate);
        }
    }

    // Start the daemon under test. Tracing writes to stdout; capture both.
    let daemon_log = std::env::temp_dir().join("gamebus-presenced-test-discord.log");
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

    // Wait until the IPC listener actually accepts connections. The bus name
    // is acquired before the Discord source is spawned, so it arrives first.
    let sock = socket_path();
    assert!(
        wait_for_socket(&sock, Duration::from_secs(5)).await,
        "daemon did not bind {}",
        sock.display()
    );

    let manager = ManagerProxy::new(&conn).await.unwrap();

    // The client runs blocking in-process; SO_PEERCRED therefore reports the
    // test binary's own pid.
    let test_pid = std::process::id();
    let expected_path = format!("/org/gamebus/Presence/v1/Activity/discord_{test_pid}");

    // Connect and SET_ACTIVITY with known fields.
    let mut client = tokio::task::spawn_blocking(|| {
        let mut client = DiscordIpcClient::new(CLIENT_ID);
        client.connect().expect("IPC connect/handshake failed");
        client
            .set_activity(
                ClientActivity::new()
                    .name("Test Game")
                    .details("Limgrave")
                    .state("Exploring")
                    .timestamps(Timestamps::new().start(1_700_000_000_000))
                    .assets(Assets::new().large_image("large").small_text("Online"))
                    .party(Party::new().size([2, 4]))
                    .activity_type(ActivityType::Playing),
            )
            .expect("SET_ACTIVITY failed");
        client
    })
    .await
    .unwrap();

    // The activity object must appear.
    let list = wait_for_activities(&manager, WAIT, |list| {
        list.iter().any(|p| p.as_str() == expected_path)
    })
    .await;
    assert!(
        list.iter().any(|p| p.as_str() == expected_path),
        "ListActivities {list:?} missing {expected_path}"
    );
    assert!(manager.has_activity().await.unwrap());

    // Read its properties.
    let activity = ActivityPropsProxy::builder(&conn)
        .path(expected_path.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    assert_eq!(
        activity.sources().await.unwrap(),
        vec!["discord".to_string()]
    );
    assert_eq!(activity.kind().await.unwrap(), "game");
    assert_eq!(activity.name().await.unwrap(), "Test Game");
    assert_eq!(activity.details().await.unwrap(), "Limgrave");
    assert_eq!(activity.state().await.unwrap(), "Exploring");
    assert_eq!(activity.process_id().await.unwrap(), test_pid);
    assert_eq!(activity.executable().await.unwrap(), "");
    assert_eq!(
        activity.app_ids().await.unwrap().get("discord").unwrap(),
        CLIENT_ID
    );
    assert_eq!(activity.since().await.unwrap(), 1_700_000_000);
    assert_eq!(activity.large_image().await.unwrap(), "large");
    assert_eq!(activity.small_text().await.unwrap(), "Online");
    assert_eq!(activity.party_size().await.unwrap(), 2);
    assert_eq!(activity.party_max().await.unwrap(), 4);

    // Mid-session update: a second SET_ACTIVITY must update the object in
    // place (same path, new data).
    let mut client = tokio::task::spawn_blocking(move || {
        client
            .set_activity(
                ClientActivity::new()
                    .name("Test Game")
                    .details("Limgrave")
                    .state("Boss: Margit")
                    .activity_type(ActivityType::Playing),
            )
            .expect("second SET_ACTIVITY failed");
        client
    })
    .await
    .unwrap();

    let start = std::time::Instant::now();
    loop {
        if activity.state().await.unwrap_or_default() == "Boss: Margit" {
            break;
        }
        assert!(
            start.elapsed() < WAIT,
            "activity object did not reflect the update in time"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Unchanged fields keep their values; missing ones stay empty.
    assert_eq!(activity.name().await.unwrap(), "Test Game");
    assert_eq!(activity.details().await.unwrap(), "Limgrave");

    // Clear: the record dies with the last source.
    tokio::task::spawn_blocking(move || {
        client.clear_activity().expect("clear_activity failed");
        client.close().expect("close failed");
    })
    .await
    .unwrap();

    let list = wait_for_activities(&manager, WAIT, |list| {
        !list.iter().any(|p| p.as_str() == expected_path)
    })
    .await;
    assert!(
        !list.iter().any(|p| p.as_str() == expected_path),
        "activity {expected_path} still listed after clear"
    );
    // Note: HasActivity is intentionally not asserted false here - this
    // machine preloads libgamemodeauto globally, so unrelated processes may
    // legitimately hold GameMode-sourced activities on the same daemon.

    // Cleanup: the daemon was started with a stale-socket check, so remove
    // the test socket for hygiene (SIGKILL skips the daemon's own unlink).
    daemon.0.kill().unwrap();
    let _ = daemon.0.wait();
    let _ = std::fs::remove_file(&sock);
}
