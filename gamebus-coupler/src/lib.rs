//! What gamebus-setup's front-ends share: the stashed records, the
//! judgements no front-end may recompute, and the edits as data.
//!
//! gamebus-setup's own TUI and beisl's compat window are equal consumers of
//! this crate. It does no IO: loading, persisting, fetching and the umu
//! database stay in the engine (gamebus-setup). A verb is a value, so the
//! same [`MissVerb`] is an in-process call today and a message tomorrow.
//! Anything that depends on the date takes it as a parameter; the clock is
//! the engine's.

pub mod compat;
pub mod miss;
pub mod verb;

pub use compat::{
    canonical_signature, pending_targets, CompatFinding, Observation, WallKind, TARGETS,
};
pub use miss::{
    normalize_store, umu_candidate, Confidence, DraftBasis, DraftedId, FixCheck, Miss,
    Verification, VerificationState,
};
pub use verb::{
    apply_finding, apply_miss, FindingChange, FindingVerb, MissChange, MissVerb, Refusal,
};
