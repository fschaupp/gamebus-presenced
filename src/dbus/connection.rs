//! D-Bus connection management.
//!
//! This module handles establishing and maintaining the D-Bus connection
//! for the presence service.

use crate::dbus::types::BUS_NAME;
use crate::error::Result;
use zbus::{Connection as ZbusConnection, ConnectionBuilder};

/// Manages the D-Bus connection for the presence service.
pub struct Connection {
    inner: ZbusConnection,
}

impl Connection {
    /// Create a new connection to the session bus.
    pub async fn new() -> Result<Self> {
        let conn = ConnectionBuilder::session()?
            .name(BUS_NAME)?
            .build()
            .await?;

        Ok(Self { inner: conn })
    }

    /// Get the underlying zbus connection.
    pub fn inner(&self) -> &ZbusConnection {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbus::types::ROOT_PATH;

    #[tokio::test]
    async fn test_bus_name_constant() {
        assert_eq!(BUS_NAME, "org.gamebus.Presence.v1");
    }

    #[tokio::test]
    async fn test_root_path_constant() {
        assert_eq!(ROOT_PATH, "/org/gamebus/Presence/v1");
    }
}
