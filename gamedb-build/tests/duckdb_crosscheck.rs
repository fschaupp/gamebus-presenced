//! Read `identities.parquet` back with somebody else's Parquet implementation.
//!
//! The file is written by the pure-Rust `parquet` crate, and the reason the
//! Parquet artifact exists at all is that a consumer can query it with DuckDB
//! over HTTP without downloading it. A round trip through our own writer and
//! reader cannot catch a shared misunderstanding of the format, so the
//! ecosystem's own implementation is asked to agree.
//!
//! **DuckDB is a reference reader and nothing else.** It is not a build
//! dependency, not a test dependency, and nothing in the release pipeline
//! installs it - the whole point of the builder being pure Rust plus bundled
//! C is that a clean machine needs `cargo` and no system package. So this
//! test skips, loudly but successfully, when no binary is there.
//!
//! Set `DUCKDB_BIN` to point at one somewhere else.

mod common;

use std::path::PathBuf;
use std::process::Command;

use gamedb_build::build::{self, Options};
use serde_json::{json, Value};

/// Where to find a DuckDB CLI, in order of preference. `None` means skip.
fn duckdb() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("DUCKDB_BIN") {
        let path = PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }
    let installed =
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".duckdb/cli/latest/duckdb");
    if installed.is_file() {
        return Some(installed);
    }
    // Anything on PATH, checked by actually running it.
    Command::new("duckdb")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| PathBuf::from("duckdb"))
}

fn query(binary: &PathBuf, sql: &str) -> Value {
    let out = Command::new(binary)
        .args(["-json", "-c", sql])
        .output()
        .expect("duckdb runs");
    assert!(
        out.status.success(),
        "duckdb failed on {sql}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "duckdb's output is not JSON: {e}\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[test]
fn duckdb_reads_back_what_the_parquet_crate_wrote() {
    let Some(binary) = duckdb() else {
        eprintln!(
            "skipping the DuckDB cross-check: no CLI found. This is a reference \
             reader, never a dependency - set DUCKDB_BIN to run it."
        );
        return;
    };

    let scratch = common::Scratch::new("duckdb");
    let outcome = build::run(&Options {
        data: common::fixture("wide"),
        out: scratch.path().to_path_buf(),
        commit: "deadbeef".into(),
        skip_lint: false,
        schema_dir: Some(common::schema_dir()),
    })
    .expect("the build succeeds");

    let parquet = scratch.join("identities.parquet");
    let parquet = parquet.to_str().expect("a printable path");
    assert!(
        !parquet.contains('\''),
        "the scratch path would break the SQL"
    );
    let source = format!("read_parquet('{parquet}')");

    // Row counts, for the parent and for both nested children. Unnesting a
    // list column has to reproduce the flat tables the JSON artifact carries.
    let counts = query(
        &binary,
        &format!(
            "SELECT (SELECT count(*) FROM {source}) AS games, \
             (SELECT count(*) FROM (SELECT unnest(stores) FROM {source})) AS stores, \
             (SELECT count(*) FROM (SELECT unnest(aliases) FROM {source})) AS aliases;"
        ),
    );
    assert_eq!(
        counts[0],
        json!({
            "games": outcome.tables.games.len(),
            "stores": outcome.tables.stores.len(),
            "aliases": outcome.tables.aliases.len(),
        }),
        "DuckDB counts different rows than the builder wrote"
    );

    // Every row, column for column, against the JSON artifact - which is the
    // same `Tables` value serialised, so agreeing with it is agreeing with
    // the shape.
    let rows = query(
        &binary,
        &format!(
            "SELECT id, title, page, year, variant_of, note, steam, umu, stores, aliases \
             FROM {source} ORDER BY id;"
        ),
    );
    let rows = rows.as_array().expect("an array of rows");
    assert_eq!(rows.len(), outcome.tables.games.len());

    for (row, game) in rows.iter().zip(&outcome.tables.games) {
        let stores: Vec<Value> = outcome
            .tables
            .stores
            .iter()
            .filter(|s| s.id == game.id)
            .map(|s| {
                json!({
                    "store": s.store,
                    "codename": s.codename,
                    "edition": s.edition,
                    "exe": s.exe,
                    "seen": s.seen,
                    "source": s.source,
                    "confidence": s.confidence,
                })
            })
            .collect();
        let aliases: Vec<&str> = outcome
            .tables
            .aliases
            .iter()
            .filter(|a| a.id == game.id)
            .map(|a| a.alias.as_str())
            .collect();

        assert_eq!(
            row,
            &json!({
                "id": game.id,
                "title": game.title,
                "page": game.page,
                "year": game.year,
                "variant_of": game.variant_of,
                "note": game.note,
                "steam": game.steam,
                "umu": game.umu,
                "stores": stores,
                "aliases": aliases,
            }),
            "DuckDB read '{}' back differently",
            game.id
        );
    }

    // The alias lookup is the whole reason the table exists, so ask DuckDB to
    // do one: an Epic codename, through the alias list, to a game's title.
    let resolved = query(
        &binary,
        &format!("SELECT title FROM {source} WHERE list_contains(aliases, 'egs-AuroraDeluxe');"),
    );
    assert_eq!(resolved[0]["title"], json!("Aurora"));
}
