//! Integration test for MPRIS naming hints, end to end.
//!
//! Serves a genuine MPRIS player (real `org.mpris.MediaPlayer2.*` bus name,
//! real `Identity` property, answered over the real wire) from this test
//! process on the private session bus, then registers the same pid with the
//! bus's own gamemoded. The daemon must name the record after the player's
//! Identity - and must NOT list MPRIS as a source: hints are naming evidence,
//! not a source.
//!
//! Private session bus and runtime directory throughout (tests/common).

use std::time::Duration;

mod common;
use common::{ActivityPropsProxy, ManagerProxy};

const WAIT: Duration = Duration::from_secs(10);
const PLAYER_IDENTITY: &str = "Gamebus Test Player";

/// A minimal but genuine MPRIS root interface: the daemon reads `Identity`
/// via `org.freedesktop.DBus.Properties.Get` exactly as it would from VLC.
struct MprisRoot;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl MprisRoot {
    #[zbus(property)]
    fn identity(&self) -> String {
        PLAYER_IDENTITY.to_string()
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mpris_identity_names_an_unidentified_record() {
    let Some(env) = common::TestEnv::new("mpris") else {
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
    if !env.gamemoded_available() {
        eprintln!("SKIP: gamemoded could not be activated on the private bus");
        return;
    }

    // The player exists BEFORE the daemon starts, so the seed pass picks it
    // up and the hint is in place when the game registers - no reliance on
    // the 15s reseed tick.
    let player_conn = env.connect().await.expect("player connection failed");
    player_conn
        .object_server()
        .at("/org/mpris/MediaPlayer2", MprisRoot)
        .await
        .expect("failed to serve the MPRIS object");
    player_conn
        .request_name("org.mpris.MediaPlayer2.gamebustest")
        .await
        .expect("failed to claim the MPRIS bus name");

    let mut daemon = env.spawn_daemon("gamebus-presenced-test-mpris.log");
    common::expect_own_daemon(&conn, daemon.pid(), "mpris").await;
    let manager = ManagerProxy::new(&conn).await.unwrap();

    // Register this test process as a "game". Its executable is the test
    // binary - nothing detectable.json could ever name, so without the hint
    // the record's name would be the executable stem.
    let test_pid = std::process::id();
    assert!(
        env.gamemoded_call("RegisterGameByPID", test_pid),
        "RegisterGameByPID failed on the private gamemoded"
    );

    let expected_path = format!("/org/gamebus/Presence/v1/Activity/pid_{test_pid}");
    let list = common::wait_for_activities(&manager, WAIT, |list| {
        list.iter().any(|p| p.as_str() == expected_path)
    })
    .await;
    assert!(
        list.iter().any(|p| p.as_str() == expected_path),
        "record for pid {test_pid} never appeared: {list:?}"
    );

    let activity = ActivityPropsProxy::builder(&conn)
        .path(expected_path.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();

    // The name must become the player's Identity (either immediately from
    // the stored hint, or via the in-place refresh).
    let start = std::time::Instant::now();
    loop {
        let name = activity.name().await.unwrap_or_default();
        if name == PLAYER_IDENTITY {
            break;
        }
        assert!(
            start.elapsed() < WAIT,
            "record was never named after the MPRIS Identity (last name: {name:?})"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Hints are naming evidence, not a source: Sources must not mention MPRIS.
    let sources = activity.sources().await.unwrap_or_default();
    assert!(
        sources.iter().all(|s| !s.to_lowercase().contains("mpris")),
        "MPRIS leaked into Sources: {sources:?}"
    );

    // Player exit must not un-name the record (monotone naming).
    drop(player_conn);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        activity.name().await.unwrap_or_default(),
        PLAYER_IDENTITY,
        "name regressed after the player closed"
    );

    // Cleanup: unregister; the record dies with its evidence.
    env.gamemoded_call("UnregisterGameByPID", test_pid);
    let list = common::wait_for_activities(&manager, WAIT, |list| {
        !list.iter().any(|p| p.as_str() == expected_path)
    })
    .await;
    assert!(
        !list.iter().any(|p| p.as_str() == expected_path),
        "record survived unregistration: {list:?}"
    );
    daemon.kill_now();
}
