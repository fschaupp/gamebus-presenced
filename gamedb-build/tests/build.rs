//! The builder: the artifacts it writes, and the promises they make.
//!
//! Two claims carry most of the weight here. **Determinism**: the same pages
//! build to the same bytes, which is what makes a checksum in
//! `manifest.toml` worth comparing and lets a client do a cheap ETag check.
//! And **agreement**: JSON, Parquet and SQLite are three views of one shape,
//! so any disagreement between them is a bug in the builder rather than a
//! choice a consumer has to know about.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use arrow_array::{Array, Int32Array, Int64Array, ListArray, StringArray, StructArray};
use gamedb_build::build::{self, Options, Tables, ARTIFACTS, MANIFEST};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

fn options(data: &Path, out: &Path, commit: &str) -> Options {
    Options {
        data: data.to_path_buf(),
        out: out.to_path_buf(),
        commit: commit.to_string(),
        skip_lint: false,
        schema_dir: Some(common::schema_dir()),
    }
}

fn build_into(data: &Path, out: &Path, commit: &str) -> build::Outcome {
    build::run(&options(data, out, commit)).expect("the build succeeds")
}

#[test]
fn it_writes_exactly_the_five_files_the_release_publishes() {
    let scratch = common::Scratch::new("release");
    build_into(&common::data_set(), scratch.path(), "deadbeef");

    let mut written: Vec<String> = std::fs::read_dir(scratch.path())
        .expect("the output directory exists")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();

    let mut expected: Vec<String> = ARTIFACTS.iter().map(|s| s.to_string()).collect();
    expected.push(MANIFEST.to_string());
    expected.sort();

    assert_eq!(written, expected);
    for name in &written {
        let size = std::fs::metadata(scratch.join(name))
            .expect("metadata")
            .len();
        assert!(size > 0, "{name} is empty");
    }
}

#[test]
fn two_builds_of_the_same_data_are_byte_identical() {
    let first = common::Scratch::new("determinism-a");
    let second = common::Scratch::new("determinism-b");
    build_into(&common::data_set(), first.path(), "same-commit");
    build_into(&common::data_set(), second.path(), "same-commit");

    for name in ARTIFACTS.iter().chain(std::iter::once(&MANIFEST)) {
        let a = std::fs::read(first.join(name)).expect("first build");
        let b = std::fs::read(second.join(name)).expect("second build");
        assert_eq!(
            a, b,
            "{name} is not reproducible - something in the build depends on \
             more than the data"
        );
    }
}

#[test]
fn only_the_manifest_moves_when_the_commit_does() {
    // The artifacts are a function of the data alone. If a commit reached one
    // of them, the same pages would checksum differently every merge.
    let first = common::Scratch::new("commit-a");
    let second = common::Scratch::new("commit-b");
    build_into(&common::data_set(), first.path(), "1111111");
    build_into(&common::data_set(), second.path(), "2222222");

    for name in ARTIFACTS {
        assert_eq!(
            std::fs::read(first.join(name)).expect("first"),
            std::fs::read(second.join(name)).expect("second"),
            "{name} changed with the source commit"
        );
    }
    assert_ne!(
        std::fs::read(first.join(MANIFEST)).expect("first"),
        std::fs::read(second.join(MANIFEST)).expect("second"),
    );
}

#[test]
fn shared_helpers_matches_the_list_the_daemon_bundles() {
    // `gamedb/helpers.toml` is the source; `shared-helpers.txt` at the repo
    // root is what src/naming.rs compiles in. Flattening the first has to
    // reproduce the second exactly for the current data, or the daemon and
    // the published artifact would protect against different executables.
    let scratch = common::Scratch::new("helpers");
    build_into(&common::data_set(), scratch.path(), "deadbeef");
    let built = std::fs::read_to_string(scratch.join("shared-helpers.txt")).expect("built list");

    // The daemon's own reading of the bundled file: trim, skip blanks and
    // comments, lowercase. See parse_shared_helpers in src/naming.rs.
    let bundled = std::fs::read_to_string(common::repo().join("shared-helpers.txt"))
        .expect("the bundled list is readable");
    let mut entries: Vec<String> = bundled
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_lowercase)
        .collect();
    entries.sort();
    let flattened = entries.iter().map(|e| format!("{e}\n")).collect::<String>();

    assert_eq!(
        built, flattened,
        "the flattened helpers.toml no longer matches shared-helpers.txt at the repo \
         root. One of the two has gained an entry the other has not."
    );
    assert!(built.contains("unitycrashhandler64.exe"));
}

#[test]
fn the_manifest_checksums_the_files_it_names() {
    let scratch = common::Scratch::new("manifest");
    let outcome = build_into(&common::data_set(), scratch.path(), "0badc0de");

    let manifest: toml::Value = std::fs::read_to_string(scratch.join(MANIFEST))
        .expect("manifest")
        .parse()
        .expect("the manifest is valid TOML");

    assert_eq!(manifest["source_commit"].as_str(), Some("0badc0de"));
    let listed = manifest["artifact"].as_array().expect("artifact array");
    assert_eq!(listed.len(), ARTIFACTS.len());

    for (entry, name) in listed.iter().zip(ARTIFACTS) {
        assert_eq!(entry["name"].as_str(), Some(name), "manifest order");
        let bytes = std::fs::read(scratch.join(name)).expect("artifact");
        let digest = Sha256::digest(&bytes);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            entry["sha256"].as_str(),
            Some(hex.as_str()),
            "{name} digest"
        );
        assert_eq!(entry["bytes"].as_integer(), Some(bytes.len() as i64));
    }

    // The manifest never checksums itself.
    assert!(!listed.iter().any(|e| e["name"].as_str() == Some(MANIFEST)));
    assert_eq!(outcome.artifacts.len(), ARTIFACTS.len());
}

#[test]
fn the_manifest_says_unknown_when_nothing_told_it_the_commit() {
    let scratch = common::Scratch::new("unknown-commit");
    build_into(&common::data_set(), scratch.path(), "unknown");
    let manifest: toml::Value = std::fs::read_to_string(scratch.join(MANIFEST))
        .expect("manifest")
        .parse()
        .expect("valid TOML");
    assert_eq!(manifest["source_commit"].as_str(), Some("unknown"));
}

// ---------------------------------------------------------------------------
// The relational shape
// ---------------------------------------------------------------------------

fn wide_tables() -> Tables {
    build::tables_from(&common::fixture("wide")).expect("the fixture set folds")
}

#[test]
fn the_fixture_set_this_crate_writes_for_itself_validates() {
    let report =
        gamedb_build::lint::run(&common::fixture("wide"), None).expect("the fixture set loads");
    assert!(report.ok(), "{}", report.render());
}

#[test]
fn every_identifier_a_page_carries_becomes_an_alias() {
    let tables = wide_tables();
    let by_alias: BTreeMap<&str, &str> = tables
        .aliases
        .iter()
        .map(|a| (a.alias.as_str(), a.id.as_str()))
        .collect();

    for (alias, id) in [
        // The canonical id resolves to itself.
        ("umu-1450", "umu-1450"),
        // Both editions on the store, and the store the lint's precedence
        // never consults, because Steam identity comes from [ids].
        ("egs-AuroraApp", "umu-1450"),
        ("egs-AuroraDeluxe", "umu-1450"),
        ("steam-1450", "umu-1450"),
        // An executable recorded against one store copy, by basename, folded.
        ("exe:aurora.exe", "umu-1450"),
        // The remaster is its own page and points home separately.
        ("gog-1207000001", "gog-1207000001"),
        ("exe:auroraremastered.exe", "gog-1207000001"),
        // A minted id, its executables, and everything it absorbed. Absorbed
        // ids stay reserved forever, so an old lookup still resolves.
        ("gamedb-b3ac0n77", "gamedb-b3ac0n77"),
        ("exe:beacon.exe", "gamedb-b3ac0n77"),
        ("exe:beacon64.exe", "gamedb-b3ac0n77"),
        ("egs-BeaconOld", "gamedb-b3ac0n77"),
        ("gamedb-oldbeac0", "gamedb-b3ac0n77"),
    ] {
        assert_eq!(
            by_alias.get(alias),
            Some(&id),
            "'{alias}' should resolve to '{id}'"
        );
    }
    assert_eq!(by_alias.len(), tables.aliases.len(), "aliases are unique");
}

#[test]
fn rows_are_sorted_by_the_key_the_shape_declares() {
    let tables = wide_tables();

    let ids: Vec<&str> = tables.games.iter().map(|g| g.id.as_str()).collect();
    assert_eq!(ids, ["gamedb-b3ac0n77", "gog-1207000001", "umu-1450"]);

    let store_keys: Vec<(&str, &str, &str)> = tables
        .stores
        .iter()
        .map(|s| (s.id.as_str(), s.store.as_str(), s.codename.as_str()))
        .collect();
    let mut sorted = store_keys.clone();
    sorted.sort();
    assert_eq!(
        store_keys, sorted,
        "stores sort by id, then store, then codename"
    );

    let aliases: Vec<&str> = tables.aliases.iter().map(|a| a.alias.as_str()).collect();
    let mut sorted = aliases.clone();
    sorted.sort();
    assert_eq!(aliases, sorted);

    let helpers: Vec<&str> = tables.helpers.iter().map(|h| h.exe.as_str()).collect();
    assert_eq!(helpers, ["anticheatservice.exe", "testcrashhandler.exe"]);
}

#[test]
fn optional_columns_carry_what_the_page_wrote_and_nothing_else() {
    let tables = wide_tables();
    let aurora = tables
        .games
        .iter()
        .find(|g| g.id == "umu-1450")
        .expect("Aurora");
    assert_eq!(aurora.year, Some(2019));
    assert_eq!(aurora.umu.as_deref(), Some("umu-1450"));
    assert_eq!(aurora.steam, None);
    assert_eq!(aurora.variant_of, None);

    let remaster = tables
        .games
        .iter()
        .find(|g| g.id == "gog-1207000001")
        .expect("the remaster");
    assert_eq!(remaster.page, "aurora-remastered");
    assert_eq!(remaster.variant_of.as_deref(), Some("umu-1450"));
    assert_eq!(remaster.year, None);
    assert_eq!(remaster.note, None);

    let editions: Vec<Option<&str>> = tables
        .stores
        .iter()
        .filter(|s| s.store == "egs")
        .map(|s| s.edition.as_deref())
        .collect();
    assert_eq!(editions, [Some("Standard"), Some("Deluxe")]);

    let beacon_helper = tables
        .helpers
        .iter()
        .find(|h| h.exe == "testcrashhandler.exe")
        .expect("helper");
    assert_eq!(beacon_helper.incident, None);
}

// ---------------------------------------------------------------------------
// The three views agree
// ---------------------------------------------------------------------------

#[test]
fn the_json_is_the_shape_and_nothing_more() {
    let scratch = common::Scratch::new("json");
    let outcome = build_into(&common::fixture("wide"), scratch.path(), "deadbeef");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(scratch.join("identities.json")).unwrap())
            .expect("valid JSON");

    let keys: Vec<&str> = json
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["schema_version", "games", "stores", "aliases", "helpers"]
    );

    let column_order = |rows: &serde_json::Value| -> Vec<String> {
        rows[0].as_object().expect("row").keys().cloned().collect()
    };
    assert_eq!(
        column_order(&json["games"]),
        [
            "id",
            "title",
            "page",
            "year",
            "variant_of",
            "note",
            "steam",
            "umu"
        ]
    );
    assert_eq!(
        column_order(&json["stores"]),
        [
            "id",
            "store",
            "codename",
            "edition",
            "exe",
            "seen",
            "source",
            "confidence"
        ]
    );
    assert_eq!(column_order(&json["aliases"]), ["alias", "id"]);
    assert_eq!(
        column_order(&json["helpers"]),
        ["exe", "reason", "incident", "seen"]
    );

    assert_eq!(
        json["games"].as_array().unwrap().len(),
        outcome.tables.games.len()
    );
    assert_eq!(
        json["aliases"].as_array().unwrap().len(),
        outcome.tables.aliases.len()
    );
}

#[test]
fn the_sqlite_holds_the_same_rows_as_the_json() {
    let scratch = common::Scratch::new("sqlite");
    let outcome = build_into(&common::fixture("wide"), scratch.path(), "deadbeef");
    let db = Connection::open(scratch.join("identities.sqlite")).expect("open");

    let count = |table: &str| -> i64 {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .expect("count")
    };
    assert_eq!(count("games") as usize, outcome.tables.games.len());
    assert_eq!(count("stores") as usize, outcome.tables.stores.len());
    assert_eq!(count("aliases") as usize, outcome.tables.aliases.len());
    assert_eq!(count("helpers") as usize, outcome.tables.helpers.len());

    // The lookup a client actually does: any slug it has, to one page.
    let id: String = db
        .query_row(
            "SELECT id FROM aliases WHERE alias = ?1",
            ["egs-AuroraDeluxe"],
            |r| r.get(0),
        )
        .expect("alias resolves");
    assert_eq!(id, "umu-1450");

    let (title, page, year, umu): (String, String, Option<i64>, Option<String>) = db
        .query_row(
            "SELECT title, page, year, umu FROM games WHERE id = ?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .expect("game row");
    assert_eq!(title, "Aurora");
    // The file the row was folded from, so a tool holding the id can name
    // the page to edit without re-deriving a slug from the title.
    assert_eq!(page, "aurora");
    assert_eq!(year, Some(2019));
    assert_eq!(umu.as_deref(), Some("umu-1450"));

    // NULL really is NULL, not the empty string.
    let missing: Option<String> = db
        .query_row("SELECT variant_of FROM games WHERE id = ?1", [&id], |r| {
            r.get(0)
        })
        .expect("query");
    assert_eq!(missing, None);

    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("user_version");
    assert_eq!(version, gamedb_build::SCHEMA_VERSION);
}

#[test]
fn the_sqlite_indexes_the_two_lookups_that_matter() {
    let scratch = common::Scratch::new("indexes");
    build_into(&common::fixture("wide"), scratch.path(), "deadbeef");
    let db = Connection::open(scratch.join("identities.sqlite")).expect("open");

    let plan = |sql: &str| -> String {
        let mut stmt = db
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("prepare");
        let rows: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(3))
            .expect("plan")
            .map(|r| r.expect("row"))
            .collect();
        rows.join("; ")
    };

    let aliases = plan("SELECT id FROM aliases WHERE alias = 'x'");
    assert!(
        aliases.contains("USING INDEX") || aliases.contains("USING COVERING INDEX"),
        "aliases(alias) is not indexed: {aliases}"
    );
    let stores = plan("SELECT id FROM stores WHERE store = 'egs' AND codename = 'x'");
    assert!(
        stores.contains("stores_store_codename"),
        "stores(store, codename) is not indexed: {stores}"
    );
}

#[test]
fn the_parquet_round_trips_through_the_crate_that_wrote_it() {
    let scratch = common::Scratch::new("parquet");
    let outcome = build_into(&common::fixture("wide"), scratch.path(), "deadbeef");
    let batches = gamedb_build::parquet_out::read(&scratch.join("identities.parquet"))
        .expect("the file reads back");
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(rows, outcome.tables.games.len());

    let batch = &batches[0];
    let column = |name: &str| {
        batch
            .column(batch.schema().index_of(name).expect(name))
            .clone()
    };

    let ids = column("id");
    let ids = ids
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("id is utf8");
    let observed: Vec<&str> = (0..ids.len()).map(|i| ids.value(i)).collect();
    let expected: Vec<&str> = outcome.tables.games.iter().map(|g| g.id.as_str()).collect();
    assert_eq!(observed, expected, "row order survives the round trip");

    // NULLs stay NULL rather than becoming a default.
    let years = column("year");
    let years = years
        .as_any()
        .downcast_ref::<Int32Array>()
        .expect("year is int32");
    let steam = column("steam");
    let steam = steam
        .as_any()
        .downcast_ref::<Int64Array>()
        .expect("steam is int64");
    for (i, game) in outcome.tables.games.iter().enumerate() {
        assert_eq!(years.is_null(i), game.year.is_none(), "{} year", game.id);
        assert_eq!(steam.is_null(i), game.steam.is_none(), "{} steam", game.id);
    }

    // Unnesting the two nested columns reproduces the flat tables exactly.
    let stores = column("stores");
    let stores = stores
        .as_any()
        .downcast_ref::<ListArray>()
        .expect("stores is a list");
    let mut flattened = Vec::new();
    for (i, id) in observed.iter().enumerate() {
        let entries = stores.value(i);
        let entries = entries
            .as_any()
            .downcast_ref::<StructArray>()
            .expect("struct");
        let field = |name: &str| {
            entries
                .column_by_name(name)
                .expect(name)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("utf8")
                .clone()
        };
        let (store, codename) = (field("store"), field("codename"));
        for j in 0..entries.len() {
            flattened.push((
                (*id).to_string(),
                store.value(j).to_string(),
                codename.value(j).to_string(),
            ));
        }
    }
    let flat: Vec<(String, String, String)> = outcome
        .tables
        .stores
        .iter()
        .map(|s| (s.id.clone(), s.store.clone(), s.codename.clone()))
        .collect();
    assert_eq!(flattened, flat);

    let aliases = column("aliases");
    let aliases = aliases
        .as_any()
        .downcast_ref::<ListArray>()
        .expect("aliases is a list");
    let total: usize = (0..aliases.len()).map(|i| aliases.value(i).len()).sum();
    assert_eq!(total, outcome.tables.aliases.len());
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

#[test]
fn a_data_set_that_does_not_validate_builds_nothing() {
    let scratch = common::Scratch::new("gate");
    // The lint fixture set is deliberately broken; publishing a condensed
    // form of it would ship a contradiction.
    let error = build::run(&Options {
        data: common::lint_fixtures(),
        out: scratch.join("out"),
        commit: "deadbeef".into(),
        skip_lint: false,
        schema_dir: Some(common::schema_dir()),
    })
    .expect_err("a broken data set must not build");
    let message = error.to_string();
    assert!(message.contains("does not validate"), "{message}");
    assert!(
        message.contains("the same game cannot live on two pages"),
        "{message}"
    );
    assert!(!scratch.join("out").join("identities.json").exists());
}
