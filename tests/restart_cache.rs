//! Integration test for the S3c restart cache.
//!
//! A daemon instance serves a raw Discord IPC client asserting SET_ACTIVITY,
//! is SIGKILLed, and a second instance is spawned with the same (isolated)
//! XDG_RUNTIME_DIR. The second instance must re-adopt the record from the
//! cache with NO client reconnect - the client's process (this test binary)
//! is still alive with the same `/proc` start-time, so the record validates.
//!
//! The re-adopted record may appear under `discord_<pid>` or, once the
//! GameMode seed catches up on machines with libgamemodeauto preloaded, under
//! the merged `pid_<pid>` identity - the assertion is on the record's
//! existence and its preserved Discord fields, not the id.
//!
//! Skips gracefully when there is no session bus.

use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use zbus::zvariant::OwnedObjectPath;
use zbus::{fdo, proxy, Connection};

const GAMEBUS_NAME: &str = "org.gamebus.Presence.v1";
const WAIT: Duration = Duration::from_secs(10);

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
    fn name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn process_id(&self) -> zbus::Result<u32>;
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

fn frame(opcode: u32, payload: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + payload.len());
    bytes.extend_from_slice(&opcode.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(payload.as_bytes());
    bytes
}

fn spawn_daemon(dir: &std::path::Path, log_name: &str) -> ChildGuard {
    let log = std::env::temp_dir().join(log_name);
    let log_out = std::fs::File::create(&log).unwrap();
    let log_err = log_out.try_clone().unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_gamebus-presenced"))
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err))
        .env("RUST_LOG", "debug")
        .env("XDG_RUNTIME_DIR", dir)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
        )
        .spawn()
        .expect("failed to spawn gamebus-presenced");
    ChildGuard(child)
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_re_adopts_cached_activity() {
    let conn = match Connection::session().await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("SKIP: no session bus: {e}");
            return;
        }
    };

    let dir = std::env::temp_dir().join(format!("gamebus-restart-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let cache_file = dir.join("gamebus-presenced").join("cache.json");

    // --- Daemon 1: serve the client, let the record hit the cache. ---
    let mut daemon1 = spawn_daemon(&dir, "gamebus-presenced-test-restart-1.log");
    assert!(
        wait_for_name(&conn, GAMEBUS_NAME, Duration::from_secs(5)).await,
        "daemon 1 did not acquire its bus name"
    );
    let sock = dir.join("discord-ipc-0");
    let start = std::time::Instant::now();
    while !sock.exists() && start.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(sock.exists(), "daemon 1 did not bind {}", sock.display());

    let test_pid = std::process::id();
    let mut client = UnixStream::connect(&sock).await.unwrap();
    client
        .write_all(&frame(0, r#"{"v":1,"client_id":"restart-test"}"#))
        .await
        .unwrap();
    client
        .write_all(&frame(
            1,
            r#"{"cmd":"SET_ACTIVITY","args":{"pid":1,"activity":{"name":"Restart Survivor","details":"Still Here","type":0}},"nonce":"n-1"}"#,
        ))
        .await
        .unwrap();

    // Wait until the write-through cache contains the record.
    let start = std::time::Instant::now();
    loop {
        if let Ok(content) = std::fs::read_to_string(&cache_file) {
            if content.contains("Restart Survivor") {
                break;
            }
        }
        assert!(start.elapsed() < WAIT, "cache never contained the record");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // SIGKILL: no graceful cleanup - the cache is all that survives.
    daemon1.0.kill().unwrap();
    let _ = daemon1.0.wait();
    drop(client);

    // --- Daemon 2: the record must return with no client reconnect. ---
    let _daemon2 = spawn_daemon(&dir, "gamebus-presenced-test-restart-2.log");
    assert!(
        wait_for_name(&conn, GAMEBUS_NAME, Duration::from_secs(5)).await,
        "daemon 2 did not acquire its bus name"
    );
    let manager = ManagerProxy::new(&conn).await.unwrap();

    let suffix = format!("_{test_pid}");
    let start = std::time::Instant::now();
    let path = loop {
        let list = manager.list_activities().await.unwrap_or_default();
        if let Some(p) = list.iter().find(|p| p.as_str().ends_with(&suffix)) {
            break p.clone();
        }
        assert!(
            start.elapsed() < WAIT,
            "no record for pid {test_pid} re-adopted after restart: {list:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    let activity = ActivityPropsProxy::builder(&conn)
        .path(path.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    assert_eq!(activity.process_id().await.unwrap(), test_pid);

    // The cached Discord fields must be preserved - either still on the
    // re-adopted solo record, or carried through the merge once GameMode's
    // seed joins the pid on preload machines.
    let start = std::time::Instant::now();
    loop {
        let name = activity.name().await.unwrap_or_default();
        if name == "Restart Survivor" {
            break;
        }
        assert!(
            start.elapsed() < WAIT,
            "re-adopted record lost its Discord fields (name: {name:?})"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let _ = std::fs::remove_dir_all(&dir);
}
