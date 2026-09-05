//! Setup/status tooling for `gamebus-setup`.
//!
//! Included by `src/bin/gamebus-setup.rs` with
//! `#[path = "../setup/mod.rs"] mod setup;`. Deliberately outside `src/bin/`,
//! where cargo's binary auto-discovery would try to compile these files as
//! binaries in their own right.
//!
//! The split is: `paths` and `units` are pure; `status` probes the system and
//! hands back plain data that pure functions turn into rows; `actions` plans a
//! list of steps (pure) that `execute` then performs.

pub mod actions;
pub mod compat;
pub mod gamedb;
pub mod heroic_library;
pub mod lutris_library;
pub mod mcp;
pub mod paths;
pub mod status;
pub mod ui;
pub mod umu_misses;
pub mod units;
