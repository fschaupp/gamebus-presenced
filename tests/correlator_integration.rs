//! Integration test for the S3 correlator, end to end on the session bus.
//!
//! One pid asserted by BOTH sources: a genuine `discord-rich-presence`
//! client (whose `SO_PEERCRED` pid is this test process) and GameMode (via
//! `busctl ... RegisterGameByPID` for this same pid). Asserts the join:
//! the solo `discord_<pid>` object is absorbed into `pid_<pid>` with both
//! sources listed, that clearing Discord degrades the record in place
//! (name falls back to the executable stem, details empty), and that
//! unregistering from GameMode removes the record - it dies with the last
//! source.
//!
//! Skips gracefully when there is no session bus or no gamemoded, and when
//! a real Discord client is running (the proxy would forward the test's
//! invalid client_id to the upstream, which rejects it).

use discord_rich_presence::activity::{Activity as ClientActivity, ActivityType};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use zbus::zvariant::OwnedObjectPath;
use zbus::{fdo, proxy, Connection};

const GAMEBUS_NAME: &str = "org.gamebus.Presence.v1";
const WAIT: Duration = Duration::from_secs(10);
const CLIENT_ID: &str = "correlator-test";

#[proxy(
    interface = "org.gamebus.Presence.v1.Manager",
    default_service = "org.gamebus.Presence.v1",
    default_path = "/org/gamebus/Presence/v1"
)]
trait Manager {
    fn list_activities(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[proxy(
    interface = "org.gamebus.Presence.v1.Activity",
    default_service = "org.gamebus.Presence.v1",
    assume_defaults = false
)]
trait ActivityProps {
    #[zbus(property)]
    fn sources(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn details(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn process_id(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn executable(&self) -> zbus::Result<String>;
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

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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

fn socket_path() -> std::path::PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("discord-ipc-0")
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

/// busctl call against gamemoded; returns success of the invocation itself.
fn gamemoded_call(method: &str, pid: u32) -> bool {
    Command::new("busctl")
        .args([
            "--user",
            "call",
            "com.feralinteractive.GameMode",
            "/com/feralinteractive/GameMode",
            "com.feralinteractive.GameMode",
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

#[tokio::test(flavor = "multi_thread")]
async fn gamemode_and_discord_join_by_pid() {
    let conn = match Connection::session().await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("SKIP: no session bus: {e}");
            return;
        }
    };
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

    // Skip when a real Discord client is running: the proxy would forward
    // the test's invalid client_id to the upstream, which rejects it.
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    // `discord-ipc-0` is checked too - the daemon under test is not up yet, so
    // anything answering there is somebody else's; a file nobody answers on is
    // a corpse from an earlier SIGKILLed daemon and gets cleared out of the way.
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

    let daemon_log = std::env::temp_dir().join("gamebus-presenced-test-correlator.log");
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
    // The bus name is acquired before the Discord source is spawned, so it
    // arrives first; the RPC client below must not connect until the IPC
    // listener is actually accepting.
    let sock = socket_path();
    assert!(
        wait_for_socket(&sock, Duration::from_secs(5)).await,
        "daemon did not bind {}",
        sock.display()
    );
    let manager = ManagerProxy::new(&conn).await.unwrap();

    let test_pid = std::process::id();
    let solo_path = format!("/org/gamebus/Presence/v1/Activity/discord_{test_pid}");
    let merged_path = format!("/org/gamebus/Presence/v1/Activity/pid_{test_pid}");

    // Deterministic start: this machine preloads libgamemodeauto globally, so
    // the test process may already be registered with gamemoded. Clear any
    // pre-existing registration (failure just means none existed).
    gamemoded_call("UnregisterGameByPID", test_pid);
    let list = wait_for_activities(&manager, WAIT, |list| {
        !list.iter().any(|p| p.as_str() == merged_path)
    })
    .await;
    assert!(
        !list.iter().any(|p| p.as_str() == merged_path),
        "could not clear pre-existing GameMode registration for {test_pid}"
    );

    // Phase 1: Discord only -> solo discord_<pid> object.
    let mut client = tokio::task::spawn_blocking(|| {
        let mut client = DiscordIpcClient::new(CLIENT_ID);
        client.connect().expect("IPC connect/handshake failed");
        client
            .set_activity(
                ClientActivity::new()
                    .name("Join Test Game")
                    .details("Correlator")
                    .state("Phase 1")
                    .activity_type(ActivityType::Playing),
            )
            .expect("SET_ACTIVITY failed");
        client
    })
    .await
    .unwrap();

    let list = wait_for_activities(&manager, WAIT, |list| {
        list.iter().any(|p| p.as_str() == solo_path)
    })
    .await;
    assert!(
        list.iter().any(|p| p.as_str() == solo_path),
        "solo discord object {solo_path} did not appear: {list:?}"
    );

    // Phase 2: GameMode registers the same pid -> the solo object is
    // absorbed into pid_<pid> carrying both sources.
    assert!(
        gamemoded_call("RegisterGameByPID", test_pid),
        "RegisterGameByPID busctl call failed"
    );
    let list = wait_for_activities(&manager, WAIT, |list| {
        list.iter().any(|p| p.as_str() == merged_path)
            && !list.iter().any(|p| p.as_str() == solo_path)
    })
    .await;
    assert!(
        list.iter().any(|p| p.as_str() == merged_path),
        "merged object {merged_path} did not appear: {list:?}"
    );
    assert!(
        !list.iter().any(|p| p.as_str() == solo_path),
        "solo object {solo_path} was not absorbed: {list:?}"
    );

    let merged = ActivityPropsProxy::builder(&conn)
        .path(merged_path.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    let mut sources = merged.sources().await.unwrap();
    sources.sort();
    assert_eq!(sources, vec!["discord".to_string(), "gamemode".to_string()]);
    assert_eq!(merged.name().await.unwrap(), "Join Test Game");
    assert_eq!(merged.details().await.unwrap(), "Correlator");
    assert_eq!(merged.process_id().await.unwrap(), test_pid);
    assert!(
        !merged.executable().await.unwrap().is_empty(),
        "merged record must carry GameMode's executable"
    );

    // Phase 3: Discord clears -> the record degrades in place to the
    // GameMode-only fragment. It must NOT die.
    let mut client = tokio::task::spawn_blocking(move || {
        client.clear_activity().expect("clear_activity failed");
        client
    })
    .await
    .unwrap();

    let start = std::time::Instant::now();
    loop {
        let degraded = merged.sources().await.unwrap_or_default();
        if degraded == vec!["gamemode".to_string()] {
            break;
        }
        assert!(
            start.elapsed() < WAIT,
            "record did not degrade to gamemode-only (sources: {degraded:?})"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        !merged.executable().await.unwrap().is_empty(),
        "degraded record lost the executable"
    );
    assert_eq!(merged.details().await.unwrap(), "");
    // Name regresses to the executable stem, which is all GameMode knows.
    assert_ne!(merged.name().await.unwrap(), "Join Test Game");
    assert!(!merged.name().await.unwrap().is_empty());

    // Phase 4: the record dies with the last source.
    tokio::task::spawn_blocking(move || {
        client.close().expect("close failed");
    })
    .await
    .unwrap();
    assert!(gamemoded_call("UnregisterGameByPID", test_pid));
    let list = wait_for_activities(&manager, WAIT, |list| {
        !list.iter().any(|p| p.as_str() == merged_path)
    })
    .await;
    assert!(
        !list.iter().any(|p| p.as_str() == merged_path),
        "record {merged_path} survived its last source: {list:?}"
    );

    // SIGKILL skips the daemon's own unlink, so clear the socket here: a
    // leftover file would otherwise sit in the shared runtime dir for the next
    // test binary to trip over.
    let _ = daemon.0.kill();
    let _ = daemon.0.wait();
    let _ = std::fs::remove_file(&sock);
}
