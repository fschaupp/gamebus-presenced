//! MPRIS naming hints (S6).
//!
//! Not a source: MPRIS players never create, keep alive, or remove activity
//! records, and never appear in `Sources`. This watcher only harvests the one
//! thing MPRIS is authoritative about — a human-readable application name —
//! and hands it to the enricher as a [`SourceEvent::NameHint`].
//!
//! The join is evidence-based, never fabricated: a hint carries the player's
//! kernel-verified pid (`GetConnectionUnixProcessID`, same trust rule as the
//! Discord listener's `SO_PEERCRED`), and the enricher applies it only to a
//! record that pid belongs to. Precedence is the naming anti-goal's: a hint
//! replaces only a *default* name (empty, or the executable stem) that
//! Discord, the group identity, and detectable.json all failed to improve —
//! one rung above the stem, below everything curated.
//!
//! Documented miss: for sandboxed players (Flatpak browsers and the like)
//! `GetConnectionUnixProcessID` returns the `xdg-dbus-proxy` pid, not the
//! application's — the hint then joins nothing, by design. Observed live
//! 2026-08-06 with a Flatpak zen browser. Same category as the wrapper-tree
//! Discord join miss: we do not fabricate joins across sandbox boundaries.

use std::collections::HashMap;

use tokio::sync::mpsc;
use tracing::{debug, warn};
use zbus::export::futures_util::StreamExt;
use zbus::{fdo, Connection};

use crate::sources::SourceEvent;

/// Every MPRIS player claims a bus name under this prefix.
const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
/// The single well-known object path of the MPRIS specification.
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const MPRIS_INTERFACE: &str = "org.mpris.MediaPlayer2";

/// Watch for MPRIS players appearing on the bus and emit naming hints.
///
/// Seeds from `ListNames` so players that were already running are seen, then
/// follows `NameOwnerChanged`. Player exit emits nothing: applied names are
/// monotone (a record never regresses to a stem because a player closed), and
/// the enricher prunes hints for dead pids on its tick.
pub async fn watch(conn: Connection, tx: mpsc::Sender<SourceEvent>) {
    let fdo = match fdo::DBusProxy::new(&conn).await {
        Ok(proxy) => proxy,
        Err(e) => {
            warn!(error = %e, "Failed to create bus proxy; MPRIS naming hints disabled");
            return;
        }
    };

    let mut owner_changes = match fdo.receive_name_owner_changed().await {
        Ok(stream) => stream,
        Err(e) => {
            warn!(error = %e, "Failed to subscribe to NameOwnerChanged; MPRIS naming hints disabled");
            return;
        }
    };

    // Seed: players already on the bus when we start.
    // pid → unique-name of the hint already sent, so an owner change that
    // re-announces the same player does not re-emit.
    let mut seen: HashMap<u32, String> = HashMap::new();
    if let Ok(names) = fdo.list_names().await {
        for name in names {
            if name.starts_with(MPRIS_PREFIX) {
                emit_hint(&conn, &fdo, name.as_str(), &tx, &mut seen).await;
            }
        }
    }

    while let Some(signal) = owner_changes.next().await {
        let Ok(args) = signal.args() else { continue };
        let name = args.name.as_str();
        if !name.starts_with(MPRIS_PREFIX) {
            continue;
        }
        // Only appearances matter; disappearances are handled by monotone
        // naming + the enricher's dead-pid prune.
        if args.new_owner.is_none() {
            continue;
        }
        emit_hint(&conn, &fdo, name, &tx, &mut seen).await;
    }
    debug!("NameOwnerChanged stream ended; MPRIS naming hints stopped");
}

/// Resolve one player's pid and Identity, and send the hint.
async fn emit_hint(
    conn: &Connection,
    fdo: &fdo::DBusProxy<'_>,
    bus_name: &str,
    tx: &mpsc::Sender<SourceEvent>,
    seen: &mut HashMap<u32, String>,
) {
    let Ok(owned): Result<zbus::names::BusName, _> = bus_name.try_into() else {
        return;
    };
    // Kernel-verified pid of the connection that owns the player name.
    let pid = match fdo.get_connection_unix_process_id(owned).await {
        Ok(pid) => pid,
        Err(e) => {
            debug!(player = bus_name, error = %e, "MPRIS player pid unresolvable; hint skipped");
            return;
        }
    };

    let Some(identity) = read_identity(conn, bus_name).await else {
        debug!(
            player = bus_name,
            pid, "MPRIS player has no usable Identity; hint skipped"
        );
        return;
    };

    if seen.get(&pid).map(String::as_str) == Some(bus_name) {
        return;
    }
    seen.insert(pid, bus_name.to_string());

    debug!(player = bus_name, pid, name = %identity, "MPRIS naming hint");
    let _ = tx
        .send(SourceEvent::NameHint {
            pid,
            name: identity,
        })
        .await;
}

/// Read `org.mpris.MediaPlayer2.Identity` from a player.
async fn read_identity(conn: &Connection, bus_name: &str) -> Option<String> {
    let proxy = zbus::Proxy::new(
        conn,
        bus_name.to_string(),
        MPRIS_PATH,
        "org.freedesktop.DBus.Properties",
    )
    .await
    .ok()?;
    let value: zbus::zvariant::OwnedValue = proxy
        .call("Get", &(MPRIS_INTERFACE, "Identity"))
        .await
        .ok()?;
    let identity: String = value.try_into().ok()?;
    let identity = identity.trim().to_string();
    (!identity.is_empty()).then_some(identity)
}
