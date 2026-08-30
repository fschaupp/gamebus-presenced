//! Shared D-Bus client for the `org.gamebus.Presence.v1` surface.
//!
//! Included by both `gamebus-presence` and `gamebus-setup` with
//! `#[path = "../client.rs"] mod client;` - the proxies and the property reads
//! live here once, and each binary renders the result its own way (plain text
//! for the CLI, a list widget for the TUI).
//!
//! Not a module of the daemon: `src/main.rs` never declares it, so nothing here
//! is linked into `gamebus-presenced`.

use std::collections::HashMap;
use zbus::zvariant::OwnedObjectPath;
use zbus::{proxy, Connection};

/// Client proxy for the Manager interface.
#[proxy(
    interface = "org.gamebus.Presence.v1.Manager",
    default_service = "org.gamebus.Presence.v1",
    default_path = "/org/gamebus/Presence/v1"
)]
trait Manager {
    fn list_activities(&self) -> zbus::Result<Vec<OwnedObjectPath>>;

    #[zbus(property)]
    fn has_activity(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn version(&self) -> zbus::Result<u64>;
}

/// Client proxy for Activity objects.
#[proxy(
    interface = "org.gamebus.Presence.v1.Activity",
    default_service = "org.gamebus.Presence.v1",
    assume_defaults = false
)]
trait ActivityProps {
    #[zbus(property)]
    fn sources(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn kind(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn details(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn process_id(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn executable(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn app_ids(&self) -> zbus::Result<HashMap<String, String>>;
    #[zbus(property)]
    fn since(&self) -> zbus::Result<u64>;
}

/// One activity, read off the bus into plain data.
///
/// Every field degrades to its default on a read error - an activity that
/// vanished mid-read is a normal race, not a failure worth propagating.
#[derive(Debug, Clone, Default)]
pub struct ActivityView {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub sources: Vec<String>,
    pub pid: u32,
    pub executable: String,
    pub details: String,
    pub state: String,
    pub app_ids: HashMap<String, String>,
    pub since: u64,
}

impl ActivityView {
    /// Name for display, falling back when the sources gave us nothing.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            "(unknown)"
        } else {
            &self.name
        }
    }

    /// How long this has been going, from the `Since` timestamp.
    ///
    /// `None` when no source supplied one, or when it is in the future - a
    /// clock change should read as "unknown", not as a negative duration.
    pub fn elapsed(&self) -> Option<String> {
        if self.since == 0 {
            return None;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let secs = now.checked_sub(self.since)?;
        Some(match (secs / 3600, (secs % 3600) / 60) {
            (0, 0) => "just now".to_string(),
            (0, m) => format!("{m}m"),
            (h, m) => format!("{h}h {m}m"),
        })
    }
}

/// The whole bus surface in one read: interface version, HasActivity, activities.
pub async fn snapshot(conn: &Connection) -> zbus::Result<(u64, bool, Vec<ActivityView>)> {
    let manager = ManagerProxy::new(conn).await?;
    let version = manager.version().await.unwrap_or(0);
    let has_activity = manager.has_activity().await.unwrap_or(false);

    let mut views = Vec::new();
    for path in manager.list_activities().await.unwrap_or_default() {
        let builder = match ActivityPropsProxy::builder(conn).path(path.as_str()) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let proxy = match builder.build().await {
            Ok(p) => p,
            Err(_) => continue,
        };

        views.push(ActivityView {
            path: path.as_str().to_string(),
            name: proxy.name().await.unwrap_or_default(),
            kind: proxy.kind().await.unwrap_or_default(),
            sources: proxy.sources().await.unwrap_or_default(),
            pid: proxy.process_id().await.unwrap_or(0),
            executable: proxy.executable().await.unwrap_or_default(),
            details: proxy.details().await.unwrap_or_default(),
            state: proxy.state().await.unwrap_or_default(),
            app_ids: proxy.app_ids().await.unwrap_or_default(),
            since: proxy.since().await.unwrap_or(0),
        });
    }

    Ok((version, has_activity, views))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs_ago: u64) -> ActivityView {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        ActivityView {
            since: now - secs_ago,
            ..Default::default()
        }
    }

    #[test]
    fn elapsed_reads_as_a_human_would_say_it() {
        assert_eq!(at(30).elapsed().as_deref(), Some("just now"));
        assert_eq!(at(90).elapsed().as_deref(), Some("1m"));
        assert_eq!(at(3 * 3600 + 25 * 60).elapsed().as_deref(), Some("3h 25m"));
    }

    #[test]
    fn no_timestamp_means_no_claim() {
        // Sources that never supplied a start time must not be rendered as
        // having started at the epoch.
        assert_eq!(ActivityView::default().elapsed(), None);
    }

    #[test]
    fn a_future_timestamp_is_unknown_rather_than_negative() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let future = ActivityView {
            since: now + 3600,
            ..Default::default()
        };
        assert_eq!(future.elapsed(), None);
    }

    #[test]
    fn an_empty_name_falls_back_rather_than_rendering_blank() {
        assert_eq!(ActivityView::default().display_name(), "(unknown)");
        let named = ActivityView {
            name: "Brotato".to_string(),
            ..Default::default()
        };
        assert_eq!(named.display_name(), "Brotato");
    }
}
