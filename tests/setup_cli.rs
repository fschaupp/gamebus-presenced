//! Integration tests for the `gamebus-setup` command line.
//!
//! Everything here drives the real binary as a subprocess with an explicit
//! environment (`Command::env`, never `std::env::set_var` - the test harness is
//! multi-threaded and in-process environment mutation is racy).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

/// A throwaway `$HOME` for one test.
struct TempHome(PathBuf);

impl TempHome {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("gamebus-setup-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("failed to create temporary home");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Every path under this home, so a test can assert nothing was written.
    fn entries(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(read) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in read.flatten() {
                out.push(entry.path());
                if entry.path().is_dir() {
                    walk(&entry.path(), out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.0, &mut out);
        out
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run the tool with a controlled environment.
fn run(home: &TempHome, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gamebus-setup"))
        .args(args)
        .env("HOME", home.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("failed to run gamebus-setup")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn help_lists_the_subcommands() {
    let home = TempHome::new("help");
    let out = run(&home, &["help"]);
    assert!(out.status.success(), "help exited {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stderr);
    for expected in ["status", "plan", "apply", "--target"] {
        assert!(text.contains(expected), "help missing {expected:?}: {text}");
    }
}

#[test]
fn unknown_command_fails_loudly() {
    let home = TempHome::new("unknown");
    let out = run(&home, &["frobnicate"]);
    assert!(!out.status.success(), "unknown command should not succeed");
    assert!(String::from_utf8_lossy(&out.stderr).contains("Unknown command"));
}

#[test]
fn planning_a_user_install_names_every_destination() {
    let home = TempHome::new("plan-user");
    let out = run(&home, &["plan", "install", "--target", "user"]);
    assert!(out.status.success(), "plan exited {:?}", out.status);

    let text = stdout(&out);
    let h = home.path().display();
    for expected in [
        format!("{h}/.local/bin"),
        format!("{h}/.local/share/gamebus-presenced"),
        format!("{h}/.config/systemd/user"),
        format!("{h}/.local/share/dbus-1/services"),
    ] {
        assert!(text.contains(&expected), "plan missing {expected}:\n{text}");
    }
    assert!(text.contains("systemctl --user daemon-reload"), "{text}");
}

/// The whole point of a dry run.
#[test]
fn planning_writes_absolutely_nothing() {
    let home = TempHome::new("plan-pure");
    for target in ["user", "system"] {
        let out = run(&home, &["plan", "install", "--target", target]);
        assert!(out.status.success());
    }
    let entries = home.entries();
    assert!(entries.is_empty(), "plan created files: {entries:?}");
}

#[test]
fn planning_a_system_install_targets_usr_local_and_says_it_needs_root() {
    if skip_if_root("planning_a_system_install_targets_usr_local_and_says_it_needs_root") {
        return;
    }
    let home = TempHome::new("plan-system");
    let out = run(&home, &["plan", "install", "--target", "system"]);
    assert!(out.status.success());

    let text = stdout(&out);
    for expected in [
        "/usr/local/bin/gamebus-presenced",
        "/usr/local/lib/systemd/user",
        "/usr/local/share/gamebus-presenced",
        "/usr/local/share/dbus-1/services",
    ] {
        assert!(text.contains(expected), "plan missing {expected}:\n{text}");
    }
    // Non-root here, so it must say so and offer the exact command.
    assert!(text.contains("requires root"), "{text}");
    assert!(text.contains("sudo "), "{text}");
    assert!(text.contains("--privileged-only"), "{text}");

    // /usr proper is the distribution package manager's; a hand-run install
    // must never plan a write there.
    assert!(
        !text.contains(" /usr/bin/") && !text.contains(" /usr/lib/"),
        "system plan writes into package-manager territory:\n{text}"
    );
    // And nothing may have been touched by a dry run.
    assert!(
        !Path::new("/usr/local/bin/gamebus-presenced").exists(),
        "plan installed a binary into /usr/local"
    );
}

/// Running as root, `needs_root` is correctly false and `apply` would really
/// install to /usr/local - the system layout ignores the TempHome by design, so
/// there is no containment. Skip rather than write.
fn skip_if_root(what: &str) -> bool {
    // SAFETY: geteuid cannot fail and touches no memory we own.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("SKIP: {what} asserts a non-root refusal, and running it as root would install");
        return true;
    }
    false
}

/// `apply` writes; it must refuse to do so by accident.
#[test]
fn apply_refuses_without_confirmation() {
    let home = TempHome::new("apply-unconfirmed");
    let out = run(&home, &["apply", "install", "--target", "user"]);
    assert!(!out.status.success(), "apply ran without --confirm");
    assert!(String::from_utf8_lossy(&out.stderr).contains("--confirm"));
    assert!(home.entries().is_empty(), "refused apply still wrote files");
}

#[test]
fn apply_of_a_system_install_refuses_rather_than_half_failing() {
    if skip_if_root("apply_of_a_system_install_refuses_rather_than_half_failing") {
        return;
    }
    let home = TempHome::new("apply-system");
    let out = run(
        &home,
        &["apply", "install", "--target", "system", "--confirm"],
    );
    // Running as an ordinary user: it must decline up front and print the
    // escalation command, not start writing and fail partway through.
    assert!(!out.status.success(), "apply as non-root should refuse");
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("needs root"), "{text}");
    assert!(text.contains("sudo "), "{text}");
}

#[test]
fn status_runs_without_a_terminal_and_says_what_it_found() {
    // A private bus and runtime dir, so the probe reports on nothing live:
    // without them this would connect to the developer's session bus and to
    // every discord-ipc-N socket on it.
    let Some(env) = common::TestEnv::new("setup-cli-status") else {
        eprintln!("SKIP: could not start a private session bus");
        return;
    };
    let home = TempHome::new("status");
    let out = Command::new(env!("CARGO_BIN_EXE_gamebus-setup"))
        .arg("status")
        .env("HOME", home.path())
        .env("DBUS_SESSION_BUS_ADDRESS", &env.bus_address)
        .env("XDG_RUNTIME_DIR", &env.runtime_dir)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("failed to run gamebus-setup");
    assert!(out.status.success(), "status exited {:?}", out.status);

    let text = stdout(&out);
    for expected in ["gamebus-presenced", "Daemon", "Autostart", "Install target"] {
        assert!(
            text.contains(expected),
            "status missing {expected:?}:\n{text}"
        );
    }
}
