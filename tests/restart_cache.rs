//! Integration test for the S3 restart cache: a record written through to the
//! cache must be re-adopted by a fresh daemon after a SIGKILL, with no client
//! reconnect.
//!
//! Private session bus and runtime directory, so the SIGKILL and the socket it
//! leaves behind cannot affect anything outside the test.

use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

mod common;
use common::{ActivityPropsProxy, ManagerProxy};

const WAIT: Duration = Duration::from_secs(10);

/// Build one IPC frame: little-endian opcode + length, then the payload.
fn frame(opcode: u32, payload: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + payload.len());
    bytes.extend_from_slice(&opcode.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(payload.as_bytes());
    bytes
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_re_adopts_cached_activity() {
    let Some(env) = common::TestEnv::new("restart") else {
        eprintln!("SKIP: could not start a private session bus (is dbus-daemon installed?)");
        return;
    };
    let conn = match env.connect().await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("SKIP: private bus unusable: {e}");
            return;
        }
    };

    let cache_file = env.cache_file();

    // --- Daemon 1: serve the client, let the record hit the cache. ---
    let mut daemon1 = env.spawn_daemon("gamebus-presenced-test-restart-1.log");
    common::expect_own_daemon(&conn, daemon1.pid(), "daemon 1").await;

    let sock = env.socket_path();
    assert!(
        common::wait_for_socket(&sock, Duration::from_secs(5)).await,
        "daemon 1 did not bind {}",
        sock.display()
    );

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
    daemon1.kill_now();
    drop(client);

    // --- Daemon 2: the record must return with no client reconnect. ---
    let _daemon2 = env.spawn_daemon("gamebus-presenced-test-restart-2.log");
    common::expect_own_daemon(&conn, _daemon2.pid(), "daemon 2").await;
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
}
