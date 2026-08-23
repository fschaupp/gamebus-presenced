//! `identities.sqlite`: the relational shape as itself, indexed for lookup.
//!
//! Written with `rusqlite`'s bundled SQLite - one C file compiled by cargo,
//! no system package, no DuckDB. This is the artifact the daemon reads first;
//! Parquet on the client is a later read-path change, which is the whole
//! point of keeping the TOML pages as the source of truth.
//!
//! Determinism needs a little care here, because a SQLite file's page layout
//! depends on the order rows arrive in. Rows are inserted in the sorted order
//! the shape defines, the page size is pinned rather than inherited from the
//! build machine, and `VACUUM` at the end rewrites the file into one canonical
//! layout. Nothing in the format carries a timestamp.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::build::Tables;
use crate::model::{Error, Result};

/// `PRAGMA application_id`: "GDB1", so `file` and anything else reading the
/// header can tell what this database is rather than just that it is SQLite.
const APPLICATION_ID: i32 = 0x4744_4231;

const SCHEMA: &str = "\
CREATE TABLE games (
    id         TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    -- The page's file stem. A label, not a key: unlike `id` it is not
    -- unique by construction.
    page       TEXT NOT NULL,
    year       INTEGER,
    variant_of TEXT,
    note       TEXT,
    steam      INTEGER,
    umu        TEXT
);

CREATE TABLE stores (
    id         TEXT NOT NULL REFERENCES games(id),
    store      TEXT NOT NULL,
    codename   TEXT NOT NULL,
    edition    TEXT,
    exe        TEXT,
    seen       TEXT NOT NULL,
    source     TEXT NOT NULL,
    confidence TEXT NOT NULL
);

-- Every identifier that resolves to a page. TEXT PRIMARY KEY gives the
-- lookup index on `alias` for free, and enforces that no identifier ever
-- resolves to two games.
CREATE TABLE aliases (
    alias TEXT PRIMARY KEY,
    id    TEXT NOT NULL REFERENCES games(id)
);

CREATE TABLE helpers (
    exe      TEXT PRIMARY KEY,
    reason   TEXT NOT NULL,
    incident TEXT,
    seen     TEXT NOT NULL
);

-- 'which game owns this store product' - the launcher's own question.
CREATE INDEX stores_store_codename ON stores(store, codename);
-- 'what else does this game sell as' - the step after resolving an alias.
CREATE INDEX stores_id ON stores(id);
";

fn fail(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Data(format!("identities.sqlite: {what}: {e}"))
}

pub fn write(path: &Path, tables: &Tables) -> Result<()> {
    // Removed rather than overwritten: opening an existing database would
    // build on whatever it already held.
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let stale = path.with_file_name(format!(
            "{}{suffix}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        if stale.exists() {
            std::fs::remove_file(&stale).map_err(|source| Error::Io {
                path: stale,
                source,
            })?;
        }
    }

    let connection = Connection::open(path).map_err(|e| fail("opening", e))?;
    // Before any page is written, or it has no effect.
    connection
        .execute_batch(&format!(
            "PRAGMA page_size = 4096;\n\
             PRAGMA application_id = {APPLICATION_ID};\n\
             PRAGMA user_version = {};\n",
            crate::SCHEMA_VERSION
        ))
        .map_err(|e| fail("stamping the header", e))?;
    connection
        .execute_batch(SCHEMA)
        .map_err(|e| fail("creating the schema", e))?;

    connection
        .execute_batch("BEGIN")
        .map_err(|e| fail("beginning", e))?;
    {
        let mut games = connection
            .prepare("INSERT INTO games (id, title, page, year, variant_of, note, steam, umu) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")
            .map_err(|e| fail("preparing games", e))?;
        for row in &tables.games {
            games
                .execute(params![
                    row.id,
                    row.title,
                    row.page,
                    row.year,
                    row.variant_of,
                    row.note,
                    row.steam,
                    row.umu
                ])
                .map_err(|e| fail("inserting a game", e))?;
        }

        let mut stores = connection
            .prepare("INSERT INTO stores (id, store, codename, edition, exe, seen, source, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")
            .map_err(|e| fail("preparing stores", e))?;
        for row in &tables.stores {
            stores
                .execute(params![
                    row.id,
                    row.store,
                    row.codename,
                    row.edition,
                    row.exe,
                    row.seen,
                    row.source,
                    row.confidence
                ])
                .map_err(|e| fail("inserting a store entry", e))?;
        }

        let mut aliases = connection
            .prepare("INSERT INTO aliases (alias, id) VALUES (?1, ?2)")
            .map_err(|e| fail("preparing aliases", e))?;
        for row in &tables.aliases {
            aliases
                .execute(params![row.alias, row.id])
                .map_err(|e| fail("inserting an alias", e))?;
        }

        let mut helpers = connection
            .prepare("INSERT INTO helpers (exe, reason, incident, seen) VALUES (?1, ?2, ?3, ?4)")
            .map_err(|e| fail("preparing helpers", e))?;
        for row in &tables.helpers {
            helpers
                .execute(params![row.exe, row.reason, row.incident, row.seen])
                .map_err(|e| fail("inserting a helper", e))?;
        }
    }
    connection
        .execute_batch("COMMIT")
        .map_err(|e| fail("committing", e))?;

    // One canonical page layout, whatever order the pages happened to land in.
    connection
        .execute_batch("VACUUM")
        .map_err(|e| fail("vacuuming", e))?;
    connection.close().map_err(|(_, e)| fail("closing", e))?;
    Ok(())
}
