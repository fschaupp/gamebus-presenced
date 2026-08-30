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
        .env_remove("GAMEBUS_UMU_API")
        .env_remove("GAMEBUS_UMU_PROTONFIXES")
        .env_remove("GAMEBUS_GAMEDB_INDEX");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run gamebus-setup")
}

/// Annotate a stash fixture entry the way `--verify` does for a game that
/// needs umu: a collision-checked id, plus the protonfix that justifies a
/// database row at all. Upstream only wants games that require a fix in
/// Proton, so an export fixture without this is held back - which is what
/// the fixtures that omit it are there to prove.
fn needs_umu(stash: &str, key: &str, id: &str) -> String {
    let anchor = format!("\"{key}\":{{");
    let annotations = format!(
        "\"drafted_id\":{{\"id\":\"{id}\",\"basis\":\"steam-sku\",\
           \"collision_checked\":\"2026-08-22\"}},\
         \"fix\":{{\"umu_id\":\"{id}\",\"fixes\":[\"gamefixes-steam/{n}.py\"],\
           \"checked\":\"2026-08-22\"}},",
        n = id.trim_start_matches("umu-")
    );
    let annotated = stash.replacen(&anchor, &format!("{anchor}{annotations}"), 1);
    assert_ne!(annotated, stash, "no stash entry keyed {key}");
    annotated
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
    // The endpoint configuration and shared-helper list ship with the install.
    assert!(text.contains("endpoints.toml"), "{text}");
    assert!(text.contains("shared-helpers.txt"), "{text}");
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

#[test]
fn umu_misses_lists_and_exports_the_stash() {
    let home = TempHome::new("umu-misses");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    // Two verified entries: Control runs under Proton without a protonfix,
    // so upstream does not want a row for it; Fixture Quest needs one. Both
    // stay in the review list - only the export tells them apart.
    let stash = r#"{"egs:Calluna":{"title":"Control","store":"egs","codename":"Calluna",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "executable":"Control_DX12.exe","first_seen":"2026-08-06",
            "last_seen":"2026-08-07",
            "drafted_id":{"id":"umu-870780","basis":"steam-sku","collision_checked":"2026-08-22"},
            "fix":{"umu_id":"umu-870780","fixes":[],"checked":"2026-08-22"}},
        "egs:Catnip":{"title":"Fixture Quest","store":"egs","codename":"Catnip",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-06","last_seen":"2026-08-07"}}"#;
    std::fs::write(
        stash_dir.join("umu-misses.json"),
        needs_umu(stash, "egs:Catnip", "umu-397540"),
    )
    .unwrap();

    let list = run(&home, &["umu-misses"]);
    assert!(list.status.success());
    let text = stdout(&list);
    for expected in ["Control", "egs", "Calluna", "high", "heroic-config"] {
        assert!(text.contains(expected), "list missing {expected}:\n{text}");
    }
    assert!(
        text.contains("no protonfix for umu-870780"),
        "the list hides that Control needs no entry:\n{text}"
    );

    let export = run(&home, &["umu-misses", "--export"]);
    assert!(export.status.success());
    let text = stdout(&export);
    // Submission-shaped: the database's own header, and only the game that
    // actually needs umu.
    assert!(text.contains("TITLE,STORE,CODENAME,UMU_ID"), "{text}");
    assert!(
        text.contains("Fixture Quest,egs,Catnip,umu-397540,,,"),
        "export row malformed:\n{text}"
    );
    assert!(
        !text.contains("Control,egs,Calluna"),
        "a game that needs no protonfix reached the submission:\n{text}"
    );
    let err = String::from_utf8_lossy(&export.stderr);
    assert!(
        err.contains("Control") && err.contains("no protonfix"),
        "hold-back reason missing the scope rule:\n{err}"
    );
}

/// A stash covering all three S9b verification outcomes, and the fixture
/// database that produces them - no network anywhere near these tests
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

/// The protonfix list a verification run reads: both fixture games need a
/// fix, so both belong in the database. Steam files are named by the bare
/// appid, every other id by itself - the two shapes upstream uses.
const S9B_FIXES: &str = concat!(
    "gamefixes-steam/397540.py\n",
    "gamefixes-umu/umu-zzzfixturequest.py\n",
);

#[test]
fn umu_misses_verify_marks_all_three_states_and_drafts_offline() {
    let home = TempHome::new("umu-verify");
    let db = write_s9b_fixtures(&home);
    let fixes = home.path().join("protonfixes.txt");
    std::fs::write(&fixes, S9B_FIXES).unwrap();

    let out = run_env(
        &home,
        &["umu-misses", "--verify", "--db", db.to_str().unwrap()],
        &[
            CLOSED_PORT_API,
            ("GAMEBUS_UMU_PROTONFIXES", fixes.to_str().unwrap()),
        ],
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
    // And the scope rule: both games have a protonfix, so both are worth
    // submitting - the summary names the fix that says so.
    assert!(
        text.contains("needs umu: protonfix gamefixes-steam/397540.py"),
        "{text}"
    );
    assert!(
        text.contains("needs umu: protonfix gamefixes-umu/umu-zzzfixturequest.py"),
        "{text}"
    );

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
        "gamefixes-steam/397540.py",
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
    assert!(
        text.contains("Borderlands 3,egs,Bee,umu-397540,,,"),
        "{text}"
    );
    assert!(
        text.contains("Zzz Fixture Quest,none,none,umu-zzzfixturequest,,,"),
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
    // Every row states the fix that earns it a place in the database.
    assert!(text.contains("already has a protonfix"), "{text}");
    assert!(
        text.contains("fix: [`gamefixes-steam/397540.py`](https://github.com/"),
        "the evidence does not link the fix:\n{text}"
    );
    // One row is a title-slug draft - nothing proves that game is absent
    // from Steam, so the Steam-rule box must stay for the human.
    assert!(
        text.contains("- [ ] Every id follows the database rules"),
        "steam-rule box was pre-ticked over a slug draft:\n{text}"
    );
    // Conformance boxes: the gog rule is enforced by the hold-back, the egs
    // fixture's store came from the daemon (no override). Mechanical
    // guarantees (lowercase STORE, empty NOTE) claim no box: the checklist
    // is for what a human verifies.
    assert!(
        text.contains("- [x] GOG codenames are numeric gogdb.org"),
        "{text}"
    );
    assert!(
        text.contains("- [x] EGS codenames are the egdata.app Builds"),
        "{text}"
    );
    assert!(!text.contains("NOTE column carries"), "{text}");
    assert!(!text.contains("Store ids are lowercase"), "{text}");
    // Provenance lives in the evidence, not in the CSV rows.
    assert!(text.contains("## Evidence"), "{text}");
    assert!(
        !text.contains("collision-checked 2026-08-07\","),
        "provenance leaked into a CSV cell:\n{text}"
    );
}

#[test]
fn gog_rows_without_a_gogdb_product_id_are_held_back() {
    let home = TempHome::new("umu-gogdb");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    // The database's GOG rule: codename = numeric gogdb.org product id.
    // Heroic GOG launches carry exactly that; a name-shaped codename means
    // somebody has to look the id up before this row may be submitted.
    let stash = r#"{"gog:1423049311":{"title":"Numeric Fine","store":"gog","codename":"1423049311",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-08","last_seen":"2026-08-08"},
        "gog:witchery":{"title":"Name Shaped","store":"gog","codename":"witchery",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-08","last_seen":"2026-08-08"}}"#;
    // Both games need umu, so the codename rule is the only thing that can
    // hold one back.
    let stash = needs_umu(stash, "gog:1423049311", "umu-1000001");
    std::fs::write(
        stash_dir.join("umu-misses.json"),
        needs_umu(&stash, "gog:witchery", "umu-1000002"),
    )
    .unwrap();

    let export = run(&home, &["umu-misses", "--export"]);
    assert!(export.status.success());
    let text = stdout(&export);
    assert!(
        text.contains("Numeric Fine,gog,1423049311,umu-1000001,,,"),
        "{text}"
    );
    assert!(
        !text.contains("Name Shaped"),
        "non-gogdb codename reached the submission:\n{text}"
    );
    let err = String::from_utf8_lossy(&export.stderr);
    assert!(
        err.contains("gogdb.org") && err.contains("witchery"),
        "hold-back reason missing the gogdb rule:\n{err}"
    );
}

#[test]
fn a_codename_override_reaches_the_export_and_passes_the_gog_gate() {
    let home = TempHome::new("umu-codename-override");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    // Two annotated identities: a gog entry whose launcher-reported codename
    // is name-shaped (held back on its own - see the gogdb-gate test) but
    // whose override is the numeric product id, and an egs entry whose
    // override replaces a wrong App Name. The daemon-owned codename field
    // stays untouched in both; the export must use the overrides.
    let stash = r#"{"gog:witchery":{"title":"Name Shaped","store":"gog","codename":"witchery",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-08","last_seen":"2026-08-08",
            "codename_override":"1423049311"},
        "egs:WrongName":{"title":"Control","store":"egs","codename":"WrongName",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-08","last_seen":"2026-08-08",
            "codename_override":"Calluna"}}"#;
    let stash = needs_umu(stash, "gog:witchery", "umu-1000001");
    std::fs::write(
        stash_dir.join("umu-misses.json"),
        needs_umu(&stash, "egs:WrongName", "umu-1000002"),
    )
    .unwrap();

    let export = run(&home, &["umu-misses", "--export"]);
    assert!(export.status.success());
    let text = stdout(&export);
    assert!(
        text.contains("Name Shaped,gog,1423049311,umu-1000001,,,"),
        "gog override missing or gate still held it back:\n{text}"
    );
    assert!(
        text.contains("Control,egs,Calluna,umu-1000002,,,"),
        "egs override missing:\n{text}"
    );
    assert!(
        !text.contains("witchery") && !text.contains("WrongName"),
        "a launcher-reported codename leaked past its override:\n{text}"
    );
    let err = String::from_utf8_lossy(&export.stderr);
    assert!(!err.contains("held back"), "{err}");
}

#[test]
fn a_title_override_reaches_the_export_and_counts_as_confident() {
    let home = TempHome::new("umu-title-override");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    // The Spellcraft incident: a shared helper exe resolved the wrong title
    // at high confidence, and a second entry whose resolver only managed a
    // low-confidence stem. Both carry the user's correction - the export
    // must use it, and the correction alone must pass the confidence gate.
    let stash = r#"{"gog:1660194629":{"title":"Spellcraft","store":"gog","codename":"1660194629",
            "umu_id":"umu-0","title_source":"detectable","confidence":"high",
            "executable":"UnityCrashHandler64.exe",
            "first_seen":"2026-08-08","last_seen":"2026-08-08",
            "title_override":"Project Hospital"},
        "egs:Stemmed":{"title":"stemmed","store":"egs","codename":"Stemmed",
            "umu_id":"umu-0","title_source":"stem","confidence":"low",
            "first_seen":"2026-08-08","last_seen":"2026-08-08",
            "title_override":"The Real Title"}}"#;
    let stash = needs_umu(stash, "gog:1660194629", "umu-1000001");
    std::fs::write(
        stash_dir.join("umu-misses.json"),
        needs_umu(&stash, "egs:Stemmed", "umu-1000002"),
    )
    .unwrap();

    let export = run(&home, &["umu-misses", "--export"]);
    assert!(export.status.success());
    let text = stdout(&export);
    assert!(
        text.contains("Project Hospital,gog,1660194629,umu-1000001,,,"),
        "overridden title missing from the row:\n{text}"
    );
    assert!(
        text.contains("The Real Title,egs,Stemmed,umu-1000002,,,"),
        "a title override did not pass the confidence gate:\n{text}"
    );
    assert!(
        !text.contains("Spellcraft") && !text.contains("stemmed,"),
        "a resolver title leaked past its override:\n{text}"
    );
    let err = String::from_utf8_lossy(&export.stderr);
    assert!(!err.contains("held back"), "{err}");

    // The merge-request evidence says who set the title - honestly.
    let md = run(&home, &["umu-misses", "--export-md"]);
    assert!(md.status.success());
    let text = stdout(&md);
    assert!(
        text.contains("title set by you (resolver said 'Spellcraft')"),
        "evidence hides the override:\n{text}"
    );
    assert!(
        !text.contains("title from detectable"),
        "evidence claims resolver provenance over an override:\n{text}"
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
    // it - only the basename may reach a public submission.
    let stash = r#"{"egs:evil":{"title":"Bad Game","store":"egs","codename":"Evil,umu-hijack,x",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "executable":"/home/private-user/Games/Heroic/Bad Game/Game.exe",
            "first_seen":"2026-08-07","last_seen":"2026-08-07"}}"#;
    std::fs::write(
        stash_dir.join("umu-misses.json"),
        needs_umu(stash, "egs:evil", "umu-1000001"),
    )
    .unwrap();

    let export = run(&home, &["umu-misses", "--export"]);
    assert!(export.status.success());
    let text = stdout(&export);
    assert!(
        text.contains(r#"Bad Game,egs,"Evil,umu-hijack,x",umu-1000001,,,Game.exe"#),
        "codename not escaped - columns shifted:\n{text}"
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
    // env var and no --db. Verify must try that URL - and fail honestly -
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
    let stash = r#"{"egs:Keep":{"title":"Keep Me","store":"egs","codename":"Keep",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-07","last_seen":"2026-08-07"},
        "egs:Skip":{"title":"Skip Me","store":"egs","codename":"Skip",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-07","last_seen":"2026-08-07",
            "dismissed":"2026-08-07"}}"#;
    let stash = needs_umu(stash, "egs:Keep", "umu-1000001");
    std::fs::write(
        stash_dir.join("umu-misses.json"),
        needs_umu(&stash, "egs:Skip", "umu-1000002"),
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

    // Still visible (marked) in the review list - parked, not deleted.
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

    // Both the list and the flows must refuse - "No misses recorded" over a
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
/// on the title lookup - the live API's observed behavior (`?title=Control`
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

    // Honest failure means an untouched stash - byte for byte.
    assert_eq!(before, std::fs::read(&stash_path).unwrap());
}

/// The owner policy (2026-08-24): umu candidacy is opt-in. A stash mixing a
/// promoted umu miss, an unpromoted one with no fix, and a launcher launch
/// (empty umu id) exports exactly the promoted one - and the review list
/// still shows all three, each with its tag.
#[test]
fn umu_exports_take_candidates_and_the_list_shows_everything() {
    let home = TempHome::new("umu-candidates");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    let stash = r#"{"egs:Promoted":{"title":"Promoted Quest","store":"egs","codename":"Promoted",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-24","last_seen":"2026-08-24",
            "drafted_id":{"id":"umu-promotedquest","basis":"title-slug","collision_checked":"2026-08-24"},
            "umu_promoted":"2026-08-24"},
        "egs:Waiting":{"title":"Waiting Game","store":"egs","codename":"Waiting",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "first_seen":"2026-08-24","last_seen":"2026-08-24",
            "drafted_id":{"id":"umu-waitinggame","basis":"title-slug","collision_checked":"2026-08-24"},
            "fix":{"umu_id":"umu-waitinggame","fixes":[],"checked":"2026-08-24"}},
        "itchio:926077":{"title":"Danger Scavenger","store":"itchio","codename":"926077",
            "umu_id":"","title_source":"lutris-wrapper","confidence":"medium",
            "executable":"/media/Data/Spiele/itchio/danger-scavenger/Danger_Scavenger.x86_64",
            "launcher":"lutris","launcher_name":"Danger Scavenger",
            "launcher_dir":"/media/Data/Spiele/itchio/danger-scavenger",
            "codename_source":"lutris-config","runner":"native",
            "first_seen":"2026-08-23","last_seen":"2026-08-23"}}"#;
    std::fs::write(stash_dir.join("umu-misses.json"), stash).unwrap();

    // The CSV export carries exactly the promoted entry.
    let export = run(&home, &["umu-misses", "--export"]);
    assert!(export.status.success());
    let text = stdout(&export);
    assert!(
        text.contains("Promoted Quest,egs,Promoted,umu-promotedquest,,,"),
        "the promoted entry missed the export:\n{text}"
    );
    assert!(
        !text.contains("Waiting Game,") && !text.contains("Danger Scavenger,"),
        "a non-candidate reached the submission:\n{text}"
    );
    let err = String::from_utf8_lossy(&export.stderr);
    assert!(
        err.contains("Waiting Game") && err.contains("not promoted"),
        "the hold-back must name the missing promotion:\n{err}"
    );
    assert!(
        err.contains("Danger Scavenger") && err.contains("launcher launch"),
        "the launcher launch's hold-back reason is missing:\n{err}"
    );

    // The merge-request text: one game, and it says why the row is there.
    let md = run(&home, &["umu-misses", "--export-md"]);
    assert!(md.status.success());
    let text = stdout(&md);
    assert!(text.contains("# Add 1 game: Promoted Quest"), "{text}");
    assert!(
        text.contains("promoted for submission by the reviewing user 2026-08-24"),
        "{text}"
    );
    assert!(
        !text.contains("Waiting Game,egs") && !text.contains("Danger Scavenger,itchio"),
        "a non-candidate row leaked into the merge request:\n{text}"
    );

    // The review list shows all three, each with its candidacy tag.
    let list = run(&home, &["umu-misses"]);
    assert!(list.status.success());
    let text = stdout(&list);
    for expected in ["Promoted Quest", "Waiting Game", "Danger Scavenger"] {
        assert!(text.contains(expected), "list missing {expected}:\n{text}");
    }
    assert!(text.contains("promoted by you 2026-08-24"), "{text}");
    assert!(text.contains("not a umu candidate"), "{text}");
    assert!(text.contains("launcher launch"), "{text}");
}

// ---- gamedb: the same stash, as pages for the gamebus-gamedb data set.

/// A real build of the data set (`gamedb-build --data gamedb`), trimmed to
/// the tables the setup tool reads. Control is published here; anything else
/// the stash carries is not, which is the whole point of the check.
const FIXTURE_GAMEDB_INDEX: &str = r#"{
  "schema_version": 1,
  "games": [
    {"id": "steam-870780", "title": "Control", "page": "control", "year": null,
     "variant_of": null, "note": null, "steam": 870780, "umu": null}
  ],
  "stores": [
    {"id": "steam-870780", "store": "egs", "codename": "Calluna",
     "edition": null, "exe": null, "seen": "2026-08-15",
     "source": "heroic-config", "confidence": "high"}
  ],
  "aliases": [
    {"alias": "egs-Calluna", "id": "steam-870780"},
    {"alias": "steam-870780", "id": "steam-870780"}
  ],
  "helpers": []
}"#;

/// Two games: Control, which the data set already carries, and Fixture
/// Quest, which nothing upstream knows - one of each, so the listing and
/// the export both have something to hold back and something to write.
const FIXTURE_GAMEDB_STASH: &str = r#"{
    "egs:Calluna":{"title":"Control","store":"egs","codename":"Calluna",
        "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
        "first_seen":"2026-08-06","last_seen":"2026-08-15",
        "drafted_id":{"id":"umu-870780","basis":"steam-sku","collision_checked":"2026-08-22"},
        "fix":{"umu_id":"umu-870780","fixes":[],"checked":"2026-08-22"}},
    "egs:Catnip":{"title":"Fixture Quest","store":"egs","codename":"Catnip",
        "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
        "executable":"Z:\\games\\FixtureQuest\\FixtureQuest.exe",
        "first_seen":"2026-08-06","last_seen":"2026-08-07"}}"#;

/// Stash plus a local index, and the path the environment override points
/// the tool at instead of the network.
fn write_gamedb_fixtures(home: &TempHome) -> PathBuf {
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    std::fs::write(stash_dir.join("umu-misses.json"), FIXTURE_GAMEDB_STASH).unwrap();
    let index = home.path().join("identities.json");
    std::fs::write(&index, FIXTURE_GAMEDB_INDEX).unwrap();
    index
}

#[test]
fn gamedb_lists_what_the_data_set_already_has_and_what_is_ready() {
    let home = TempHome::new("gamedb-list");
    let index = write_gamedb_fixtures(&home);

    let out = run_env(
        &home,
        &["gamedb"],
        &[("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())],
    );
    assert!(out.status.success(), "gamedb exited {:?}", out.status);
    let text = stdout(&out);
    assert!(
        text.contains("Control") && text.contains("in gamebus-gamedb as steam-870780"),
        "the published game is not marked as such:\n{text}"
    );
    assert!(
        text.contains("Fixture Quest") && text.contains("egs/Catnip"),
        "the ready game is missing its identity:\n{text}"
    );
    // The listing says how much the index can be trusted and where an
    // export would go - both are decisions the user has to make.
    assert!(text.contains("gamebus-gamedb index: local file"), "{text}");
    assert!(text.contains("Export directory:"), "{text}");
}

#[test]
fn gamedb_export_writes_a_page_per_ready_game_and_never_twice() {
    let home = TempHome::new("gamedb-export");
    let index = write_gamedb_fixtures(&home);
    let out_dir = home.path().join("checkout");
    let env = [("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())];

    let out = run_env(
        &home,
        &["gamedb", "--export", "--out", out_dir.to_str().unwrap()],
        &env,
    );
    assert!(out.status.success(), "export exited {:?}", out.status);
    let text = stdout(&out);
    assert!(
        text.contains("held back") && text.contains("already in gamebus-gamedb as steam-870780"),
        "the published game was not held back:\n{text}"
    );
    assert!(text.contains("1 page(s) written"), "{text}");
    assert!(
        text.contains("open a pull request at https://github.com/fschaupp/gamebus-gamedb"),
        "the export never says where the page goes:\n{text}"
    );

    // The page itself: named by the title's slug, under games/.
    let page = out_dir.join("games/fixture-quest.toml");
    let toml = std::fs::read_to_string(&page).expect("no page written");
    assert!(
        !out_dir.join("games/control.toml").exists(),
        "a second page for a published game was written"
    );
    for expected in [
        "title = \"Fixture Quest\"",
        "gamedb = \"egs-Catnip\"",
        "[[stores.egs]]",
        "codename = \"Catnip\"",
        "exe = \"FixtureQuest.exe\"",
        "seen = \"2026-08-07\"",
        "source = \"heroic-config\"",
        "confidence = \"high\"",
    ] {
        assert!(
            toml.contains(expected),
            "page missing {expected:?}:\n{toml}"
        );
    }
    // Every line is either blank, a table header, or key = value - the
    // shape a TOML parser needs, checked without pulling one in.
    for line in toml.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            line.starts_with('[') || line.split_once(" = ").is_some(),
            "not a TOML line: {line:?}\n{toml}"
        );
    }

    // A second run must never overwrite: the file on disk may carry a
    // reviewer's edits, and this is the only copy of them.
    let before = std::fs::read(&page).unwrap();
    let out = run_env(
        &home,
        &["gamedb", "--export", "--out", out_dir.to_str().unwrap()],
        &env,
    );
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("already written at") && text.contains("delete it to regenerate"),
        "the second run did not say the page was already there:\n{text}"
    );
    assert!(text.contains("0 page(s) written"), "{text}");
    assert_eq!(
        before,
        std::fs::read(&page).unwrap(),
        "the page was rewritten"
    );
}

#[test]
fn gamedb_export_without_a_destination_refuses_and_names_both_ways_to_give_one() {
    let home = TempHome::new("gamedb-nodest");
    let index = write_gamedb_fixtures(&home);
    let out = run_env(
        &home,
        &["gamedb", "--export"],
        &[("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())],
    );
    assert!(!out.status.success(), "export wrote somewhere unnamed");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--out"), "{err}");
    assert!(err.contains("--documents"), "{err}");
}

#[test]
fn gamedb_export_without_an_index_refuses_and_points_at_the_fetch() {
    let home = TempHome::new("gamedb-noindex");
    write_gamedb_fixtures(&home); // the index exists, but nothing points at it
    let out_dir = home.path().join("checkout");
    let out = run(
        &home,
        &["gamedb", "--export", "--out", out_dir.to_str().unwrap()],
    );
    assert!(
        !out.status.success(),
        "a page was written without checking the data set"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("No gamebus-gamedb index") && err.contains("--fetch"),
        "{err}"
    );
    assert!(
        !out_dir.exists(),
        "the export directory was created before the check"
    );
}

#[test]
fn gamedb_documents_follows_the_users_own_documents_directory() {
    let home = TempHome::new("gamedb-documents");
    let index = write_gamedb_fixtures(&home);
    // The localized name is why xdg's own file has to be read rather than
    // "Documents" assumed.
    std::fs::create_dir_all(home.path().join(".config")).unwrap();
    std::fs::write(
        home.path().join(".config/user-dirs.dirs"),
        "XDG_DESKTOP_DIR=\"$HOME/Schreibtisch\"\nXDG_DOCUMENTS_DIR=\"$HOME/Dokumente\"\n",
    )
    .unwrap();

    let out = run_env(
        &home,
        &["gamedb", "--export", "--documents"],
        &[("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())],
    );
    assert!(out.status.success(), "export exited {:?}", out.status);
    assert!(
        home.path()
            .join("Dokumente/gamebus-gamedb/games/fixture-quest.toml")
            .exists(),
        "the page did not land in the documents directory:\n{}",
        stdout(&out)
    );
}

// ---- gamedb: enhancing a page the data set already publishes.

/// Control's page as gamebus-gamedb publishes it, byte for byte - the file a
/// checkout of the data set has under `games/`.
const PUBLISHED_CONTROL: &str = "title = \"Control\"\n\
     gamedb = \"steam-870780\"\n\
     note = \"\"\"\n\
     No protonfix exists upstream for this game (checked 2026-08-22), so \\\n\
     umu-database does not want it and it therefore has no umu id. The Epic \\\n\
     codename below has nowhere else to go.\"\"\"\n\
     \n\
     [ids]\n\
     steam = 870780\n\
     source = \"steam-sku\"\n\
     seen = \"2026-08-22\"\n\
     \n\
     [[stores.egs]]\n\
     codename = \"Calluna\"\n\
     seen = \"2026-08-15\"\n\
     source = \"heroic-config\"\n\
     confidence = \"high\"\n\
     note = \"Heroic writes the app name capitalized; Lutris matches it case-sensitively.\"\n";

/// The owner's real stash for Control: the Epic entry the published page
/// already knows, a GOG launch it does not, and eight wrapper launches that
/// resolved a title and a top-level executable but no store.
const FIXTURE_ENHANCE_STASH: &str = r#"{
    "egs:Calluna":{"title":"Control","store":"egs","codename":"Calluna",
        "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
        "first_seen":"2026-08-06","last_seen":"2026-08-15",
        "drafted_id":{"id":"umu-870780","basis":"steam-sku","collision_checked":"2026-08-22"}},
    "gog:2049187585":{"title":"Control","store":"gog","codename":"2049187585",
        "umu_id":"umu-0","title_source":"heroic-library","confidence":"medium",
        "first_seen":"2026-08-18","last_seen":"2026-08-18",
        "drafted_id":{"id":"umu-870780","basis":"steam-sku","collision_checked":"2026-08-22"}},
    "wrapper:control":{"title":"Control","store":"none",
        "umu_id":"umu-0","title_source":"detectable","confidence":"high",
        "executable":"Z:\\games\\Control\\Control_DX12.exe",
        "first_seen":"2026-08-07","last_seen":"2026-08-07",
        "drafted_id":{"id":"umu-870780","basis":"steam-sku","collision_checked":"2026-08-22"}}}"#;

/// The enhancement fixtures: that stash, and the local index pointing at
/// Control. Returns the index path the environment override names.
fn write_enhance_fixtures(home: &TempHome) -> PathBuf {
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    std::fs::write(stash_dir.join("umu-misses.json"), FIXTURE_ENHANCE_STASH).unwrap();
    let index = home.path().join("identities.json");
    std::fs::write(&index, FIXTURE_GAMEDB_INDEX).unwrap();
    index
}

/// The expected report line, whichever route the page's text came in by.
const ENHANCED_LINE: &str = "+gog/2049187585, +exe Control_DX12.exe";

/// Gap 1: a game the data set already carries, where this machine knows
/// more. The whole game used to be held back; now the published page is
/// edited in place and everything already in it survives.
#[test]
fn gamedb_export_enhances_a_published_page_in_the_checkout_it_writes_to() {
    let home = TempHome::new("gamedb-enhance-local");
    let index = write_enhance_fixtures(&home);
    let out_dir = home.path().join("checkout");
    std::fs::create_dir_all(out_dir.join("games")).unwrap();
    let page = out_dir.join("games/control.toml");
    std::fs::write(&page, PUBLISHED_CONTROL).unwrap();

    let out = run_env(
        &home,
        &["gamedb", "--export", "--out", out_dir.to_str().unwrap()],
        &[("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())],
    );
    assert!(out.status.success(), "export exited {:?}", out.status);
    let text = stdout(&out);
    assert!(
        text.contains("enhanced") && text.contains(ENHANCED_LINE),
        "the enhancement was not reported:\n{text}"
    );
    assert!(text.contains("1 enhanced"), "{text}");

    let enhanced = std::fs::read_to_string(&page).expect("the page is still there");
    // Everything the page already carried, byte for byte - the reviewer's
    // note above all, which is the only copy of it anywhere.
    assert!(
        enhanced.starts_with(PUBLISHED_CONTROL.trim_end_matches('\n')) || {
            // The root `exe` list lands among the root keys, so the tail of
            // the original moves down rather than staying at offset zero.
            PUBLISHED_CONTROL
                .lines()
                .all(|line| enhanced.contains(line))
        },
        "the published page lost something:\n{enhanced}"
    );
    for expected in [
        "note = \"Heroic writes the app name capitalized; Lutris matches it case-sensitively.\"",
        "exe = [\"Control_DX12.exe\"]",
        "[[stores.gog]]",
        "codename = \"2049187585\"",
        "confidence = \"medium\"",
    ] {
        assert!(
            enhanced.contains(expected),
            "missing {expected:?}:\n{enhanced}"
        );
    }
    // The created root key is a ROOT key: above the first table header, or
    // it would parse as `ids.exe` and the schema would reject the page.
    let exe_at = enhanced.find("exe = [").expect("the list was written");
    assert!(
        exe_at < enhanced.find("\n[ids]").expect("the page has [ids]"),
        "exe landed under a table header:\n{enhanced}"
    );

    // A second run has nothing left to add - the index has not caught up
    // yet, but the page on disk has - and says so rather than appending the
    // same entries again.
    let before = std::fs::read(&page).unwrap();
    let out = run_env(
        &home,
        &["gamedb", "--export", "--out", out_dir.to_str().unwrap()],
        &[("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())],
    );
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("0 enhanced"), "{text}");
    assert_eq!(
        before,
        std::fs::read(&page).unwrap(),
        "the page was rewritten"
    );
}

/// The destination is not a checkout, so the page's text has to come from
/// somewhere else. `GAMEBUS_GAMEDB_PAGES` is that somewhere, and it is what
/// keeps this test - and the real `--fetch-pages` path it stands in for -
/// off the network.
#[test]
fn gamedb_export_fetches_the_page_it_enhances_when_the_destination_has_none() {
    let home = TempHome::new("gamedb-enhance-pages");
    let index = write_enhance_fixtures(&home);
    let pages = home.path().join("published");
    std::fs::create_dir_all(&pages).unwrap();
    std::fs::write(pages.join("control.toml"), PUBLISHED_CONTROL).unwrap();
    let out_dir = home.path().join("elsewhere");

    let out = run_env(
        &home,
        &["gamedb", "--export", "--out", out_dir.to_str().unwrap()],
        &[
            ("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap()),
            ("GAMEBUS_GAMEDB_PAGES", pages.to_str().unwrap()),
        ],
    );
    assert!(out.status.success(), "export exited {:?}", out.status);
    let text = stdout(&out);
    assert!(text.contains(ENHANCED_LINE), "{text}");

    let written = std::fs::read_to_string(out_dir.join("games/control.toml"))
        .expect("the enhanced page was not written");
    assert!(written.contains("[[stores.gog]]"), "{written}");
    assert!(
        written.contains("exe = [\"Control_DX12.exe\"]"),
        "{written}"
    );
    // The source directory is read-only as far as this command is concerned.
    assert_eq!(
        std::fs::read_to_string(pages.join("control.toml")).unwrap(),
        PUBLISHED_CONTROL,
        "the export wrote back into the page directory"
    );
}

/// With no copy of the page anywhere and no permission to fetch one, the
/// export refuses - and says exactly what it would have added, so the choice
/// between the two ways of giving it the page is an informed one.
#[test]
fn gamedb_export_holds_an_enhancement_back_and_names_what_it_would_have_added() {
    let home = TempHome::new("gamedb-enhance-none");
    let index = write_enhance_fixtures(&home);
    let out_dir = home.path().join("empty");

    let out = run_env(
        &home,
        &["gamedb", "--export", "--out", out_dir.to_str().unwrap()],
        &[("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())],
    );
    assert!(out.status.success(), "export exited {:?}", out.status);
    let text = stdout(&out);
    assert!(
        text.contains("held back") && text.contains(ENHANCED_LINE),
        "the hold-back does not say what it would have added:\n{text}"
    );
    assert!(text.contains("--fetch-pages"), "{text}");
    assert!(
        !out_dir.join("games/control.toml").exists(),
        "a page was written from nothing"
    );
}

/// The listing marks an enhancement as neither published-and-done nor ready:
/// a distinct glyph, and the additions spelled out.
#[test]
fn gamedb_listing_marks_an_enhancement_with_its_own_glyph() {
    let home = TempHome::new("gamedb-enhance-list");
    let index = write_enhance_fixtures(&home);
    let out = run_env(
        &home,
        &["gamedb"],
        &[("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())],
    );
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains(&format!(
            "in gamebus-gamedb as steam-870780, enhancement: {ENHANCED_LINE}"
        )),
        "{text}"
    );
    assert!(text.contains("+ Control"), "the glyph is missing:\n{text}");
}

/// The end of the road: an enhanced page has to pass the data set's own
/// lint, run over the whole set it would be committed into.
///
/// The lint is `.scripts/gamedb-lint.py`, stdlib Python and nothing else.
/// Both it and the `gamedb/` submodule are optional here - a checkout
/// without either skips rather than failing, because neither is a build
/// dependency of this crate.
#[test]
fn an_enhanced_page_passes_the_data_sets_own_lint() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let data = repo.join("gamedb");
    let lint = repo.join(".scripts/gamedb-lint.py");
    if !data.join("games/control.toml").exists() || !lint.exists() {
        eprintln!("skipping: the gamedb submodule is not checked out");
        return;
    }
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipping: no python3, and the lint is a Python script");
        return;
    }

    let home = TempHome::new("gamedb-enhance-lint");
    let index = write_enhance_fixtures(&home);
    // The destination IS a copy of the data set, which is what a contributor
    // points --out at: their checkout of gamebus-gamedb.
    let out_dir = home.path().join("gamebus-gamedb");
    copy_tree(&data, &out_dir);

    let out = run_env(
        &home,
        &["gamedb", "--export", "--out", out_dir.to_str().unwrap()],
        &[("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())],
    );
    assert!(out.status.success(), "export exited {:?}", out.status);
    assert!(stdout(&out).contains(ENHANCED_LINE), "{}", stdout(&out));

    let lint = Command::new("python3")
        .arg(&lint)
        .arg(&out_dir)
        .output()
        .expect("python3 runs");
    let report = String::from_utf8_lossy(&lint.stdout);
    assert!(
        lint.status.success() && report.contains("OK:"),
        "the enhanced data set does not lint:\n{report}{}\n\n--- control.toml ---\n{}",
        String::from_utf8_lossy(&lint.stderr),
        std::fs::read_to_string(out_dir.join("games/control.toml")).unwrap_or_default()
    );
}

/// Copy a directory tree, skipping anything git keeps - the submodule's
/// `.git` file above all, which would make the copy a broken checkout.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("destination");
    for entry in std::fs::read_dir(from).expect("readable source").flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(".git") {
            continue;
        }
        let target = to.join(&name);
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy");
        }
    }
}

// ---- gamedb: a launcher launch that never went through umu.

/// Danger Scavenger exactly as the daemon records the live launch: a native
/// itch.io game handed over by Lutris (`umu_id` empty), the codename out of
/// Lutris's own `.lutrisgame.json`, the title from the wrapper argv.
const FIXTURE_LAUNCHER_STASH: &str = r#"{
    "itchio:926077":{"title":"Danger Scavenger","store":"itchio","codename":"926077",
        "umu_id":"","title_source":"lutris-wrapper","confidence":"medium",
        "executable":"/media/Data/Spiele/itchio/danger-scavenger/Danger_Scavenger.x86_64",
        "first_seen":"2026-08-23","last_seen":"2026-08-23",
        "launcher":"lutris","launcher_name":"Danger Scavenger",
        "launcher_dir":"/media/Data/Spiele/itchio/danger-scavenger",
        "codename_source":"lutris-config","runner":"native"}}"#;

/// The page that stash entry must fold into, byte for byte: the Lutris
/// record is the source, the codename makes the confidence, and the native
/// runner lets the `.x86_64` binary onto the page.
const DANGER_SCAVENGER_PAGE: &str = "title = \"Danger Scavenger\"\n\
     gamedb = \"itchio-926077\"\n\
     note = \"Identified from Lutris: service=itchio, appid=926077.\"\n\
     \n\
     [[stores.itchio]]\n\
     codename = \"926077\"\n\
     exe = \"Danger_Scavenger.x86_64\"\n\
     seen = \"2026-08-23\"\n\
     source = \"lutris\"\n\
     confidence = \"high\"\n";

#[test]
fn a_native_lutris_launch_exports_the_danger_scavenger_page_byte_exact() {
    let home = TempHome::new("gamedb-launcher");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    std::fs::write(stash_dir.join("umu-misses.json"), FIXTURE_LAUNCHER_STASH).unwrap();
    let index = home.path().join("identities.json");
    std::fs::write(&index, FIXTURE_GAMEDB_INDEX).unwrap();
    let out_dir = home.path().join("checkout");

    let out = run_env(
        &home,
        &["gamedb", "--export", "--out", out_dir.to_str().unwrap()],
        &[("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())],
    );
    assert!(out.status.success(), "export exited {:?}", out.status);
    assert!(
        stdout(&out).contains("1 page(s) written"),
        "{}",
        stdout(&out)
    );
    let page = std::fs::read_to_string(out_dir.join("games/danger-scavenger.toml"))
        .expect("no page written");
    assert_eq!(page, DANGER_SCAVENGER_PAGE, "the page is not byte-exact");

    // The same entry in the listing: still there by default - gamedb is
    // always active, umu miss or not.
    let out = run_env(
        &home,
        &["gamedb"],
        &[("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())],
    );
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("Danger Scavenger") && text.contains("itchio/926077"),
        "a non-umu launch fell out of the listing:\n{text}"
    );
}

/// The exported page has to pass the data set's own lint, run over a
/// scratch copy of the whole set it would be committed into. Skips cleanly
/// when python3 or the gamedb submodule is absent, exactly like the
/// enhancement lint test above - neither is a build dependency.
#[test]
fn the_danger_scavenger_page_passes_the_data_sets_own_lint() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let data = repo.join("gamedb");
    let lint = repo.join(".scripts/gamedb-lint.py");
    if !data.join("games/control.toml").exists() || !lint.exists() {
        eprintln!("skipping: the gamedb submodule is not checked out");
        return;
    }
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipping: no python3, and the lint is a Python script");
        return;
    }

    let home = TempHome::new("gamedb-launcher-lint");
    let stash_dir = home.path().join(".local/share/gamebus-presenced");
    std::fs::create_dir_all(&stash_dir).unwrap();
    std::fs::write(stash_dir.join("umu-misses.json"), FIXTURE_LAUNCHER_STASH).unwrap();
    let index = home.path().join("identities.json");
    std::fs::write(&index, FIXTURE_GAMEDB_INDEX).unwrap();
    // The destination is a scratch copy of the data set, which is what a
    // contributor points --out at: their checkout of gamebus-gamedb.
    let out_dir = home.path().join("gamebus-gamedb");
    copy_tree(&data, &out_dir);

    let out = run_env(
        &home,
        &["gamedb", "--export", "--out", out_dir.to_str().unwrap()],
        &[("GAMEBUS_GAMEDB_INDEX", index.to_str().unwrap())],
    );
    assert!(out.status.success(), "export exited {:?}", out.status);
    assert!(
        out_dir.join("games/danger-scavenger.toml").exists(),
        "{}",
        stdout(&out)
    );

    let lint = Command::new("python3")
        .arg(&lint)
        .arg(&out_dir)
        .output()
        .expect("python3 runs");
    let report = String::from_utf8_lossy(&lint.stdout);
    assert!(
        lint.status.success() && report.contains("OK:"),
        "the data set with the page does not lint:\n{report}{}",
        String::from_utf8_lossy(&lint.stderr)
    );
}
