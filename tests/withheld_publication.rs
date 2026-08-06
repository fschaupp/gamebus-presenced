//! Integration test for S7 publish-once-named, end to end.
//!
//! A keyless process whose executable is a blacklisted wrapper (`sh`) is
//! registered with the private bus's gamemoded: the bus must stay silent —
//! no ActivityAdded, and on unregister no ActivityRemoved either (it was
//! never published). A plain `sleep` registered in the same session is the
//! positive control: its stem is a truthful name and publishes as always.
//!
//! Private session bus and runtime directory throughout (tests/common).

use std::process::Stdio;
use std::time::Duration;
use zbus::export::futures_util::StreamExt;

mod common;
use common::{ChildGuard, ManagerProxy};

const WAIT: Duration = Duration::from_secs(10);
const SILENCE_WINDOW: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread")]
async fn nameless_wrapper_never_reaches_the_bus() {
    let Some(env) = common::TestEnv::new("withheld") else {
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

    let mut daemon = env.spawn_daemon("gamebus-presenced-test-withheld.log");
    common::expect_own_daemon(&conn, daemon.pid(), "withheld").await;
    let manager = ManagerProxy::new(&conn).await.unwrap();
    let mut added = manager.receive_activity_added().await.unwrap();
    let mut removed = manager.receive_activity_removed().await.unwrap();

    // The nameless case: `sh` is on the wrapper list, so its stem clears to
    // empty; the `;:` keeps sh from exec-ing into sleep, so the exe stays sh.
    // No SteamAppId/LUTRIS env → keyless → ungrouped → withheld.
    let wrapper = env
        .command("sh")
        .args(["-c", "sleep 60;:"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn the wrapper");
    let wrapper_pid = wrapper.id();
    let _wrapper = ChildGuard(wrapper);
    assert!(
        env.gamemoded_call("RegisterGameByPID", wrapper_pid),
        "RegisterGameByPID failed for the wrapper"
    );

    // Positive control in the same session: a plain sleep, stem "sleep",
    // publishes as always — proving the daemon is alive and the gate
    // is selective, not silent.
    let control = env
        .command("sleep")
        .arg("60")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn the control");
    let control_pid = control.id();
    let _control = ChildGuard(control);
    assert!(
        env.gamemoded_call("RegisterGameByPID", control_pid),
        "RegisterGameByPID failed for the control"
    );

    // The control MUST appear …
    let control_path = format!("/org/gamebus/Presence/v1/Activity/pid_{control_pid}");
    let list = common::wait_for_activities(&manager, WAIT, |list| {
        list.iter().any(|p| p.as_str() == control_path)
    })
    .await;
    assert!(
        list.iter().any(|p| p.as_str() == control_path),
        "positive control never published: {list:?}"
    );

    // … and the wrapper must not, now or ever.
    let wrapper_path = format!("/org/gamebus/Presence/v1/Activity/pid_{wrapper_pid}");
    assert!(
        !manager
            .list_activities()
            .await
            .unwrap_or_default()
            .iter()
            .any(|p| p.as_str() == wrapper_path),
        "nameless wrapper was published"
    );

    // Unregister the wrapper: total silence for its pid — it was never on
    // the bus, so there is nothing to remove. Drain signals for a window and
    // reject anything concerning the wrapper.
    env.gamemoded_call("UnregisterGameByPID", wrapper_pid);
    let deadline = tokio::time::Instant::now() + SILENCE_WINDOW;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            sig = added.next() => {
                if let Some(sig) = sig {
                    if let Ok(args) = sig.args() {
                        let path = args.object_path.as_str();
                        assert!(
                            !path.ends_with(&format!("pid_{wrapper_pid}"))
                                && !path.ends_with(&format!("steam_{wrapper_pid}")),
                            "withheld wrapper appeared on the bus: {path}"
                        );
                    }
                }
            }
            sig = removed.next() => {
                if let Some(sig) = sig {
                    if let Ok(args) = sig.args() {
                        let path = args.object_path.as_str();
                        assert!(
                            !path.ends_with(&format!("pid_{wrapper_pid}"))
                                && !path.ends_with(&format!("steam_{wrapper_pid}")),
                            "never-published wrapper produced a removal: {path}"
                        );
                    }
                }
            }
        }
    }

    // The control still lives and dies normally — one clean removal.
    env.gamemoded_call("UnregisterGameByPID", control_pid);
    let list = common::wait_for_activities(&manager, WAIT, |list| {
        !list.iter().any(|p| p.as_str() == control_path)
    })
    .await;
    assert!(
        !list.iter().any(|p| p.as_str() == control_path),
        "control record survived unregistration: {list:?}"
    );
    daemon.kill_now();
}
