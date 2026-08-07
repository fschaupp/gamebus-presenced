//! Integration tests for the `gamebus-setup` command line.
//!
//! Everything here drives the real binary as a subprocess with an explicit
//! environment (`Command::env`, never `std::env::set_var` — the test harness is
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
    run_env(home, args, &[])
}

/// Same, with extra environment variables (S9b: API/database overrides).
fn run_env(home: &TempHome, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gamebus-setup"));
    cmd.args(args)
        .env("HOME", home.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_CACHE_HOME")
        .env_remove("GAMEBUS_UMU_DB")
        .env_remove("GAMEBUS_UMU_API");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run gamebus-setup")
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
    // The endpoint configuration ships with the install.
    assert!(text.contains("endpoints.toml"), "{text}");
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
/// install to /usr/local — the system layout ignores the TempHome by design, so
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

#[test]
fn umu_misses_lists_and_exports_the_stash() {
    let home = TempHome::new("umu-misses");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    std::fs::write(
        stash_dir.join("umu-misses.json"),
        r#"{"egs:Calluna":{"title":"Control","store":"egs","codename":"Calluna",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "executable":"Control_DX12.exe","first_seen":"2026-08-06",
            "last_seen":"2026-08-07"}}"#,
    )
    .unwrap();

    let list = run(&home, &["umu-misses"]);
    assert!(list.status.success());
    let text = stdout(&list);
    for expected in ["Control", "egs", "Calluna", "high", "heroic-config"] {
        assert!(text.contains(expected), "list missing {expected}:\n{text}");
    }

    let export = run(&home, &["umu-misses", "--export"]);
    assert!(export.status.success());
    let text = stdout(&export);
    // Submission-shaped: the database's own header and a review placeholder
    // for the id (umu-<Steam appid> is the reviewer's call).
    assert!(text.contains("TITLE,STORE,CODENAME,UMU_ID"), "{text}");
    assert!(
        text.contains("Control,egs,Calluna,umu-FIXME"),
        "export row malformed:\n{text}"
    );
}

/// A stash covering all three S9b verification outcomes, and the fixture
/// database that produces them — no network anywhere near these tests
/// (`GAMEBUS_UMU_API` points at a closed port; a query would fail loudly).
const S9B_STASH: &str = r#"{
    "gog:1207600000":{"title":"Already Here","store":"gog","codename":"1207600000",
        "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
        "first_seen":"2026-08-06","last_seen":"2026-08-07"},
    "egs:Bee":{"title":"Borderlands 3","store":"egs","codename":"Bee",
        "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
        "first_seen":"2026-08-06","last_seen":"2026-08-07"},
    "lutris:zzz":{"title":"Zzz Fixture Quest","store":"none",
        "umu_id":"umu-default","title_source":"lutris-wrapper","confidence":"medium",
        "first_seen":"2026-08-06","last_seen":"2026-08-07"}}"#;

const S9B_DB: &str = concat!(
    "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)\n",
    "Borderlands 3,egs,Catnip,umu-397540,bl3,,\n",
    "Already Here,gog,1207600000,umu-111x,,,\n",
);

const CLOSED_PORT_API: (&str, &str) = ("GAMEBUS_UMU_API", "http://127.0.0.1:1");

fn write_s9b_fixtures(home: &TempHome) -> PathBuf {
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    std::fs::write(stash_dir.join("umu-misses.json"), S9B_STASH).unwrap();
    let db = home.path().join("umu-database.csv");
    std::fs::write(&db, S9B_DB).unwrap();
    db
}

#[test]
fn umu_misses_verify_marks_all_three_states_and_drafts_offline() {
    let home = TempHome::new("umu-verify");
    let db = write_s9b_fixtures(&home);

    let out = run_env(
        &home,
        &["umu-misses", "--verify", "--db", db.to_str().unwrap()],
        &[CLOSED_PORT_API],
    );
    assert!(out.status.success(), "verify failed: {out:?}");
    let text = stdout(&out);
    // Launcher-side miss: store+codename found in the database.
    assert!(
        text.contains("already in the database as umu-111x"),
        "{text}"
    );
    // Cross-store: the title exists under another store's entry.
    assert!(text.contains("umu-397540"), "{text}");
    // Confirmed missing → a collision-checked title-slug draft.
    assert!(text.contains("drafted umu-zzzfixturequest"), "{text}");

    // The verdicts persisted into the stash.
    let stash = std::fs::read_to_string(
        home.path()
            .join(".local/share/gamebus-presenced/umu-misses.json"),
    )
    .unwrap();
    for expected in [
        "already-in-database",
        "cross-store-id",
        "confirmed-missing",
        "umu-zzzfixturequest",
        "title-slug",
    ] {
        assert!(
            stash.contains(expected),
            "stash missing {expected}:\n{stash}"
        );
    }

    // Export: the launcher-side miss is held back, the other two carry the
    // verified/drafted ids instead of umu-FIXME.
    let export = run_env(&home, &["umu-misses", "--export"], &[]);
    assert!(export.status.success());
    let text = stdout(&export);
    assert!(text.contains("Borderlands 3,egs,Bee,umu-397540"), "{text}");
    assert!(
        text.contains("Zzz Fixture Quest,none,none,umu-zzzfixturequest"),
        "{text}"
    );
    assert!(
        !text.contains("Already Here"),
        "held-back entry leaked:\n{text}"
    );
    let errtext = String::from_utf8_lossy(&export.stderr);
    assert!(errtext.contains("held back"), "{errtext}");

    // The Markdown export: a slim merge request with the CSV fenced in.
    let md = run_env(&home, &["umu-misses", "--export-md"], &[]);
    assert!(md.status.success());
    let text = stdout(&md);
    assert!(text.contains("# Add 2 games"), "{text}");
    assert!(text.contains("```csv"), "{text}");
    assert!(text.contains("## Evidence"), "{text}");
    assert!(text.contains("## Checklist"), "{text}");
    assert!(text.contains("collision-checked"), "{text}");
    // One row is a title-slug draft — nothing proves that game is absent
    // from Steam, so the Steam-rule box must stay for the human.
    assert!(
        text.contains("- [ ] Every id follows the database rules"),
        "steam-rule box was pre-ticked over a slug draft:\n{text}"
    );
}

#[test]
fn umu_misses_export_escapes_external_data_and_strips_paths() {
    let home = TempHome::new("umu-escape");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    // Codename comes verbatim from an untrusted process's environment: a
    // comma in it must not shift the CSV columns (that would forge the
    // UMU_ID cell). The executable is an absolute path with the username in
    // it — only the basename may reach a public submission.
    std::fs::write(
        stash_dir.join("umu-misses.json"),
        r#"{"egs:evil":{"title":"Bad Game","store":"egs","codename":"Evil,umu-hijack,x",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "executable":"/home/private-user/Games/Heroic/Bad Game/Game.exe",
            "first_seen":"2026-08-07","last_seen":"2026-08-07"}}"#,
    )
    .unwrap();

    let export = run(&home, &["umu-misses", "--export"]);
    assert!(export.status.success());
    let text = stdout(&export);
    assert!(
        text.contains(r#"Bad Game,egs,"Evil,umu-hijack,x",umu-FIXME"#),
        "codename not escaped — columns shifted:\n{text}"
    );
    assert!(
        !text.contains("/home/private-user"),
        "absolute path leaked into the export:\n{text}"
    );
    assert!(text.contains("Game.exe"), "{text}");
}

#[test]
fn endpoints_come_from_the_config_file_not_only_the_env() {
    let home = TempHome::new("umu-endpoints");
    write_s9b_fixtures(&home);
    // A user override in ~/.config points the umu API at a closed port; NO
    // env var and no --db. Verify must try that URL — and fail honestly —
    // proving the file was read. With the compiled default it would reach
    // the real API instead.
    let conf_dir = home.path().join(".config/gamebus-presenced");
    std::fs::create_dir_all(&conf_dir).unwrap();
    std::fs::write(
        conf_dir.join("endpoints.toml"),
        "[umu]\napi = \"http://127.0.0.1:1\"\n",
    )
    .unwrap();

    let out = run(&home, &["umu-misses", "--verify"]);
    assert!(
        !out.status.success(),
        "verify should have hit the closed port"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("127.0.0.1:1"),
        "config file was ignored:\n{err}"
    );
}

#[test]
fn dismissed_entries_stay_in_the_stash_but_out_of_the_exports() {
    let home = TempHome::new("umu-dismissed");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    std::fs::write(
        stash_dir.join("umu-misses.json"),
        r#"{"egs:Keep":{"title":"Keep Me","store":"egs","codename":"Keep",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-07","last_seen":"2026-08-07"},
        "egs:Skip":{"title":"Skip Me","store":"egs","codename":"Skip",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-07","last_seen":"2026-08-07",
            "dismissed":"2026-08-07"}}"#,
    )
    .unwrap();

    let export = run(&home, &["umu-misses", "--export"]);
    assert!(export.status.success());
    let text = stdout(&export);
    assert!(text.contains("Keep Me"), "{text}");
    assert!(
        !text.contains("Skip Me"),
        "dismissed entry exported:\n{text}"
    );
    let err = String::from_utf8_lossy(&export.stderr);
    assert!(err.contains("dismissed by you"), "{err}");

    // Still visible (marked) in the review list — parked, not deleted.
    let list = run(&home, &["umu-misses"]);
    let text = stdout(&list);
    assert!(text.contains("Skip Me"), "{text}");
    assert!(text.contains("dismissed 2026-08-07"), "{text}");
}

#[test]
fn a_corrupt_stash_errors_loudly_and_stays_untouched() {
    let home = TempHome::new("umu-corrupt");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    let stash_path = stash_dir.join("umu-misses.json");
    std::fs::write(&stash_path, "{\"oops\": ,}").unwrap();
    let before = std::fs::read(&stash_path).unwrap();

    // Both the list and the flows must refuse — "No misses recorded" over a
    // corrupt file would read as data loss.
    for args in [vec!["umu-misses"], vec!["umu-misses", "--export"]] {
        let out = run(&home, &args);
        assert!(
            !out.status.success(),
            "{args:?} succeeded on a corrupt stash"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("failed to parse"), "{err}");
    }
    assert_eq!(before, std::fs::read(&stash_path).unwrap());
}

/// A one-thread fake umu API: exact-miss on the codename lookup, fuzzy ids
/// on the title lookup — the live API's observed behavior (`?title=Control`
/// returns Ground Control's ids, with no title field to compare against).
fn spawn_fake_umu_api() -> String {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let body = if req.contains("codename=") {
                "[]"
            } else if req.contains("title=") {
                r#"[{"umu_id":"umu-254820"},{"umu_id":"umu-254840"}]"#
            } else {
                "[]"
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });
    format!("http://{addr}")
}

#[test]
fn umu_misses_fuzzy_api_title_hits_stay_advisory() {
    let home = TempHome::new("umu-fuzzy");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    // Not in the fixture database, so verification reaches the API; the
    // fake API answers the title lookup with someone else's ids.
    std::fs::write(
        stash_dir.join("umu-misses.json"),
        r#"{"egs:KZed":{"title":"Kontrol Zed","store":"egs","codename":"KZed",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-07","last_seen":"2026-08-07"}}"#,
    )
    .unwrap();
    let db = home.path().join("umu-database.csv");
    std::fs::write(&db, S9B_DB).unwrap();

    let api = spawn_fake_umu_api();
    let out = run_env(
        &home,
        &["umu-misses", "--verify", "--db", db.to_str().unwrap()],
        &[("GAMEBUS_UMU_API", api.as_str())],
    );
    assert!(out.status.success(), "verify failed: {out:?}");

    let stash = std::fs::read_to_string(stash_dir.join("umu-misses.json")).unwrap();
    // Fuzzy title matches must never become the verdict: the entry stays
    // confirmed-missing, gets its own drafted id, and the ids the API
    // offered appear only in the human-facing note.
    assert!(stash.contains("confirmed-missing"), "{stash}");
    assert!(!stash.contains("cross-store-id"), "{stash}");
    assert!(stash.contains("umu-kzed"), "{stash}");
    assert!(stash.contains("substring match"), "{stash}");
    assert!(
        !stash.contains(r#""id": "umu-254820""#),
        "a fuzzy id landed in a verdict:\n{stash}"
    );
}

#[test]
fn umu_misses_verify_without_any_database_fails_and_touches_nothing() {
    let home = TempHome::new("umu-verify-offline");
    write_s9b_fixtures(&home); // the --db flag is NOT passed, so only the API remains
    let stash_path = home
        .path()
        .join(".local/share/gamebus-presenced/umu-misses.json");
    let before = std::fs::read(&stash_path).unwrap();

    let out = run_env(&home, &["umu-misses", "--verify"], &[CLOSED_PORT_API]);
    assert!(
        !out.status.success(),
        "verify must fail with nothing to verify against"
    );
    let errtext = String::from_utf8_lossy(&out.stderr);
    assert!(errtext.contains("Verify failed"), "{errtext}");

    // Honest failure means an untouched stash — byte for byte.
    assert_eq!(before, std::fs::read(&stash_path).unwrap());
}
