//! gamebus-presenced - D-Bus presence daemon for Linux desktop.
//!
//! Publishes a unified "what is this machine playing" presence on the session bus
//! by collecting fragments from multiple sources (GameMode, Discord IPC, Steam)
//! and correlating them by pid.
//!
//! The D-Bus surface fed by the GameMode source - pid +
//! executable presence with no Discord code at all.

mod dbus;
mod error;
mod sources;

use crate::dbus::activity::ActivityInterface;
use crate::dbus::connection::Connection;
use crate::dbus::manager::{Manager, ManagerInterface};
use crate::dbus::types::{Activity, Source, BUS_NAME, ROOT_PATH, VERSION};
use crate::error::Result;
use crate::sources::{discord, gamemode, SourceEvent};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use zbus::zvariant::OwnedObjectPath;

/// Main entry point for the presence daemon.
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("gamebus_presenced=info".parse().unwrap()),
        )
        .init();

    info!("gamebus-presenced v{} starting", VERSION);
    info!("Bus name: {}", BUS_NAME);
    info!("Root path: {}", ROOT_PATH);

    // Create D-Bus connection
    info!("Connecting to session bus...");
    let conn = Connection::new().await?;
    info!("Connected to session bus");

    // Create manager
    let manager = Arc::new(Manager::new());

    // Register the Manager interface at the root path
    let manager_iface = ManagerInterface::new(manager.clone());
    conn.inner()
        .object_server()
        .at(ROOT_PATH, manager_iface)
        .await?;

    info!("Manager interface registered at {}", ROOT_PATH);

    // Channel from source watchers to the daemon core.
    let (tx, mut rx) = mpsc::channel::<SourceEvent>(64);

    // Spawn the GameMode source watcher on its own connection handle.
    tokio::spawn(gamemode::watch(conn.inner().clone(), tx.clone()));
    info!("GameMode source watcher started");
    tokio::spawn(discord::listen(tx));
    info!("Discord IPC source listener started");

    // Signal context for Manager signals (ActivityAdded/Removed) and the
    // HasActivity property change notification.
    let manager_ref = conn
        .inner()
        .object_server()
        .interface::<_, ManagerInterface>(ROOT_PATH)
        .await?;
    let signal_ctxt = manager_ref.signal_context().clone();
    drop(manager_ref);

    info!("Service ready. Waiting for activity...");

    // Daemon core: consume source events and maintain the bus surface.
    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Some(SourceEvent::Updated(activity)) => {
                        handle_updated(&conn, &manager, &signal_ctxt, *activity).await;
                    }
                    Some(SourceEvent::Removed { id, source }) => {
                        handle_removed(&conn, &manager, &signal_ctxt, &id, source).await;
                    }
                    Some(SourceEvent::SourceLost { source }) => {
                        handle_source_lost(&conn, &manager, &signal_ctxt, source).await;
                    }
                    None => {
                        // All source watchers have exited; nothing left to do.
                        warn!("All activity sources exited; shutting down core loop");
                        break;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down...");
                break;
            }
        }
    }

    Ok(())
}

/// Publish a new activity or update an existing object in place.
async fn handle_updated(
    conn: &Connection,
    manager: &Arc<Manager>,
    signal_ctxt: &zbus::object_server::SignalContext<'_>,
    activity: Activity,
) {
    let outcome = match manager.add_activity(activity.clone()).await {
        Ok(outcome) => outcome,
        Err(e) => {
            error!(error = %e, "Failed to add activity");
            return;
        }
    };

    info!(id = %outcome.id, sources = ?activity.sources, "Activity updated");

    let path = Activity::path_for_id(&outcome.id);
    if outcome.replaced {
        match conn
            .inner()
            .object_server()
            .interface::<_, ActivityInterface>(path.as_str())
            .await
        {
            Ok(iface_ref) => {
                let mut iface = iface_ref.get_mut().await;
                if let Err(e) = iface.update(activity, iface_ref.signal_context()).await {
                    error!(error = %e, path, "Failed to update activity object");
                }
            }
            Err(e) => error!(error = %e, path, "Failed to get activity object for update"),
        }
        return;
    }

    // Register the per-activity object, then announce it. Consumers
    // reacting to ActivityAdded must find the object already present.
    let mut stored = activity;
    stored.id = outcome.id.clone();
    let path = stored.object_path();
    if let Err(e) = conn
        .inner()
        .object_server()
        .at(path.as_str(), ActivityInterface::new(stored))
        .await
    {
        error!(error = %e, path, "Failed to register activity object");
        return;
    }

    let owned_path = match OwnedObjectPath::try_from(path.clone()) {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, path, "Invalid object path");
            return;
        }
    };

    if let Err(e) = ManagerInterface::activity_added(signal_ctxt, owned_path).await {
        warn!(error = %e, "Failed to emit ActivityAdded");
    }

    if outcome.became_non_empty {
        emit_has_activity_changed(conn).await;
    }
}

/// Remove one source activity.
async fn handle_removed(
    conn: &Connection,
    manager: &Arc<Manager>,
    signal_ctxt: &zbus::object_server::SignalContext<'_>,
    id: &str,
    source: Source,
) {
    let outcome = match manager.remove_activity(id).await {
        Ok(outcome) => outcome,
        Err(e) => {
            error!(error = %e, "Failed to remove activity");
            return;
        }
    };

    if !outcome.existed {
        tracing::debug!(
            id,
            ?source,
            "Removal for unknown activity (already removed?)"
        );
        return;
    }

    info!(id, ?source, "Activity removed");
    remove_activity_object(conn, signal_ctxt, id).await;

    if outcome.became_empty {
        emit_has_activity_changed(conn).await;
    }
}

/// Handle gamemoded leaving the bus: all gamemode-sourced records die.
async fn handle_source_lost(
    conn: &Connection,
    manager: &Arc<Manager>,
    signal_ctxt: &zbus::object_server::SignalContext<'_>,
    source: Source,
) {
    let removed = manager.remove_by_source(source).await;
    if removed.is_empty() {
        return;
    }

    info!(
        ?source,
        count = removed.len(),
        "Source lost; dropping its activities"
    );
    let had_activity_before = !removed.is_empty();

    for id in &removed {
        remove_activity_object(conn, signal_ctxt, id).await;
    }

    if had_activity_before && !manager.has_activity().await {
        emit_has_activity_changed(conn).await;
    }
}

/// Remove an activity object from the bus and emit ActivityRemoved.
async fn remove_activity_object(
    conn: &Connection,
    signal_ctxt: &zbus::object_server::SignalContext<'_>,
    id: &str,
) {
    let path = Activity::path_for_id(id);

    if let Err(e) = conn
        .inner()
        .object_server()
        .remove::<ActivityInterface, _>(path.as_str())
        .await
    {
        warn!(error = %e, path, "Failed to remove activity object");
    }

    match OwnedObjectPath::try_from(path.clone()) {
        Ok(owned_path) => {
            if let Err(e) = ManagerInterface::activity_removed(signal_ctxt, owned_path).await {
                warn!(error = %e, "Failed to emit ActivityRemoved");
            }
        }
        Err(e) => warn!(error = %e, path, "Invalid object path"),
    }
}

/// Emit PropertiesChanged for HasActivity on the Manager interface.
async fn emit_has_activity_changed(conn: &Connection) {
    match conn
        .inner()
        .object_server()
        .interface::<_, ManagerInterface>(ROOT_PATH)
        .await
    {
        Ok(iface_ref) => {
            let iface = iface_ref.get().await;
            if let Err(e) = iface.has_activity_changed(iface_ref.signal_context()).await {
                warn!(error = %e, "Failed to emit HasActivity change");
            }
        }
        Err(e) => warn!(error = %e, "Failed to get Manager interface for HasActivity change"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_version_constant() {
        assert_eq!(VERSION, 1);
    }
}
