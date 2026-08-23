//! `identities.parquet`: the games table, columnar, with its children nested.
//!
//! Parquet holds one table per file and the shape has four, so the two child
//! tables ride along inside the parent: `stores` as a `LIST<STRUCT<...>>` and
//! `aliases` as a `LIST<VARCHAR>`, both in the same column order the flat
//! shape declares. Nothing is lost - unnesting either column reproduces the
//! flat table exactly, and `tests/build.rs` checks that it does. `helpers` is
//! not a property of a game and stays in `identities.json`,
//! `identities.sqlite` and `shared-helpers.txt`.
//!
//! Written with the pure-Rust `parquet` crate. DuckDB reads this file and
//! `tests/duckdb_crosscheck.rs` makes it prove so, but DuckDB is a reference
//! reader and never a build or test dependency: that test skips cleanly when
//! the binary is absent.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Int32Array, Int64Array, ListArray, RecordBatch, StringArray, StructArray,
};
use arrow_buffer::{OffsetBuffer, ScalarBuffer};
use arrow_schema::{DataType, Field, Fields, Schema};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

use crate::build::Tables;
use crate::model::{Error, Result};

/// Stamped into the file instead of the crate's own version string, so a
/// dependency bump cannot change an artifact's checksum without the data
/// changing.
const CREATED_BY: &str = "gamedb-build (identities v1)";

fn fail(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Data(format!("identities.parquet: {what}: {e}"))
}

/// The `STRUCT` one `stores` row becomes, in the flat table's column order
/// minus `id`, which the parent row already carries.
fn store_fields() -> Fields {
    Fields::from(vec![
        Field::new("store", DataType::Utf8, false),
        Field::new("codename", DataType::Utf8, false),
        Field::new("edition", DataType::Utf8, true),
        Field::new("exe", DataType::Utf8, true),
        Field::new("seen", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("confidence", DataType::Utf8, false),
    ])
}

fn schema() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("page", DataType::Utf8, false),
        Field::new("year", DataType::Int32, true),
        Field::new("variant_of", DataType::Utf8, true),
        Field::new("note", DataType::Utf8, true),
        Field::new("steam", DataType::Int64, true),
        Field::new("umu", DataType::Utf8, true),
        Field::new(
            "stores",
            DataType::List(Arc::new(Field::new(
                "item",
                DataType::Struct(store_fields()),
                false,
            ))),
            false,
        ),
        Field::new(
            "aliases",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
            false,
        ),
    ])
}

/// Build the record batch. Public so a test can compare it against the JSON
/// without going through a file.
pub fn batch(tables: &Tables) -> Result<RecordBatch> {
    // Both children are already sorted by the parent's key, so grouping is a
    // single pass and the order inside each list is the flat table's order.
    let mut by_game: HashMap<&str, Vec<&crate::build::StoreRow>> = HashMap::new();
    for row in &tables.stores {
        by_game.entry(&row.id).or_default().push(row);
    }
    let mut aliases_by_game: HashMap<&str, Vec<&str>> = HashMap::new();
    for row in &tables.aliases {
        aliases_by_game.entry(&row.id).or_default().push(&row.alias);
    }

    let mut ids: Vec<&str> = Vec::with_capacity(tables.games.len());
    let mut titles: Vec<&str> = Vec::with_capacity(tables.games.len());
    let mut pages: Vec<&str> = Vec::with_capacity(tables.games.len());
    let mut years: Vec<Option<i32>> = Vec::with_capacity(tables.games.len());
    let mut variant_of: Vec<Option<&str>> = Vec::with_capacity(tables.games.len());
    let mut notes: Vec<Option<&str>> = Vec::with_capacity(tables.games.len());
    let mut steam: Vec<Option<i64>> = Vec::with_capacity(tables.games.len());
    let mut umu: Vec<Option<&str>> = Vec::with_capacity(tables.games.len());

    let mut store_offsets: Vec<i32> = vec![0];
    let (mut c_store, mut c_codename, mut c_seen, mut c_source, mut c_confidence) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut c_edition, mut c_exe): (Vec<Option<&str>>, Vec<Option<&str>>) =
        (Vec::new(), Vec::new());

    let mut alias_offsets: Vec<i32> = vec![0];
    let mut alias_values: Vec<&str> = Vec::with_capacity(tables.aliases.len());

    for game in &tables.games {
        ids.push(&game.id);
        titles.push(&game.title);
        pages.push(&game.page);
        years.push(game.year.map(|y| y as i32));
        variant_of.push(game.variant_of.as_deref());
        notes.push(game.note.as_deref());
        steam.push(game.steam);
        umu.push(game.umu.as_deref());

        for row in by_game.get(game.id.as_str()).into_iter().flatten() {
            c_store.push(row.store.as_str());
            c_codename.push(row.codename.as_str());
            c_edition.push(row.edition.as_deref());
            c_exe.push(row.exe.as_deref());
            c_seen.push(row.seen.as_str());
            c_source.push(row.source.as_str());
            c_confidence.push(row.confidence.as_str());
        }
        store_offsets.push(c_store.len() as i32);

        for alias in aliases_by_game.get(game.id.as_str()).into_iter().flatten() {
            alias_values.push(alias);
        }
        alias_offsets.push(alias_values.len() as i32);
    }

    let store_struct = StructArray::try_new(
        store_fields(),
        vec![
            Arc::new(StringArray::from(c_store)) as ArrayRef,
            Arc::new(StringArray::from(c_codename)) as ArrayRef,
            Arc::new(StringArray::from(c_edition)) as ArrayRef,
            Arc::new(StringArray::from(c_exe)) as ArrayRef,
            Arc::new(StringArray::from(c_seen)) as ArrayRef,
            Arc::new(StringArray::from(c_source)) as ArrayRef,
            Arc::new(StringArray::from(c_confidence)) as ArrayRef,
        ],
        None,
    )
    .map_err(|e| fail("building the stores struct", e))?;

    let stores = ListArray::try_new(
        Arc::new(Field::new("item", DataType::Struct(store_fields()), false)),
        OffsetBuffer::new(ScalarBuffer::from(store_offsets)),
        Arc::new(store_struct),
        None,
    )
    .map_err(|e| fail("building the stores list", e))?;

    let aliases = ListArray::try_new(
        Arc::new(Field::new("item", DataType::Utf8, false)),
        OffsetBuffer::new(ScalarBuffer::from(alias_offsets)),
        Arc::new(StringArray::from(alias_values)) as ArrayRef,
        None,
    )
    .map_err(|e| fail("building the aliases list", e))?;

    RecordBatch::try_new(
        Arc::new(schema()),
        vec![
            Arc::new(StringArray::from(ids)) as ArrayRef,
            Arc::new(StringArray::from(titles)) as ArrayRef,
            Arc::new(StringArray::from(pages)) as ArrayRef,
            Arc::new(Int32Array::from(years)) as ArrayRef,
            Arc::new(StringArray::from(variant_of)) as ArrayRef,
            Arc::new(StringArray::from(notes)) as ArrayRef,
            Arc::new(Int64Array::from(steam)) as ArrayRef,
            Arc::new(StringArray::from(umu)) as ArrayRef,
            Arc::new(stores) as ArrayRef,
            Arc::new(aliases) as ArrayRef,
        ],
    )
    .map_err(|e| fail("assembling the record batch", e))
}

pub fn write(path: &Path, tables: &Tables) -> Result<()> {
    let batch = batch(tables)?;
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(
            ZstdLevel::try_new(3).map_err(|e| fail("choosing a zstd level", e))?,
        ))
        .set_created_by(CREATED_BY.to_string())
        .build();

    let file = File::create(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(properties))
        .map_err(|e| fail("opening the writer", e))?;
    writer.write(&batch).map_err(|e| fail("writing", e))?;
    writer.close().map_err(|e| fail("closing", e))?;
    Ok(())
}

/// Read the file back with the same crate that wrote it, as one batch.
///
/// The round trip a test needs, and the thing DuckDB is then asked to agree
/// with.
pub fn read(path: &Path) -> Result<Vec<RecordBatch>> {
    let file = File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| fail("opening for read", e))?
        .build()
        .map_err(|e| fail("building the reader", e))?;
    reader
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| fail("reading", e))
}
