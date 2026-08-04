//! Core D-Bus types for gamebus-presenced.
//!
//! This module defines the data structures that map to D-Bus interfaces
//! as specified in docs/design/gamebus-presence.md.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use rsrpc::cmd::Activity as RsrpcActivity;

/// Bus name for the presence service.
pub const BUS_NAME: &str = "org.gamebus.Presence.v1";

/// Root object path for the presence service.
pub const ROOT_PATH: &str = "/org/gamebus/Presence/v1";

/// Current protocol version.
/// Bump this for additive changes; incompatible changes require a new bus name (e.g., .v2).
pub const VERSION: u64 = 1;

/// Source of an activity record.
/// Indicates which integration provided the information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Discord Rich Presence via IPC socket
    Discord,
    /// Feral GameMode daemon
    GameMode,
    /// Steam appid from registry.vdf or /proc/<pid>/environ
    Steam,
    /// Unknown or unclassified source
    Unknown,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Discord => write!(f, "discord"),
            Source::GameMode => write!(f, "gamemode"),
            Source::Steam => write!(f, "steam"),
            Source::Unknown => write!(f, "unknown"),
        }
    }
}

impl From<Source> for &str {
    fn from(source: Source) -> Self {
        match source {
            Source::Discord => "discord",
            Source::GameMode => "gamemode",
            Source::Steam => "steam",
            Source::Unknown => "unknown",
        }
    }
}

/// Kind of activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A game is running
    Game,
    /// A non-game application is running
    App,
    /// Kind is unknown
    Unknown,
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kind::Game => write!(f, "game"),
            Kind::App => write!(f, "app"),
            Kind::Unknown => write!(f, "unknown"),
        }
    }
}

/// Source-scoped application IDs.
/// Maps source name to the ID string that source uses.
/// e.g., {"discord": "123456789", "steam": "480"}
pub type AppIds = HashMap<String, String>;

/// Extra fields that don't have a dedicated property.
/// Escape hatch for source-specific data we haven't modelled.
/// For S0, we use String values to avoid lifetime issues with Value.
pub type Extra = HashMap<String, String>;

/// An activity record representing what a process is doing.
///
/// All properties are read-only and emit change signals when modified.
/// Empty string means "not known", never a placeholder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    /// Unique identifier for this activity.
    /// Used in object paths: /org/gamebus/Presence/v1/Activity/<id>
    pub id: String,

    /// Which sources back this record.
    pub sources: Vec<Source>,

    /// Kind of activity: game, app, or unknown.
    pub kind: Kind,

    /// Resolved human name; falls back to executable stem if unknown.
    pub name: String,

    /// Discord's first free-text line (usually title).
    pub details: String,

    /// Discord's second free-text line (usually chapter/state).
    pub state: String,

    /// Process ID. 0 when not correlated to a process.
    pub process_id: u32,

    /// Executable path from GameMode or /proc/<pid>/exe.
    pub executable: String,

    /// Source-scoped IDs (discord app ID, steam app ID, etc.).
    pub app_ids: AppIds,

    /// Unix timestamp (seconds) when activity started. 0 = unset.
    pub since: i64,

    /// Unix timestamp (seconds) when activity ended. 0 = unset.
    pub until: i64,

    /// Large artwork image URL or key.
    pub large_image: String,

    /// Large artwork hover text.
    pub large_text: String,

    /// Small artwork image URL or key.
    pub small_image: String,

    /// Small artwork hover text.
    pub small_text: String,

    /// Current party size. 0 = unset.
    pub party_size: u32,

    /// Maximum party size. 0 = unset.
    pub party_max: u32,

    /// Escape hatch for unmodelled source fields.
    pub extra: Extra,
}

impl Activity {
    /// Create a new activity with the given ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            sources: Vec::new(),
            kind: Kind::Unknown,
            name: String::new(),
            details: String::new(),
            state: String::new(),
            process_id: 0,
            executable: String::new(),
            app_ids: HashMap::new(),
            since: 0,
            until: 0,
            large_image: String::new(),
            large_text: String::new(),
            small_image: String::new(),
            small_text: String::new(),
            party_size: 0,
            party_max: 0,
            extra: HashMap::new(),
        }
    }

    /// Build an activity from a GameMode registration.
    ///
    /// Per the design doc: `name` falls back to the executable stem, and every
    /// field GameMode cannot answer stays empty ("empty string means not
    /// known"). The activity ID is `pid_<pid>` - underscores, not hyphens,
    /// because D-Bus object path elements allow only alphanumerics and `_`.
    ///
    /// `since` is the registration time reported by GameMode (unix seconds).
    pub fn from_gamemode(pid: i32, executable: &str, since: i64) -> Self {
        let name = std::path::Path::new(executable)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();

        Self {
            id: format!("pid_{pid}"),
            sources: vec![Source::GameMode],
            kind: Kind::Game,
            name,
            process_id: pid.max(0) as u32,
            executable: executable.to_string(),
            since,
            ..Self::new("")
        }
    }

    /// Build an activity from a Discord SET_ACTIVITY payload.
    ///
    /// `activity` is rsRPC's parsed payload **after** `ActivityCmd::fix()`,
    /// so its timestamps are already normalised to milliseconds.
    pub fn from_discord(pid: i32, client_id: &str, activity: &RsrpcActivity) -> Self {
        let (start_ms, end_ms) = activity
            .timestamps
            .as_ref()
            .map(timestamp_pair)
            .unwrap_or((None, None));
        let assets = activity.assets.as_ref();
        let party_size = activity
            .party
            .as_ref()
            .and_then(|p| p.size.as_ref())
            .and_then(|size| size.first())
            .copied()
            .unwrap_or(0);
        let party_max = activity
            .party
            .as_ref()
            .and_then(|p| p.size.as_ref())
            .and_then(|size| size.get(1))
            .copied()
            .unwrap_or(0);
        let mut app_ids = HashMap::new();
        if !client_id.is_empty() {
            app_ids.insert("discord".to_string(), client_id.to_string());
        }

        Self {
            id: format!("discord_{pid}"),
            sources: vec![Source::Discord],
            kind: if matches!(activity.r#type, 0 | 5) {
                Kind::Game
            } else {
                Kind::App
            },
            name: activity.name.clone().unwrap_or_default(),
            details: activity.details.clone().unwrap_or_default(),
            state: activity.state.clone().unwrap_or_default(),
            process_id: pid.max(0) as u32,
            app_ids,
            since: start_ms.map(|ms| ms / 1000).unwrap_or(0),
            until: end_ms.map(|ms| ms / 1000).unwrap_or(0),
            large_image: assets
                .and_then(|a| a.large_image.clone())
                .unwrap_or_default(),
            large_text: assets
                .and_then(|a| a.large_text.clone())
                .unwrap_or_default(),
            small_image: assets
                .and_then(|a| a.small_image.clone())
                .unwrap_or_default(),
            small_text: assets
                .and_then(|a| a.small_text.clone())
                .unwrap_or_default(),
            party_size,
            party_max,
            ..Self::new("")
        }
    }

    /// Generate the D-Bus object path for an activity ID.
    pub fn path_for_id(id: &str) -> String {
        format!("{}/Activity/{}", ROOT_PATH, id)
    }

    /// Generate the D-Bus object path for this activity.
    pub fn object_path(&self) -> String {
        Self::path_for_id(&self.id)
    }
}

/// Extract (start, end) in milliseconds from rsRPC's `Timestamps`.
///
/// rsRPC's `TimeoutValue` tuple field is crate-private, so we go through its
/// `Serialize` impl - after `ActivityCmd::fix()`, values are milliseconds.
fn timestamp_pair(ts: &rsrpc::cmd::Timestamps) -> (Option<i64>, Option<i64>) {
    let value = serde_json::to_value(ts).unwrap_or_default();
    (
        value.get("start").and_then(serde_json::Value::as_i64),
        value.get("end").and_then(serde_json::Value::as_i64),
    )
}

impl Default for Activity {
    fn default() -> Self {
        Self::new("default")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activity_object_path() {
        let activity = Activity::new("test-123");
        assert_eq!(
            activity.object_path(),
            "/org/gamebus/Presence/v1/Activity/test-123"
        );
    }

    #[test]
    fn test_source_display() {
        assert_eq!(format!("{}", Source::Discord), "discord");
        assert_eq!(format!("{}", Source::GameMode), "gamemode");
        assert_eq!(format!("{}", Source::Steam), "steam");
        assert_eq!(format!("{}", Source::Unknown), "unknown");
    }

    #[test]
    fn test_kind_display() {
        assert_eq!(format!("{}", Kind::Game), "game");
        assert_eq!(format!("{}", Kind::App), "app");
        assert_eq!(format!("{}", Kind::Unknown), "unknown");
    }

    #[test]
    fn test_from_gamemode_basic() {
        let activity = Activity::from_gamemode(12345, "/usr/games/eldenring", 1700000000);
        assert_eq!(activity.id, "pid_12345");
        assert_eq!(activity.sources, vec![Source::GameMode]);
        assert_eq!(activity.kind, Kind::Game);
        assert_eq!(activity.name, "eldenring");
        assert_eq!(activity.process_id, 12345);
        assert_eq!(activity.executable, "/usr/games/eldenring");
        assert_eq!(activity.since, 1700000000);
        // Fields GameMode cannot answer stay empty/zero.
        assert_eq!(activity.details, "");
        assert_eq!(activity.state, "");
        assert_eq!(activity.until, 0);
        assert!(activity.app_ids.is_empty());
        assert!(activity.extra.is_empty());
    }

    #[test]
    fn test_from_gamemode_name_fallback() {
        // .exe-style suffix: stem strips the last extension only.
        let activity = Activity::from_gamemode(1, "/games/foo/bar.exe", 1);
        assert_eq!(activity.name, "bar");

        // No file name at all: name stays empty (never a placeholder).
        let activity = Activity::from_gamemode(1, "", 1);
        assert_eq!(activity.name, "");

        // Root path has no file stem.
        let activity = Activity::from_gamemode(1, "/", 1);
        assert_eq!(activity.name, "");

        // Trailing slashes are normalised away: stem of the last component.
        let activity = Activity::from_gamemode(1, "/usr/games/", 1);
        assert_eq!(activity.name, "games");
    }

    #[test]
    fn test_from_gamemode_object_path() {
        let activity = Activity::from_gamemode(999, "/bin/game", 1);
        assert_eq!(
            activity.object_path(),
            "/org/gamebus/Presence/v1/Activity/pid_999"
        );
    }

    #[test]
    fn test_from_discord_maps_fields() {
        // Parse the real wire format through rsRPC's battle-tested model,
        // including its fix() normalisation (timestamps -> milliseconds).
        let mut cmd: rsrpc::cmd::ActivityCmd = serde_json::from_str(
            r#"{
                "cmd": "SET_ACTIVITY",
                "args": {
                    "pid": 42,
                    "activity": {
                        "name": "Elden Ring",
                        "type": 0,
                        "details": "Limgrave",
                        "state": "Exploring",
                        "timestamps": { "start": 1700000000000, "end": 1700003600000 },
                        "assets": {
                            "large_image": "large",
                            "large_text": "The Lands Between",
                            "small_image": "small",
                            "small_text": "Online"
                        },
                        "party": { "id": "p1", "size": [2, 4] }
                    }
                },
                "nonce": "abc"
            }"#,
        )
        .unwrap();
        cmd.fix();
        let payload = cmd.args.as_ref().and_then(|a| a.activity.as_ref()).unwrap();

        let activity = Activity::from_discord(42, "123456", payload);
        assert_eq!(activity.id, "discord_42");
        assert_eq!(activity.sources, vec![Source::Discord]);
        assert_eq!(activity.kind, Kind::Game);
        assert_eq!(activity.name, "Elden Ring");
        assert_eq!(activity.details, "Limgrave");
        assert_eq!(activity.state, "Exploring");
        assert_eq!(activity.process_id, 42);
        assert_eq!(activity.app_ids.get("discord").unwrap(), "123456");
        assert_eq!(activity.since, 1_700_000_000);
        assert_eq!(activity.until, 1_700_003_600);
        assert_eq!(activity.party_size, 2);
        assert_eq!(activity.party_max, 4);
        assert_eq!(activity.large_image, "large");
        assert_eq!(activity.large_text, "The Lands Between");
        assert_eq!(activity.small_image, "small");
        assert_eq!(activity.small_text, "Online");
        // Discord does not know the executable; that is the correlator's job.
        assert_eq!(activity.executable, "");
    }

    #[test]
    fn test_from_discord_empty_fields_stay_empty() {
        let payload: rsrpc::cmd::Activity = serde_json::from_str(r#"{"type": 2}"#).unwrap();
        let activity = Activity::from_discord(7, "", &payload);
        assert_eq!(activity.id, "discord_7");
        assert_eq!(activity.kind, Kind::App);
        assert_eq!(activity.name, "");
        assert_eq!(activity.details, "");
        assert_eq!(activity.state, "");
        assert_eq!(activity.since, 0);
        assert_eq!(activity.party_size, 0);
        assert!(activity.app_ids.is_empty());
    }
}
