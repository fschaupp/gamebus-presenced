//! gamebus-presenced - D-Bus presence daemon for Linux desktop.
//!
//! Publishes a unified "what is this machine playing" presence on the session bus
//! by collecting fragments from multiple sources (GameMode, Discord IPC, Steam)
//! and correlating them by pid.
//!
//! S4a: source events flow through the Enricher middleware, which probes
//! `/proc/<pid>/environ` for Steam appids and manages the Steam partial
//! lifecycle (tied to last non-Steam source).

mod cache;
mod correlator;
mod dbus;
mod enricher;
mod error;
mod naming;
mod sources;

use crate::cache::CachedRecord;
use crate::correlator::{Correlator, Effect};
use crate::dbus::activity::ActivityInterface;
use crate::dbus::connection::Connection;
use crate::dbus::manager::{Manager, ManagerInterface};
use crate::dbus::types::{Activity, BUS_NAME, ROOT_PATH, VERSION};
use crate::error::Result;
use crate::sources::{discord, gamemode, SourceEvent};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use zbus::zvariant::OwnedObjectPath;

/// Main entry point for the presence daemon.
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging. RUST_LOG overrides the default info level.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("gamebus_presenced=info")),
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

    // Signal context for Manager signals (ActivityAdded/Removed) and the
    // HasActivity property change notification.
    let manager_ref = conn
        .inner()
        .object_server()
        .interface::<_, ManagerInterface>(ROOT_PATH)
        .await?;
    let signal_ctxt = manager_ref.signal_context().clone();
    drop(manager_ref);

    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);

    // Daemon core state. The Enricher starts without a naming database;
    // it's loaded after sources spawn so the 12MB JSON parse doesn't delay
    // the Discord listener or GameMode watcher.
    let mut correlator = Correlator::new();
    let mut enricher = enricher::Enricher::new();

    // S3c: re-adopt cache-restored records BEFORE sources start, so the
    // sources' re-derived records merge into them (rather than racing them).
    for record in cache::load_in(&runtime_dir) {
        info!(
            pid = record.pid,
            id = %record.activity.id,
            "Re-adopting cached activity"
        );
        let effects = correlator.adopt(record.activity);
        apply_effects(&conn, &manager, &signal_ctxt, effects).await;
    }

    // Channel from source watchers to the daemon core.
    let (tx, mut rx) = mpsc::channel::<SourceEvent>(64);

    // Spawn the GameMode source watcher on its own connection handle.
    tokio::spawn(gamemode::watch(conn.inner().clone(), tx.clone()));
    info!("GameMode source watcher started");
    tokio::spawn(discord::listen(tx));
    info!("Discord IPC source listener started");

    // S4b: load the naming database now that sources are running.
    // Blocking (~100ms for 12MB JSON), but the listeners are already up.
    enricher.load_naming();

    // S4e: periodically retry identification for wrappers whose game hasn't
    // launched yet (Battle.net launcher → actual game starts minutes later).
    let mut reidentify_interval = tokio::time::interval(std::time::Duration::from_secs(15));
    reidentify_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    info!("Service ready. Waiting for activity...");

    // Daemon core: consume source events, enrich, correlate by pid, maintain the bus.
    loop {
        tokio::select! {
            event = rx.recv() => {
                let raw_events = match event {
                    Some(e) => enricher.process(e),
                    None => {
                        // All source watchers have exited; nothing left to do.
                        warn!("All activity sources exited; shutting down core loop");
                        break;
                    }
                };
                for enriched in raw_events {
                    let effects = match enriched {
                        SourceEvent::Updated(activity) => correlator.on_updated(*activity),
                        SourceEvent::Removed { id, source } => correlator.on_removed(&id, source),
                        SourceEvent::SourceLost { source } => correlator.on_source_lost(source),
                    };
                    apply_effects(&conn, &manager, &signal_ctxt, effects).await;
                }
                // S3c: write the published surface through to the restart cache.
                sync_cache(&correlator, &manager, &runtime_dir).await;
            }
            _ = reidentify_interval.tick() => {
                let tick_events = enricher.tick();
                if !tick_events.is_empty() {
                    for enriched in tick_events {
                        let effects = match enriched {
                            SourceEvent::Updated(activity) => correlator.on_updated(*activity),
                            SourceEvent::Removed { id, source } => correlator.on_removed(&id, source),
                            SourceEvent::SourceLost { source } => correlator.on_source_lost(source),
                        };
                        apply_effects(&conn, &manager, &signal_ctxt, effects).await;
                    }
                    sync_cache(&correlator, &manager, &runtime_dir).await;
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

/// Write the currently published records to the restart cache (S3c).
/// Best-effort: failures are logged inside the cache module, never fatal.
async fn sync_cache(
    correlator: &Correlator,
    manager: &Arc<Manager>,
    runtime_dir: &std::path::Path,
) {
    let snapshot = manager.snapshot().await;
    let records: Vec<CachedRecord> = correlator
        .published_pairs()
        .into_iter()
        .filter_map(|(pid, id)| {
            let activity = snapshot.iter().find(|a| a.id == id)?.clone();
            // Don't cache Steam-only records: the periodic scan re-finds
            // them on restart. Caching them would re-adopt stale entries
            // for processes that died between runs.
            if activity.sources == [crate::dbus::types::Source::Steam] {
                return None;
            }
            let start_time = cache::process_start_time(pid)?;
            Some(CachedRecord {
                pid,
                start_time,
                activity,
            })
        })
        .collect();
    cache::save_in(runtime_dir, &records);
}

/// Execute the correlator's bus effects in order.
async fn apply_effects(
    conn: &Connection,
    manager: &Arc<Manager>,
    signal_ctxt: &zbus::object_server::SignalContext<'_>,
    effects: Vec<Effect>,
) {
    for effect in effects {
        match effect {
            Effect::PublishNew(activity) => {
                publish_new(conn, manager, signal_ctxt, activity).await;
            }
            Effect::UpdateInPlace(activity) => {
                update_in_place(conn, manager, signal_ctxt, activity).await;
            }
            Effect::Remove(id) => {
                remove_by_id(conn, manager, signal_ctxt, &id).await;
            }
        }
    }
}

/// Register a new activity object and announce it. Consumers reacting to
/// ActivityAdded must find the object already present.
async fn publish_new(
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

    info!(id = %outcome.id, sources = ?activity.sources, "Activity published");

    let stored = Activity {
        id: outcome.id.clone(),
        ..activity
    };
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

/// Refresh an existing activity object in place.
async fn update_in_place(
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
    if !outcome.replaced {
        // The correlator believes this object exists but the registry did
        // not - treat it as a fresh publish to self-heal.
        publish_new(conn, manager, signal_ctxt, activity).await;
        return;
    }
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
}

/// Remove one activity object from the bus and emit ActivityRemoved.
async fn remove_by_id(
    conn: &Connection,
    manager: &Arc<Manager>,
    signal_ctxt: &zbus::object_server::SignalContext<'_>,
    id: &str,
) {
    let outcome = match manager.remove_activity(id).await {
        Ok(outcome) => outcome,
        Err(e) => {
            error!(error = %e, "Failed to remove activity");
            return;
        }
    };

    if !outcome.existed {
        tracing::debug!(id, "Removal for unknown activity (already removed?)");
        return;
    }

    info!(id, "Activity removed");
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

    if outcome.became_empty {
        emit_has_activity_changed(conn).await;
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
