//! Integration tests for game groups: processes sharing a merge key
//! collapse into one published record.
//!
//! Two end-to-end properties on the private-bus fixture:
//! - helper churn (extra members registering and dying) never reaches the bus;
//! - the record survives the death of its representative while another member
//!   still holds evidence, and the whole lifecycle is one Added + one Removed.
//!
//! Both tests spawn plain `sleep` processes carrying a `SteamAppId` environ
//! entry, so the two members of each group probe to the same `steam:<appid>`
//! merge key. Distinct appids per test: each daemon scans the real `/proc`,
//! so a shared appid would let one test's daemon adopt the other test's
//! processes as evidence and hold its group open.
//!
//! The daemon's periodic scan cannot create records for these processes on its
//! own (`sleep` is not under `/steamapps/` and never identifies), and any
//! records it publishes for *real* games running on the host are filtered out
//! by pid, matching the existing tests' tolerance.

use std::process::Stdio;
use std::time::Duration;
use zbus::export::futures_util::StreamExt;

mod common;
use common::{ActivityAddedStream, ActivityPropsProxy, ActivityRemovedStream, ChildGuard};

const ADDED_TIMEOUT: Duration = Duration::from_secs(20);
/// Window in which a suppressed event must NOT appear. Short enough that each
/// lifecycle ends before the daemon's 15s tick sweep can migrate a dead rep
/// (which would legitimately emit a publish-first pair).
const SILENCE_WINDOW: Duration = Duration::from_secs(5);

/// Whether a signal path concerns one of our group's pids (`pid_<p>` records
/// or their `steam_<p>` partials). Records for unrelated processes the
/// daemon's real-`/proc` scan may publish are ignored.
fn concerns(path: &str, pids: &[u32]) -> bool {
    pids.iter()
        .any(|p| path.ends_with(&format!("pid_{p}")) || path.ends_with(&format!("steam_{p}")))
}

/// Drain both signal streams for `window`, panicking on any Added/Removed
/// that concerns our pids. Silence on the bus is the property under test.
async fn assert_group_silence(
    added: &mut ActivityAddedStream<'_>,
    removed: &mut ActivityRemovedStream<'_>,
    pids: &[u32],
    window: Duration,
    label: &str,
) {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return,
            sig = added.next() => {
                if let Some(sig) = sig {
                    if let Ok(args) = sig.args() {
                        let path = args.object_path.as_str();
                        assert!(
                            !concerns(path, pids),
                            "{label}: unexpected ActivityAdded for {path}"
                        );
                    }
                }
            }
            sig = removed.next() => {
                if let Some(sig) = sig {
                    if let Ok(args) = sig.args() {
                        let path = args.object_path.as_str();
                        assert!(
                            !concerns(path, pids),
                            "{label}: unexpected ActivityRemoved for {path}"
                        );
                    }
                }
            }
        }
    }
}

/// Drain the Added stream for `window`, panicking on any `steam_<pid>`
/// record for our pids - the transient re-publish flash at teardown that the
/// Steam-first removal order eliminates (exactly one ActivityRemoved).
/// `pid_<pid>` traffic is tolerated here: a tick-sweep migration pair is
/// legitimate when the daemon's 15s tick lands inside the lifecycle.
async fn assert_no_steam_flash(
    added: &mut ActivityAddedStream<'_>,
    pids: &[u32],
    window: Duration,
    label: &str,
) {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return,
            sig = added.next() => {
                if let Some(sig) = sig {
                    if let Ok(args) = sig.args() {
                        let path = args.object_path.as_str();
                        assert!(
                            !pids.iter().any(|p| path.ends_with(&format!("steam_{p}"))),
                            "{label}: transient steam record published: {path}"
                        );
                    }
                }
            }
        }
    }
}

/// Spawn a long-sleeping "game" process carrying `SteamAppId` in its environ.
///
/// `wrap_in_sh` runs it as `sh -c 'exec sleep 60'` - a distinct executable for
/// the second member. `exec` keeps it a single process: a forked grandchild
/// would survive the kill below still carrying the key, and its scan evidence
/// would hold the group open past the final unregister.
fn spawn_member(env: &common::TestEnv, appid: &str, wrap_in_sh: bool) -> ChildGuard {
    let mut cmd = if wrap_in_sh {
        let mut c = env.command("sh");
        c.args(["-c", "exec sleep 60"]);
        c
    } else {
        let mut c = env.command("sleep");
        c.arg("60");
        c
    };
    let child = cmd
        .env("SteamAppId", appid)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn group member");
    ChildGuard(child)
}

/// Wait for the ActivityAdded matching `pid`, with the daemon log on timeout.
async fn wait_for_added(
    added: &mut ActivityAddedStream<'_>,
    pid: u32,
    log_name: &str,
) -> zbus::zvariant::OwnedObjectPath {
    let expected_suffix = format!("pid_{pid}");
    match tokio::time::timeout(ADDED_TIMEOUT, async {
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
    {
        Ok(Some(path)) => path,
        other => {
            let log =
                std::fs::read_to_string(std::env::temp_dir().join(log_name)).unwrap_or_default();
            panic!("timed out waiting for ActivityAdded ({other:?});\n--- daemon log ---\n{log}");
        }
    }
}

/// Wait for the ActivityRemoved matching `path`, with the daemon log on timeout.
async fn wait_for_removed(
    removed: &mut ActivityRemovedStream<'_>,
    path: &zbus::zvariant::OwnedObjectPath,
    log_name: &str,
) {
    match tokio::time::timeout(ADDED_TIMEOUT, async {
        while let Some(signal) = removed.next().await {
            if let Ok(args) = signal.args() {
                if &args.object_path == path {
                    return Some(());
                }
            }
        }
        None
    })
    .await
    {
        Ok(Some(())) => {}
        other => {
            let log =
                std::fs::read_to_string(std::env::temp_dir().join(log_name)).unwrap_or_default();
            panic!("timed out waiting for ActivityRemoved ({other:?});\n--- daemon log ---\n{log}");
        }
    }
}

/// R1 + R3 end to end: a second same-key member registering and dying
/// produces zero bus traffic; the whole lifecycle is one Added + one Removed.
#[tokio::test(flavor = "multi_thread")]
async fn helper_churn_never_reaches_bus() {
    let Some(env) = common::TestEnv::new("groups-churn") else {
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

    const LOG: &str = "gamebus-presenced-test-groups-churn.log";
    let _daemon = env.spawn_daemon(LOG);
    common::expect_own_daemon(&conn, _daemon.pid(), "groups-churn").await;

    let manager = common::ManagerProxy::new(&conn).await.unwrap();
    let mut added = manager.receive_activity_added().await.unwrap();
    let mut removed = manager.receive_activity_removed().await.unwrap();

    // First registered member becomes representative and publishes.
    let rep = spawn_member(&env, "90001", false);
    let rep_pid = rep.pid();
    let _rep = rep;
    assert!(
        env.gamemoded_call("RegisterGameByPID", rep_pid),
        "RegisterGameByPID(rep) failed"
    );
    let added_path = wait_for_added(&mut added, rep_pid, LOG).await;

    // Second member with the same key: equal class, absorbed - silent.
    let mut helper = spawn_member(&env, "90001", true);
    let helper_pid = helper.pid();
    assert!(
        env.gamemoded_call("RegisterGameByPID", helper_pid),
        "RegisterGameByPID(helper) failed"
    );
    let pids = [rep_pid, helper_pid];
    assert_group_silence(&mut added, &mut removed, &pids, SILENCE_WINDOW, "absorb").await;

    // Kill and unregister the absorbed member: the bus never saw it, so its
    // death must produce nothing - no Removed, and the record stays merged.
    helper.kill_now();
    assert!(
        env.gamemoded_call("UnregisterGameByPID", helper_pid),
        "UnregisterGameByPID(helper) failed"
    );
    assert_group_silence(
        &mut added,
        &mut removed,
        &pids,
        SILENCE_WINDOW,
        "helper death",
    )
    .await;

    let list = manager.list_activities().await.unwrap();
    assert!(
        list.iter().any(|p| p == &added_path),
        "record vanished after helper death: {list:?}"
    );
    let activity = ActivityPropsProxy::builder(&conn)
        .path(added_path.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    let sources = activity.sources().await.unwrap();
    assert!(
        sources.contains(&"gamemode".to_string()),
        "record lost gamemode after helper death: {sources:?}"
    );

    // Unregister the representative: last evidence gone, exactly one Removed.
    assert!(
        env.gamemoded_call("UnregisterGameByPID", rep_pid),
        "UnregisterGameByPID(rep) failed"
    );
    wait_for_removed(&mut removed, &added_path, LOG).await;

    // Teardown must not flash a transient steam_<pid> record (one
    // ActivityRemoved total): the Steam partial is removed first so the
    // record degrades in place instead of re-publishing.
    assert_no_steam_flash(&mut added, &pids, Duration::from_secs(2), "teardown").await;

    let list = manager.list_activities().await.unwrap();
    assert!(!list.iter().any(|p| p == &added_path));
}

/// Deferred migration end to end: killing the representative while a second
/// member still holds a GameMode registration keeps the record published
/// under its existing id; the whole lifecycle is one Added + one Removed.
#[tokio::test(flavor = "multi_thread")]
async fn record_survives_rep_death() {
    let Some(env) = common::TestEnv::new("groups-repdeath") else {
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

    const LOG: &str = "gamebus-presenced-test-groups-repdeath.log";
    let _daemon = env.spawn_daemon(LOG);
    common::expect_own_daemon(&conn, _daemon.pid(), "groups-repdeath").await;

    let manager = common::ManagerProxy::new(&conn).await.unwrap();
    let mut added = manager.receive_activity_added().await.unwrap();
    let mut removed = manager.receive_activity_removed().await.unwrap();

    let mut rep = spawn_member(&env, "90002", false);
    let rep_pid = rep.pid();
    assert!(
        env.gamemoded_call("RegisterGameByPID", rep_pid),
        "RegisterGameByPID(rep) failed"
    );
    let added_path = wait_for_added(&mut added, rep_pid, LOG).await;

    let survivor = spawn_member(&env, "90002", true);
    let survivor_pid = survivor.pid();
    let _survivor = survivor;
    assert!(
        env.gamemoded_call("RegisterGameByPID", survivor_pid),
        "RegisterGameByPID(survivor) failed"
    );
    // Let the absorption land before killing the rep, so the survivor's
    // evidence is on record when the rep's removal is decided.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Kill and unregister the FIRST-registered member (the rep). The survivor
    // still holds a registration, so deferred migration holds the record
    // under the existing pid_<rep> id: no Removed, no replacement Added.
    rep.kill_now();
    assert!(
        env.gamemoded_call("UnregisterGameByPID", rep_pid),
        "UnregisterGameByPID(rep) failed"
    );
    let pids = [rep_pid, survivor_pid];
    assert_group_silence(&mut added, &mut removed, &pids, SILENCE_WINDOW, "rep death").await;

    let list = manager.list_activities().await.unwrap();
    assert!(
        list.iter().any(|p| p == &added_path),
        "record did not survive rep death: {list:?}"
    );

    // Unregister the survivor: last evidence gone, exactly one Removed - and
    // it is for the original pid_<rep> id the record was published under.
    assert!(
        env.gamemoded_call("UnregisterGameByPID", survivor_pid),
        "UnregisterGameByPID(survivor) failed"
    );
    wait_for_removed(&mut removed, &added_path, LOG).await;

    // Teardown must not flash a transient steam_<pid> record (one
    // ActivityRemoved total): the Steam partial is removed first so the
    // record degrades in place instead of re-publishing.
    assert_no_steam_flash(&mut added, &pids, Duration::from_secs(2), "teardown").await;

    let list = manager.list_activities().await.unwrap();
    assert!(!list.iter().any(|p| p == &added_path));
}
