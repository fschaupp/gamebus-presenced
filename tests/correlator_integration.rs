//! Integration test for the S3 correlator: a GameMode registration and a
//! Discord RPC client with the same pid must become ONE activity record.
//!
//! Runs on a private session bus with its own gamemoded and a private runtime
//! directory, so nothing here depends on — or disturbs — the live session.

use discord_rich_presence::activity::{Activity as ClientActivity, ActivityType};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use std::time::Duration;

mod common;
use common::{ActivityPropsProxy, ManagerProxy};

const WAIT: Duration = Duration::from_secs(10);
const CLIENT_ID: &str = "correlator-test";

#[tokio::test(flavor = "multi_thread")]
async fn gamemode_and_discord_join_by_pid() {
    let Some(env) = common::TestEnv::new("correlator") else {
        eprintln!("SKIP: could not start a private session bus (is dbus-daemon installed?)");
        return;
    };
    // This test drives a real discord-rich-presence client, which finds the
    // socket through this process's own XDG_RUNTIME_DIR.
    env.export_to_process();

    let conn = match env.connect().await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("SKIP: private bus unusable: {e}");
            return;
        }
    };
    if !env.gamemoded_available() {
        eprintln!("SKIP: gamemoded could not be activated on the private bus");
        return;
    }

    let mut daemon = env.spawn_daemon("gamebus-presenced-test-correlator.log");
    common::expect_own_daemon(&conn, daemon.pid(), "correlator").await;

    // The RPC client below must not connect until the listener is accepting.
    let sock = env.socket_path();
    assert!(
        common::wait_for_socket(&sock, Duration::from_secs(5)).await,
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
    env.gamemoded_call("UnregisterGameByPID", test_pid);
    let list = common::wait_for_activities(&manager, WAIT, |list| {
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

    let list = common::wait_for_activities(&manager, WAIT, |list| {
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
        env.gamemoded_call("RegisterGameByPID", test_pid),
        "RegisterGameByPID busctl call failed"
    );
    let list = common::wait_for_activities(&manager, WAIT, |list| {
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
    assert!(env.gamemoded_call("UnregisterGameByPID", test_pid));
    let list = common::wait_for_activities(&manager, WAIT, |list| {
        !list.iter().any(|p| p.as_str() == merged_path)
    })
    .await;
    assert!(
        !list.iter().any(|p| p.as_str() == merged_path),
        "record {merged_path} survived its last source: {list:?}"
    );

    // Nothing to clean up by hand: the private runtime directory and the
    // private bus both go with the fixture.
    daemon.kill_now();
}
