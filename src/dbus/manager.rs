//! Manager interface implementation.
//!
//! This module implements the org.gamebus.Presence.v1.Manager D-Bus interface
//! plus the in-memory activity registry behind it.

use crate::dbus::types::{Activity, Source, VERSION};
use crate::error::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use zbus::interface;
use zbus::object_server::SignalContext;
use zbus::zvariant::OwnedObjectPath;

/// Path prefix for activity objects.
pub const ACTIVITY_PATH_PREFIX: &str = "/org/gamebus/Presence/v1/Activity/";

/// Result of adding an activity to the registry.
#[derive(Debug)]
pub struct AddOutcome {
    /// The activity's ID (generated if it was empty).
    pub id: String,
    /// True if an activity with this ID already existed and was replaced.
    pub replaced: bool,
    /// True if the registry flipped from empty to non-empty.
    pub became_non_empty: bool,
}

/// Result of removing an activity from the registry.
#[derive(Debug)]
pub struct RemoveOutcome {
    /// True if an activity with this ID existed.
    pub existed: bool,
    /// True if the registry is now empty.
    pub became_empty: bool,
}

/// Manager interface for the presence service.
///
/// Provides methods to list activities and query presence status,
/// plus signals for activity changes.
#[derive(Debug)]
pub struct Manager {
    /// Map of activity ID to Activity object.
    activities: Arc<RwLock<HashMap<String, Activity>>>,
}

impl Manager {
    /// Create a new Manager.
    pub fn new() -> Self {
        Self {
            activities: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add an activity to the registry.
    ///
    /// Idempotent: an existing ID is replaced (the caller is expected to
    /// refresh the D-Bus object) and reports `replaced` so no add-signal
    /// storm follows.
    pub async fn add_activity(&self, activity: Activity) -> Result<AddOutcome> {
        let mut activities = self.activities.write().await;

        let id = if activity.id.is_empty() {
            // Fallback for sources without a natural ID. Today all
            // activities arrive with an ID (pid_<pid>); this is a safety net.
            format!("activity-{}", activities.len() + 1)
        } else {
            activity.id.clone()
        };

        let mut new_activity = activity;
        new_activity.id = id.clone();

        let replaced = activities.insert(id.clone(), new_activity).is_some();
        Ok(AddOutcome {
            id,
            replaced,
            became_non_empty: activities.len() == 1 && !replaced,
        })
    }

    /// Remove an activity from the registry.
    pub async fn remove_activity(&self, id: &str) -> Result<RemoveOutcome> {
        let mut activities = self.activities.write().await;
        let existed = activities.remove(id).is_some();
        Ok(RemoveOutcome {
            existed,
            became_empty: existed && activities.is_empty(),
        })
    }

    /// Remove all activities backed by the given source, returning their IDs.
    ///
    /// Used when a source daemon leaves the bus: per the design rule "the
    /// record dies with the last source". Today all records are single-source,
    /// so a record backed by the lost source is removed outright.
    pub async fn remove_by_source(&self, source: Source) -> Vec<String> {
        let mut activities = self.activities.write().await;
        let doomed: Vec<String> = activities
            .iter()
            .filter(|(_, a)| a.sources.contains(&source))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &doomed {
            activities.remove(id);
        }
        doomed
    }

    /// Get all activity IDs.
    pub async fn list_activities(&self) -> Vec<String> {
        let activities = self.activities.read().await;
        activities.keys().cloned().collect()
    }

    /// Check if any activities exist.
    pub async fn has_activity(&self) -> bool {
        let activities = self.activities.read().await;
        !activities.is_empty()
    }

    /// Get the protocol version.
    pub fn version(&self) -> u64 {
        VERSION
    }
}

/// D-Bus interface for the Manager.
#[derive(Debug)]
pub struct ManagerInterface {
    manager: Arc<Manager>,
}

impl ManagerInterface {
    pub fn new(manager: Arc<Manager>) -> Self {
        Self { manager }
    }
}

/// Implementation of the Manager D-Bus interface.
#[interface(name = "org.gamebus.Presence.v1.Manager")]
impl ManagerInterface {
    /// List all activity object paths.
    ///
    /// Returns an array of D-Bus object paths (`ao`) for active activities.
    pub async fn list_activities(&self) -> Vec<OwnedObjectPath> {
        self.manager
            .list_activities()
            .await
            .into_iter()
            .filter_map(|id| {
                OwnedObjectPath::try_from(format!("{}{}", ACTIVITY_PATH_PREFIX, id)).ok()
            })
            .collect()
    }

    /// Check if any activity exists.
    ///
    /// This is a cheap "is anything running" check for panels.
    #[zbus(property)]
    pub async fn has_activity(&self) -> bool {
        self.manager.has_activity().await
    }

    /// Get the protocol version.
    ///
    /// Additive changes bump this property. Incompatible changes
    /// earn a new bus name (e.g., .v2).
    #[zbus(property)]
    pub fn version(&self) -> u64 {
        self.manager.version()
    }

    /// Emitted when a new activity appears. Carries the activity's object path.
    #[zbus(signal)]
    pub async fn activity_added(
        ctxt: &SignalContext<'_>,
        object_path: OwnedObjectPath,
    ) -> zbus::Result<()>;

    /// Emitted when an activity ends. Carries the activity's object path.
    #[zbus(signal)]
    pub async fn activity_removed(
        ctxt: &SignalContext<'_>,
        object_path: OwnedObjectPath,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_add_activity_uses_given_id() {
        let manager = Manager::new();
        let outcome = manager
            .add_activity(Activity::from_gamemode(42, "/bin/game", 1))
            .await
            .unwrap();
        assert_eq!(outcome.id, "pid_42");
        assert!(!outcome.replaced);
        assert!(outcome.became_non_empty);
        assert!(manager.has_activity().await);
    }

    #[tokio::test]
    async fn test_add_activity_idempotent() {
        let manager = Manager::new();
        manager
            .add_activity(Activity::from_gamemode(42, "/bin/game", 1))
            .await
            .unwrap();
        let outcome = manager
            .add_activity(Activity::from_gamemode(42, "/bin/game", 1))
            .await
            .unwrap();
        assert!(outcome.replaced);
        // Replacing the only entry must not flip has_activity.
        assert!(!outcome.became_non_empty);
        assert_eq!(manager.list_activities().await.len(), 1);
    }

    #[tokio::test]
    async fn test_remove_activity() {
        let manager = Manager::new();
        manager
            .add_activity(Activity::from_gamemode(42, "/bin/game", 1))
            .await
            .unwrap();
        let outcome = manager.remove_activity("pid_42").await.unwrap();
        assert!(outcome.existed);
        assert!(outcome.became_empty);
        assert!(!manager.has_activity().await);

        // Removing again reports the miss.
        let outcome = manager.remove_activity("pid_42").await.unwrap();
        assert!(!outcome.existed);
        assert!(!outcome.became_empty);
    }

    #[tokio::test]
    async fn test_remove_by_source() {
        let manager = Manager::new();
        manager
            .add_activity(Activity::from_gamemode(1, "/bin/a", 1))
            .await
            .unwrap();
        manager
            .add_activity(Activity::from_gamemode(2, "/bin/b", 1))
            .await
            .unwrap();

        // An activity from a different source must survive.
        let mut other = Activity::new("other-1");
        other.sources = vec![Source::Discord];
        manager.add_activity(other).await.unwrap();

        let removed = manager.remove_by_source(Source::GameMode).await;
        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"pid_1".to_string()));
        assert!(removed.contains(&"pid_2".to_string()));
        assert_eq!(manager.list_activities().await, vec!["other-1".to_string()]);
    }
}
