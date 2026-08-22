//! Lint the `gamedb` data set and build the artifacts published from it.
//!
//! Two jobs, one crate, because they read the same files and agree on the
//! same rules by construction:
//!
//! * [`lint`] is the merge gate. It is a port of `.scripts/gamedb-lint.py`
//!   and is held to that script's own fixture set, byte for byte, by
//!   `tests/lint_fixtures.rs`.
//! * [`build`] turns the TOML pages into the release assets: JSON, Parquet,
//!   SQLite, the flat helper list, and a manifest of checksums.
//!
//! The crate is a workspace member the daemon never depends on. The workspace
//! root sets `default-members = ["."]`, so `cargo build` and friends at the
//! repo root behave exactly as they did before it existed.

pub mod build;
pub mod jsonschema;
pub mod lint;
pub mod model;
pub mod pyrepr;

pub mod emit;
pub mod parquet_out;
pub mod sqlite_out;

/// Schema version stamped into `identities.sqlite` (`user_version`) and
/// carried in `identities.json` and `manifest.toml`.
///
/// Bump it when the relational shape changes in a way a consumer would
/// notice: a column added, removed, or given a different meaning.
pub const SCHEMA_VERSION: i64 = 1;
