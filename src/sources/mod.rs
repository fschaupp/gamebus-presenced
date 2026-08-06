//! Activity sources for gamebus-presenced.
//!
//! Each source watches an external provider (GameMode, Discord IPC, Steam)
//! and emits events that the daemon core turns into D-Bus activity objects.
//! Sources are independent and individually optional: a missing source
//! degrades the record, never the daemon.

pub mod discord;
pub mod gamemode;

use crate::dbus::types::{Activity, Source};

/// Source-independent events consumed by the daemon core.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceEvent {
    /// A source created or updated an activity.
    Updated(Box<Activity>),
    /// A source stopped asserting one activity.
    Removed { id: String, source: Source },
    /// A source disappeared; all records backed by it must be removed.
    SourceLost { source: Source },
}
