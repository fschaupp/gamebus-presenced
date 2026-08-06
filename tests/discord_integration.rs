//! Integration test for the Discord IPC source, end to end.
//!
//! Spawns the real daemon binary (which binds `discord-ipc-0` inside a private
//! runtime directory), connects a genuine `discord-rich-presence` RPC client to
//! it, and asserts that SET_ACTIVITY becomes a D-Bus activity object with the
//! right properties, that a second SET_ACTIVITY updates it in place, and that
//! clearing the activity removes it.
//!
//! Runs entirely on a private session bus and a private runtime directory, so a
//! real Discord client and a real gamebus-presenced can both be running without
//! affecting it - or being affected by it.

use discord_rich_presence::activity::{
    Activity as ClientActivity, ActivityType, Assets, Party, Timestamps,
};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use std::time::Duration;

mod common;
use common::{ActivityPropsProxy, ManagerProxy};

const WAIT: Duration = Duration::from_secs(10);
const CLIENT_ID: &str = "test-client-42";

#[tokio::test(flavor = "multi_thread")]
async fn discord_set_activity_appears_on_bus() {
    let Some(env) = common::TestEnv::new("discord") else {
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

    let mut daemon = env.spawn_daemon("gamebus-presenced-test-discord.log");
    common::expect_own_daemon(&conn, daemon.pid(), "discord").await;

    // Wait until the IPC listener actually accepts connections. The bus name is
    // acquired before the Discord source is spawned, so it arrives first.
    let sock = env.socket_path();
    assert!(
        common::wait_for_socket(&sock, Duration::from_secs(5)).await,
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
    let list = common::wait_for_activities(&manager, WAIT, |list| {
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

    let list = common::wait_for_activities(&manager, WAIT, |list| {
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

    // The private runtime directory goes with the fixture, so there is no
    // socket left to clean up by hand.
    daemon.kill_now();
}
