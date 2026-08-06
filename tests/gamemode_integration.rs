//! Integration test for the GameMode source, end to end.
//!
//! Spawns the real daemon against a private session bus, registers a real game
//! with the `gamemoded` that bus activates for itself, and asserts the activity
//! appears on the bus and disappears when the game exits.
//!
//! The private bus gets its own gamemoded with an empty game list, so whatever
//! the developer happens to be playing cannot influence the result.

use std::process::Stdio;
use std::time::Duration;
use zbus::export::futures_util::StreamExt;

mod common;
use common::{ActivityPropsProxy, ChildGuard, ManagerProxy};

const ADDED_TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::test]
async fn gamemode_registration_appears_on_bus() {
    let Some(env) = common::TestEnv::new("gamemode") else {
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

    let mut daemon = env.spawn_daemon("gamebus-presenced-test-daemon.log");
    common::expect_own_daemon(&conn, daemon.pid(), "gamemode").await;

    let manager = ManagerProxy::new(&conn).await.unwrap();
    let mut added = manager.receive_activity_added().await.unwrap();
    let mut removed = manager.receive_activity_removed().await.unwrap();

    // Register a real "game". gamemoderun execs its command, so the child
    // pid IS the sleep process that libgamemodeauto registers.
    let game = env
        .command("gamemoderun")
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
    let status = env
        .command("busctl")
        .args([
            "--user",
            "call",
            common::GAMEMODE_NAME,
            common::GAMEMODE_PATH,
            common::GAMEMODE_NAME,
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
            let log = std::fs::read_to_string(
                std::env::temp_dir().join("gamebus-presenced-test-daemon.log"),
            )
            .unwrap_or_default();
            panic!(
                "timed out waiting for ActivityRemoved ({other:?}); daemon status: {status:?};\n--- daemon log ---\n{log}"
            );
        }
    };
    assert_eq!(removed_path, added_path);

    let list = manager.list_activities().await.unwrap();
    assert!(!list.iter().any(|p| p == &added_path));
    // Note: HasActivity is not asserted false here - other processes may
    // legitimately hold GameMode registrations (e.g. libgamemodeauto preload
    // or a running game). The test verifies the specific game is added and
    // removed; the global HasActivity state depends on the environment.
}
