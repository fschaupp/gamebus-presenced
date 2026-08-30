//! Lutris's pga.db - the launcher's own library as a codename source for
//! the setup tool.
//!
//! Lutris records every game it manages in a sqlite file, `pga.db`, whose
//! `games` table carries the service (`gog`, `egs`, `itchio`, …) and the
//! service-internal id (`service_id`) the game was installed from - exactly
//! the codename an identity record wants. A native Lutris launch exposes
//! neither over the environment, so the daemon stashes those misses without
//! a codename; this module lets the setup tool fill them from the library
//! afterwards.
//!
//! Read-only, local, setup-tool only: the daemon never links rusqlite and
//! never opens this file. A missing pga.db is an empty library (a machine
//! without Lutris behaves identically); a present-but-unreadable one is an
//! error naming the path - silently showing "no codenames" over a corrupt
//! library would hide real data.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::umu_report::{normalize_store, UmuReport};

/// One game of the Lutris library: the launcher's own title plus the store
/// identity it was installed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LutrisGame {
    /// Lutris's display name for the game - what `GAME_NAME` carries at
    /// launch, so it matches a stashed `launcher_name` exactly.
    pub name: String,
    /// Lutris's install slug (`danger-scavenger`).
    pub slug: String,
    /// umu-database store id, normalised via
    /// [`normalize_store`](crate::umu_report::normalize_store) - never a raw
    /// Lutris service spelling.
    pub store: String,
    /// The store-internal id (`service_id`): the GOG product id, the EGS App
    /// Name, the itch.io game id.
    pub codename: String,
    /// Lutris's runner for the game (`linux`, `wine`), when recorded.
    pub runner: Option<String>,
    /// Lutris's install directory for the game, when recorded.
    pub directory: Option<String>,
}

/// The pga.db files worth reading: `$XDG_DATA_HOME/lutris/pga.db` and the
/// flatpak's copy, existing ones only. `GAMEBUS_LUTRIS_DB` overrides both
/// with one named file - exclusively, so a test (or a user pointing at a
/// backup) reads exactly that library and never the machine's own.
pub fn library_paths() -> Vec<PathBuf> {
    if let Some(over) = std::env::var_os("GAMEBUS_LUTRIS_DB") {
        return vec![PathBuf::from(over)];
    }
    let mut paths = Vec::new();
    if let Some(data) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    {
        paths.push(data.join("lutris/pga.db"));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        paths.push(home.join(".var/app/net.lutris.Lutris/data/lutris/pga.db"));
    }
    paths.retain(|p| p.is_file());
    paths
}

/// Load every library [`library_paths`] names, merged. A missing file is an
/// empty library, not an error; a file that exists but cannot be read as a
/// Lutris database is an `Err` naming its path. Rows without a store
/// identity are filtered in the query; rows whose service normalises to
/// `none` (`flathub` and friends - distribution channels, not stores) are
/// skipped here.
pub fn load() -> Result<Vec<LutrisGame>, String> {
    let mut games = Vec::new();
    for path in library_paths() {
        if !path.is_file() {
            continue;
        }
        games.extend(read_db(&path)?);
    }
    Ok(games)
}

/// One pga.db, read-only.
fn read_db(path: &Path) -> Result<Vec<LutrisGame>, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut stmt = conn
        .prepare(
            "select name, slug, runner, service, service_id, directory from games \
             where service is not null and service != '' \
             and service_id is not null and service_id != ''",
        )
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut games = Vec::new();
    for row in rows {
        let (name, slug, runner, service, service_id, directory) =
            row.map_err(|e| format!("{}: {e}", path.display()))?;
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            continue; // a row Lutris never finished naming helps nobody
        };
        let store = normalize_store(&service);
        if store == "none" {
            continue;
        }
        games.push(LutrisGame {
            name,
            slug: slug.unwrap_or_default(),
            store: store.to_string(),
            codename: service_id,
            runner: runner.filter(|r| !r.is_empty()),
            directory: directory.filter(|d| !d.is_empty()),
        });
    }
    Ok(games)
}

/// The one library row for a (name, store) pair: exact, case-sensitive name
/// match - a stashed `launcher_name` IS the pga.db name, so anything fuzzier
/// only invites the wrong codename - with the store normalised on the way
/// in.
pub fn lookup<'a>(games: &'a [LutrisGame], name: &str, store: &str) -> Option<&'a LutrisGame> {
    let store = normalize_store(store);
    games.iter().find(|g| g.name == name && g.store == store)
}

/// Every library row matching a title case-insensitively, all stores - the
/// pick list for a human choosing among a game's editions.
pub fn candidates<'a>(games: &'a [LutrisGame], title: &str) -> Vec<&'a LutrisGame> {
    let t = title.trim().to_lowercase();
    if t.is_empty() {
        return Vec::new();
    }
    games
        .iter()
        .filter(|g| g.name.to_lowercase() == t)
        .collect()
}

/// Fill missing codenames from the library: every Lutris-launched entry
/// with a `launcher_name` but no codename from any side gets the library's
/// `service_id` for (launcher_name, effective store) - written as the
/// annotation-half `codename_override`, because that is the half this tool
/// owns. Writing `codename`/`codename_source` here would not survive: both
/// are resolution-half, and every persist (ADR-009) adopts the resolution
/// half from the daemon's side, flattening a setup-tool write on the next
/// merge. `codename_source = "lutris-library"` therefore stays daemon
/// vocabulary; the returned report lines are what names the source to the
/// user.
///
/// Marks the stash dirty via [`UmuReport::update`] but does not write  -
/// callers batch and [`UmuReport::save`] once. Returns one line per fill,
/// sorted, e.g. `Control: codename 2049187585 from Lutris (gog)`.
pub fn fill_codenames(report: &mut UmuReport, games: &[LutrisGame]) -> Vec<String> {
    // key, launcher name, codename, store - collected first because the
    // stash cannot be iterated and updated at once.
    let mut hits: Vec<(String, String, String, String)> = Vec::new();
    for (key, miss) in report.entries() {
        // Entries written before the launcher facts existed carry no
        // `launcher` field but still wear the lutris key prefix; the
        // library's knowledge applies to them just the same.
        let is_lutris = miss.launcher.as_deref() == Some("lutris") || key.starts_with("lutris:");
        if !is_lutris {
            continue;
        }
        let has_codename = miss.codename.as_deref().is_some_and(|c| !c.is_empty())
            || miss
                .codename_override
                .as_deref()
                .is_some_and(|c| !c.is_empty());
        if has_codename {
            continue;
        }
        // The launcher's own name for the game where recorded; the resolved
        // title otherwise (pre-widening entries) - Lutris names match the
        // curated titles for every game measured so far.
        let Some(name) = miss
            .launcher_name
            .as_deref()
            .or_else(|| miss.effective_title())
            .filter(|n| !n.is_empty())
        else {
            continue;
        };
        if let Some(game) = lookup(games, name, miss.effective_store()) {
            hits.push((
                key.clone(),
                name.to_string(),
                game.codename.clone(),
                game.store.clone(),
            ));
        }
    }
    hits.sort();
    let mut lines = Vec::new();
    for (key, name, codename, store) in hits {
        let code = codename.clone();
        report.update(&key, move |m| {
            m.codename_override = Some(code);
            m.codename_override_source = Some("lutris-library".into());
        });
        lines.push(format!("{name}: codename {codename} from Lutris ({store})"));
    }
    lines
}

/// `GAMEBUS_LUTRIS_DB` is process-global; tests that set it take this lock
/// so parallel test threads never read each other's override. The
/// pick-flow integration test in `umu_misses::tui` takes it too, for the
/// same reason.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::ENV_LOCK;
    use super::*;
    use crate::umu_report::LaunchFacts;

    /// One scratch row: (name, slug, runner, service, service_id, directory).
    type Row<'a> = (
        &'a str,
        &'a str,
        Option<&'a str>,
        &'a str,
        &'a str,
        Option<&'a str>,
    );

    /// A scratch pga.db with the `games` columns the loader reads (the real
    /// table has more; `select` by name ignores them).
    fn write_db(path: &Path, rows: &[Row]) {
        let conn = Connection::open(path).expect("create scratch db");
        conn.execute_batch(
            "create table games (
                id integer primary key,
                name text, slug text, runner text,
                service text, service_id text, directory text
             )",
        )
        .expect("create games table");
        for (name, slug, runner, service, service_id, directory) in rows {
            conn.execute(
                "insert into games (name, slug, runner, service, service_id, directory)
                 values (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![name, slug, runner, service, service_id, directory],
            )
            .expect("insert row");
        }
    }

    fn scratch(file: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gamebus-lutris-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join(file)
    }

    /// The live machine's pga.db service rows (fixture, 2026-08-24) plus the
    /// two shapes the loader must drop: a flathub row (a channel, not a
    /// store) and a row with an empty service_id (no identity to offer).
    fn fixture_db(path: &Path) {
        write_db(
            path,
            &[
                ("Control", "control", Some("wine"), "egs", "Calluna", None),
                (
                    "Control",
                    "control",
                    Some("wine"),
                    "gog",
                    "2049187585",
                    None,
                ),
                (
                    "Call of Duty: Black Ops Cold War",
                    "call-of-duty-black-ops-cold-war",
                    Some("wine"),
                    "battlenet",
                    "zeus",
                    None,
                ),
                (
                    "Amnesia: The Bunker",
                    "amnesia-the-bunker",
                    Some("wine"),
                    "gog",
                    "1186009992",
                    None,
                ),
                (
                    "Danger Scavenger",
                    "danger-scavenger",
                    Some("linux"),
                    "itchio",
                    "926077",
                    Some("/media/Data/Spiele/itchio/danger-scavenger"),
                ),
                (
                    "Fixture Channel Game",
                    "fixture-channel-game",
                    Some("linux"),
                    "flathub",
                    "org.fixture.Game",
                    None,
                ),
                (
                    "Fixture No Id",
                    "fixture-no-id",
                    Some("wine"),
                    "gog",
                    "",
                    None,
                ),
            ],
        );
    }

    fn load_via_env(path: &Path) -> Result<Vec<LutrisGame>, String> {
        std::env::set_var("GAMEBUS_LUTRIS_DB", path);
        let out = load();
        std::env::remove_var("GAMEBUS_LUTRIS_DB");
        out
    }

    #[test]
    fn load_reads_the_library_and_skips_non_store_rows() {
        let _env = ENV_LOCK.lock().unwrap();
        let path = scratch("pga.db");
        fixture_db(&path);
        let games = load_via_env(&path).expect("fixture db loads");
        // Five store rows survive; flathub and the empty service_id do not.
        assert_eq!(games.len(), 5);
        assert!(games.iter().all(|g| g.store != "none"));
        let danger = games
            .iter()
            .find(|g| g.name == "Danger Scavenger")
            .expect("itchio row");
        assert_eq!(danger.store, "itchio");
        assert_eq!(danger.codename, "926077");
        assert_eq!(danger.runner.as_deref(), Some("linux"));
        assert_eq!(
            danger.directory.as_deref(),
            Some("/media/Data/Spiele/itchio/danger-scavenger")
        );
        assert_eq!(danger.slug, "danger-scavenger");
    }

    #[test]
    fn load_normalises_lutris_service_spellings() {
        let _env = ENV_LOCK.lock().unwrap();
        let path = scratch("spellings.db");
        write_db(
            &path,
            &[
                (
                    "Fixture EA Game",
                    "fixture-ea",
                    None,
                    "ea_app",
                    "fixture.ea",
                    None,
                ),
                (
                    "Fixture Steam Game",
                    "fixture-steam",
                    None,
                    "steamwindows",
                    "480",
                    None,
                ),
            ],
        );
        let games = load_via_env(&path).expect("spellings db loads");
        let stores: Vec<&str> = games.iter().map(|g| g.store.as_str()).collect();
        assert_eq!(stores, vec!["ea", "steam"]);
    }

    #[test]
    fn a_missing_library_is_empty_and_a_corrupt_one_errs_with_its_path() {
        let _env = ENV_LOCK.lock().unwrap();
        let missing = scratch("does-not-exist.db");
        assert_eq!(
            load_via_env(&missing).expect("missing file is fine"),
            vec![]
        );

        let corrupt = scratch("corrupt.db");
        std::fs::write(&corrupt, "this is not a sqlite database at all").unwrap();
        let err = load_via_env(&corrupt).expect_err("corrupt file must err");
        assert!(
            err.contains("corrupt.db"),
            "error must name the path: {err}"
        );
    }

    #[test]
    fn lookup_is_exact_on_name_and_normalised_on_store() {
        let _env = ENV_LOCK.lock().unwrap();
        let path = scratch("lookup.db");
        fixture_db(&path);
        let games = load_via_env(&path).unwrap();

        assert_eq!(
            lookup(&games, "Control", "gog").unwrap().codename,
            "2049187585"
        );
        assert_eq!(
            lookup(&games, "Control", "egs").unwrap().codename,
            "Calluna"
        );
        // The library has no battlenet Control; nothing fuzzy fills in.
        assert!(lookup(&games, "Control", "battlenet").is_none());
        // Exact means case-sensitive: launcher_name IS the pga.db name.
        assert!(lookup(&games, "control", "gog").is_none());
    }

    #[test]
    fn candidates_match_names_case_insensitively_across_stores() {
        let _env = ENV_LOCK.lock().unwrap();
        let path = scratch("candidates.db");
        fixture_db(&path);
        let games = load_via_env(&path).unwrap();

        let picks = candidates(&games, "control");
        let codenames: Vec<&str> = picks.iter().map(|g| g.codename.as_str()).collect();
        assert_eq!(codenames, vec!["Calluna", "2049187585"]);
        assert!(candidates(&games, "Half-Life").is_empty());
        assert!(candidates(&games, "  ").is_empty());
    }

    #[test]
    fn a_pre_widening_entry_with_a_lutris_key_still_gets_filled() {
        let _env = ENV_LOCK.lock().unwrap();
        let path = scratch("prewiden.db");
        fixture_db(&path);
        let games = load_via_env(&path).unwrap();

        // The shape the daemon wrote before the widening: umu miss, no
        // launcher facts at all, per-launch uuid key; the title came from
        // the resolver and the user corrected the store to gog.
        let mut report = UmuReport::default();
        report.note_launch("none", None, "umu-default", "lutris:7181c27b-uuid");
        report.update("lutris:7181c27b-uuid", |m| {
            m.title = Some("Control".into());
            m.store_override = Some("gog".into());
        });
        let lines = fill_codenames(&mut report, &games);
        assert_eq!(lines, ["Control: codename 2049187585 from Lutris (gog)"]);
        let m = &report.entries()["lutris:7181c27b-uuid"];
        assert_eq!(m.codename_override.as_deref(), Some("2049187585"));
        assert_eq!(
            m.codename_override_source.as_deref(),
            Some("lutris-library")
        );
    }

    #[test]
    fn fill_codenames_writes_the_override_half_and_reports_each_fill() {
        let _env = ENV_LOCK.lock().unwrap();
        let path = scratch("fill.db");
        fixture_db(&path);
        let games = load_via_env(&path).unwrap();

        // A pathless stash: update() marks dirty, nothing hits disk.
        let mut report = UmuReport::default();
        let lutris = |name: &str| LaunchFacts {
            launcher: Some("lutris".into()),
            launcher_name: Some(name.into()),
            ..LaunchFacts::default()
        };
        // The daemon guessed no store; the user corrected it to gog.
        report.note_launch_with("none", None, "", "lutris:control", lutris("Control"));
        report.update("lutris:control", |m| m.store_override = Some("gog".into()));
        // Already has a daemon-observed codename: must be left alone.
        report.note_launch_with(
            "itchio",
            Some("926077"),
            "",
            "lutris:danger-scavenger",
            lutris("Danger Scavenger"),
        );
        // Not in the library under this store: no fill, no line.
        report.note_launch_with(
            "battlenet",
            None,
            "",
            "lutris:fixture",
            lutris("Fixture Quest"),
        );

        let lines = fill_codenames(&mut report, &games);
        assert_eq!(
            lines,
            vec!["Control: codename 2049187585 from Lutris (gog)"]
        );

        let control = &report.entries()["lutris:control"];
        assert_eq!(control.codename_override.as_deref(), Some("2049187585"));
        assert_eq!(control.codename, None, "resolution half stays the daemon's");
        assert_eq!(control.effective_codename(), Some("2049187585"));

        let danger = &report.entries()["itchio:926077"];
        assert_eq!(danger.codename.as_deref(), Some("926077"));
        assert_eq!(danger.codename_override, None);

        // Idempotent: the override now counts as a codename.
        assert!(fill_codenames(&mut report, &games).is_empty());
    }
}
