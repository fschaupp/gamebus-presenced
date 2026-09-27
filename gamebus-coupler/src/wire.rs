//! What gamebus-setup's MCP server answers with, beyond the records.
//!
//! A row is a record plus its key and the judgements the engine computed for
//! it, flattened into one object so a reader sees the record's own fields at
//! the top level (the shape `compat --json` has always had).

use serde::{Deserialize, Serialize};

use crate::compat::CompatFinding;
use crate::miss::Miss;

/// One identity miss, as `list_misses` and `get_miss` return it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissRow {
    pub key: String,
    /// [`Miss::is_umu_miss`], computed by the engine.
    pub is_umu_miss: bool,
    /// [`crate::umu_candidate`], computed by the engine.
    pub umu_candidate: bool,
    #[serde(flatten)]
    pub miss: Miss,
}

/// One compat finding, as `list_findings` and `get_finding` return it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingRow {
    pub key: String,
    /// [`crate::pending_targets`], computed by the engine.
    pub needs: Vec<String>,
    #[serde(flatten)]
    pub finding: CompatFinding,
}

/// Why a tool call failed. Each reason has exactly one cure, so a client can
/// tell the user what to do rather than showing a bare refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorReason {
    /// The server was started without `--allow-edits`.
    EditsNotEnabled,
    /// The server was started without `--allow-network`.
    NetworkNotEnabled,
    /// No `gamebus-setup auth init` has run yet.
    AuthUninitialized,
    /// An edit before `gamebus/authenticate` succeeded (a client bug).
    NotAuthenticated,
    /// Explicit-approval mode: the client is not approved yet.
    ClientUnapproved,
    /// A different key than the one on record for this client name.
    ClientKeyMismatch,
    /// The ledger is required but absent.
    LedgerMissing,
    /// The ledger or its owner signatures do not verify.
    LedgerUnverified,
    /// A stash file exists but did not parse. Never reported as an empty list.
    StashUnreadable,
    /// No record under the requested key.
    NotFound,
    /// The coupler refused the verb; `message` says why.
    Refused,
    /// The arguments did not match the tool's schema.
    BadArguments,
    /// No such tool.
    UnknownTool,
}

/// The body of an `isError` tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolError {
    pub reason: ErrorReason,
    pub message: String,
}

impl ToolError {
    pub fn new(reason: ErrorReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
}
