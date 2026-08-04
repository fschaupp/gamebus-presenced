//! Activity D-Bus interface.
//!
//! This module implements the `org.gamebus.Presence.v1.Activity` interface:
//! one D-Bus object per activity at `/org/gamebus/Presence/v1/Activity/<id>`,
//! all properties read-only.

use crate::dbus::types::Activity;
use std::collections::HashMap;
use zbus::{interface, zvariant::Value};

/// D-Bus interface for a single activity object.
///
/// The data is immutable for the lifetime of the object: sources that learn
/// more about an activity publish a new object rather than mutating this one
/// (S1 GameMode records never change after registration).
#[derive(Debug)]
pub struct ActivityInterface {
    activity: Activity,
}

impl ActivityInterface {
    /// Create a new ActivityInterface wrapping the given activity record.
    pub fn new(activity: Activity) -> Self {
        Self { activity }
    }

    /// Replace the activity data and emit property changes in place.
    ///
    /// SET_ACTIVITY is infrequent, so emitting every property keeps this
    /// straightforward and guarantees consumers never miss an update.
    pub async fn update(
        &mut self,
        activity: Activity,
        ctxt: &zbus::object_server::SignalContext<'_>,
    ) -> zbus::Result<()> {
        self.activity = activity;
        self.sources_changed(ctxt).await?;
        self.kind_changed(ctxt).await?;
        self.name_changed(ctxt).await?;
        self.details_changed(ctxt).await?;
        self.state_changed(ctxt).await?;
        self.process_id_changed(ctxt).await?;
        self.executable_changed(ctxt).await?;
        self.app_ids_changed(ctxt).await?;
        self.since_changed(ctxt).await?;
        self.until_changed(ctxt).await?;
        self.large_image_changed(ctxt).await?;
        self.large_text_changed(ctxt).await?;
        self.small_image_changed(ctxt).await?;
        self.small_text_changed(ctxt).await?;
        self.party_size_changed(ctxt).await?;
        self.party_max_changed(ctxt).await?;
        self.extra_changed(ctxt).await?;
        Ok(())
    }
}

/// Implementation of the Activity D-Bus interface.
///
/// Property names follow the design doc (`ProcessId`, `AppIds`, ...); the
/// macro's default snake_case -> PascalCase conversion produces exactly these.
#[interface(name = "org.gamebus.Presence.v1.Activity")]
impl ActivityInterface {
    /// Which sources back this record, e.g. ["gamemode"].
    #[zbus(property)]
    async fn sources(&self) -> Vec<String> {
        self.activity
            .sources
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// Kind of activity: "game", "app", or "unknown".
    #[zbus(property)]
    async fn kind(&self) -> String {
        self.activity.kind.to_string()
    }

    /// Resolved human name; falls back to the executable stem.
    #[zbus(property)]
    async fn name(&self) -> String {
        self.activity.name.clone()
    }

    /// Discord's first free-text line; empty when unknown.
    #[zbus(property)]
    async fn details(&self) -> String {
        self.activity.details.clone()
    }

    /// Discord's second free-text line; empty when unknown.
    #[zbus(property)]
    async fn state(&self) -> String {
        self.activity.state.clone()
    }

    /// Process ID; 0 when not correlated to a process.
    #[zbus(property)]
    async fn process_id(&self) -> u32 {
        self.activity.process_id
    }

    /// Executable path from GameMode or /proc/<pid>/exe.
    #[zbus(property)]
    async fn executable(&self) -> String {
        self.activity.executable.clone()
    }

    /// Source-scoped IDs, e.g. {"discord": "...", "steam": "..."}.
    #[zbus(property)]
    async fn app_ids(&self) -> HashMap<String, String> {
        self.activity.app_ids.clone()
    }

    /// Unix seconds when the activity started; 0 = unset.
    #[zbus(property)]
    async fn since(&self) -> u64 {
        self.activity.since.max(0) as u64
    }

    /// Unix seconds when the activity ended; 0 = unset.
    #[zbus(property)]
    async fn until(&self) -> u64 {
        self.activity.until.max(0) as u64
    }

    /// Large artwork image URL or key.
    #[zbus(property)]
    async fn large_image(&self) -> String {
        self.activity.large_image.clone()
    }

    /// Large artwork hover text.
    #[zbus(property)]
    async fn large_text(&self) -> String {
        self.activity.large_text.clone()
    }

    /// Small artwork image URL or key.
    #[zbus(property)]
    async fn small_image(&self) -> String {
        self.activity.small_image.clone()
    }

    /// Small artwork hover text.
    #[zbus(property)]
    async fn small_text(&self) -> String {
        self.activity.small_text.clone()
    }

    /// Current party size; 0 = unset.
    #[zbus(property)]
    async fn party_size(&self) -> u32 {
        self.activity.party_size
    }

    /// Maximum party size; 0 = unset.
    #[zbus(property)]
    async fn party_max(&self) -> u32 {
        self.activity.party_max
    }

    /// Escape hatch for unmodelled source fields (a{sv}).
    ///
    /// Values are serialised as D-Bus variants. S1 GameMode records never
    /// carry Extra, so this is empty for now.
    #[zbus(property)]
    async fn extra(&self) -> HashMap<String, Value<'static>> {
        self.activity
            .extra
            .iter()
            .map(|(k, v)| (k.clone(), Value::from(v.clone())))
            .collect()
    }
}
