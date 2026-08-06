//! Integration test for S4a: a process carrying `SteamAppId` in its environment
//! and registered with GameMode must produce one record carrying the appid.
//!
//! Private session bus and runtime directory throughout.

use std::process::Stdio;
use std::time::Duration;
use zbus::export::futures_util::StreamExt;

mod common;
use common::{ActivityPropsProxy, ChildGuard, ManagerProxy};

const ADDED_TIMEOUT: Duration = Duration::from_secs(20);

#[tokio::test(flavor = "multi_thread")]
async fn steam_appid_enrichment_appears_on_bus() {
    let Some(env) = common::TestEnv::new("steam") else {
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

    let _daemon = env.spawn_daemon("gamebus-presenced-test-steam.log");
    common::expect_own_daemon(&conn, _daemon.pid(), "steam").await;

    let manager = ManagerProxy::new(&conn).await.unwrap();
    let mut added = manager.receive_activity_added().await.unwrap();
    let mut removed = manager.receive_activity_removed().await.unwrap();

    // Spawn a "game" with SteamAppId in its environment. The Enricher will
    // probe /proc/<pid>/environ and find it.
    let game = env
        .command("sleep")
        .arg("30")
        .env("SteamAppId", "480")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn sleep with SteamAppId");
    let game_pid = game.id();
    let _game = ChildGuard(game);

    // Register the pid with GameMode so the daemon sees it.
    let status = env
        .command("busctl")
        .args([
            "--user",
            "call",
            "com.feralinteractive.GameMode",
            "/com/feralinteractive/GameMode",
            "com.feralinteractive.GameMode",
            "RegisterGameByPID",
            "ii",
            &game_pid.to_string(),
            &game_pid.to_string(),
        ])
        .stdout(Stdio::null())
        .status()
        .expect("failed to run busctl");
    assert!(status.success(), "RegisterGameByPID failed");

    // Expect ActivityAdded for our pid (merged record: gamemode + steam).
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

    // Read the activity's properties.
    let activity = ActivityPropsProxy::builder(&conn)
        .path(added_path.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();

    assert_eq!(activity.process_id().await.unwrap(), game_pid);
    assert_eq!(activity.kind().await.unwrap(), "game");

    // The merged record must include both gamemode and steam.
    let sources = activity.sources().await.unwrap();
    assert!(
        sources.contains(&"gamemode".to_string()),
        "sources missing gamemode: {sources:?}"
    );
    assert!(
        sources.contains(&"steam".to_string()),
        "sources missing steam: {sources:?}"
    );

    // Steam appid must be present.
    let app_ids = activity.app_ids().await.unwrap();
    assert_eq!(
        app_ids.get("steam").map(|s| s.as_str()),
        Some("480"),
        "app_ids missing steam=480: {app_ids:?}"
    );

    // Unregister the game. This should remove the record (GameMode is the
    // only non-Steam source, so Steam partial is removed too).
    let status = env
        .command("busctl")
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

    // The record must be removed (dies with the last source).
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
            let log = std::fs::read_to_string(
                std::env::temp_dir().join("gamebus-presenced-test-steam.log"),
            )
            .unwrap_or_default();
            panic!("timed out waiting for ActivityRemoved ({other:?});\n--- daemon log ---\n{log}");
        }
    };
    assert_eq!(removed_path, added_path);

    let list = manager.list_activities().await.unwrap();
    assert!(!list.iter().any(|p| p == &added_path));
}
