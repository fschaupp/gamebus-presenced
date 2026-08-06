//! GameMode source watcher.
//!
//! Watches `com.feralinteractive.GameMode` on the session bus: seeds current
//! games via `ListGames`, then forwards `GameRegistered`/`GameUnregistered`
//! signals to the daemon core.
//!
//! Availability is tracked with `NameOwnerChanged`: when gamemoded appears
//! the watcher connects (D-Bus activation included), when it vanishes the
//! watcher emits [`GameModeEvent::SourceLost`] and waits for its return.

use crate::dbus::types::{Activity, Source};
use crate::sources::SourceEvent;
use std::sync::Arc;
use tokio::sync::{mpsc, Notify};
use tracing::{debug, info, warn};
use zbus::export::futures_util::StreamExt;
use zbus::zvariant::OwnedObjectPath;
use zbus::{fdo, proxy, Connection};

/// Well-known bus name of the GameMode daemon.
const GAMEMODE_NAME: &str = "com.feralinteractive.GameMode";

/// How long to wait before retrying after a session-level failure while the
/// name stays owned (transient errors).
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// zbus proxy for the GameMode daemon.
///
/// Note: GameMode types the game reference as an object path (`o`) on the
/// wire - it is the per-game object `/com/feralinteractive/GameMode/Games/<pid>`,
/// NOT the executable. The executable lives on that object's `Executable`
/// property.
#[proxy(
    interface = "com.feralinteractive.GameMode",
    default_service = "com.feralinteractive.GameMode",
    default_path = "/com/feralinteractive/GameMode"
)]
trait GameMode {
    /// List all currently registered games as (pid, game-object-path) pairs.
    fn list_games(&self) -> zbus::Result<Vec<(i32, OwnedObjectPath)>>;

    /// Emitted when a game registers with GameMode.
    #[zbus(signal)]
    fn game_registered(&self, pid: i32, game: OwnedObjectPath) -> zbus::Result<()>;

    /// Emitted when a game unregisters from GameMode.
    #[zbus(signal)]
    fn game_unregistered(&self, pid: i32, game: OwnedObjectPath) -> zbus::Result<()>;
}

/// zbus proxy for a per-game object under `/com/feralinteractive/GameMode/Games/`.
#[proxy(
    interface = "com.feralinteractive.GameMode.Game",
    default_service = "com.feralinteractive.GameMode",
    assume_defaults = false
)]
trait GameProps {
    /// Full path of the registered executable.
    #[zbus(property)]
    fn executable(&self) -> zbus::Result<String>;

    /// Unix seconds when the game registered with GameMode.
    #[zbus(property)]
    fn timestamp(&self) -> zbus::Result<u64>;
}

/// Resolve details for a registered game.
///
/// Primary source is the per-game GameMode object. If that read races with
/// the game exiting, fall back to /proc/<pid>/exe for the executable (per
/// the design doc) and to the local clock for `since`.
async fn resolve_game(conn: &Connection, pid: i32, game_path: &OwnedObjectPath) -> (String, i64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut executable = String::new();
    let mut since = now;

    match GamePropsProxy::builder(conn)
        .path(game_path.as_str())
        .map(|b| b.build())
    {
        Ok(builder) => match builder.await {
            Ok(game) => {
                match game.executable().await {
                    Ok(exe) => executable = exe,
                    Err(e) => debug!(error = %e, pid, "Failed to read Game Executable property"),
                }
                match game.timestamp().await {
                    Ok(ts) => since = ts as i64,
                    Err(e) => debug!(error = %e, pid, "Failed to read Game Timestamp property"),
                }
            }
            Err(e) => debug!(error = %e, pid, "Failed to build Game proxy"),
        },
        Err(e) => debug!(error = %e, pid, "Invalid game object path"),
    }

    if executable.is_empty() {
        match std::fs::read_link(format!("/proc/{pid}/exe")) {
            Ok(path) => executable = path.to_string_lossy().into_owned(),
            Err(e) => debug!(error = %e, pid, "Failed to resolve executable via /proc"),
        }
    }

    (executable, since)
}

/// Emit `Updated` for every currently registered game (the `ListGames` seed
/// loop). Shared between session start and the tick-driven reseed: re-emitting
/// a registration is idempotent because the correlator drops identical
/// re-publishes.
async fn seed_games(
    conn: &Connection,
    proxy: &GameModeProxy<'_>,
    tx: &mpsc::Sender<SourceEvent>,
) -> zbus::Result<usize> {
    let games = proxy.list_games().await?;
    let count = games.len();
    for (pid, game_path) in games {
        let (executable, since) = resolve_game(conn, pid, &game_path).await;
        send(
            tx,
            SourceEvent::Updated(Box::new(Activity::from_gamemode(pid, &executable, since))),
        )
        .await;
    }
    Ok(count)
}

/// Run the GameMode watcher task.
///
/// Loops forever: connects when gamemoded is reachable, forwards events, and
/// emits [`GameModeEvent::SourceLost`] when the daemon disappears. Never
/// returns an error to the caller - a missing source degrades the record,
/// never the daemon.
///
/// `reseed` is signalled by the main loop's periodic tick: each notification
/// re-runs the `ListGames` seed loop so state wiped mid-session (e.g. groups
/// dropped after a transient SourceLost) is rebuilt without a restart.
pub async fn watch(conn: Connection, tx: mpsc::Sender<SourceEvent>, reseed: Arc<Notify>) {
    let fdo = match fdo::DBusProxy::new(&conn).await {
        Ok(proxy) => proxy,
        Err(e) => {
            warn!(error = %e, "Failed to create org.freedesktop.DBus proxy; GameMode source disabled");
            return;
        }
    };

    // Filtered stream: only owner changes for the GameMode name.
    let mut owner_changes = match fdo
        .receive_name_owner_changed_with_args(&[(0, GAMEMODE_NAME)])
        .await
    {
        Ok(stream) => stream,
        Err(e) => {
            warn!(error = %e, "Failed to subscribe to NameOwnerChanged; GameMode source disabled");
            return;
        }
    };

    loop {
        match session(&conn, &tx, &mut owner_changes, &reseed).await {
            Ok(()) => {
                info!("GameMode session ended: gamemoded left the bus");
            }
            Err(e) => {
                warn!(error = %e, "GameMode session failed");
                if !name_is_owned(&fdo).await {
                    debug!("gamemoded not on the bus; waiting for it to appear");
                    wait_for_owner(&mut owner_changes).await;
                } else {
                    // Name still owned but the session broke: brief pause, then retry.
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        }
    }
}

/// One connected session with gamemoded.
///
/// Returns `Ok(())` when the daemon cleanly left the bus (SourceLost was
/// emitted), `Err` on any failure (including activation failure at seed time).
async fn session(
    conn: &Connection,
    tx: &mpsc::Sender<SourceEvent>,
    owner_changes: &mut fdo::NameOwnerChangedStream<'_>,
    reseed: &Notify,
) -> zbus::Result<()> {
    let proxy = GameModeProxy::new(conn).await?;

    // Subscribe to signals BEFORE seeding so a registration arriving between
    // the two is buffered rather than lost. The resulting duplicate from the
    // seed is harmless: the manager's add is idempotent.
    let mut registered = proxy.receive_game_registered().await?;
    let mut unregistered = proxy.receive_game_unregistered().await?;

    // Seed current games. This call also triggers D-Bus activation of
    // gamemoded if the service is installed but not running.
    let count = seed_games(conn, &proxy, tx).await?;
    info!(count, "Seeded games from GameMode");

    loop {
        tokio::select! {
            signal = registered.next() => {
                match signal {
                    Some(signal) => match signal.args() {
                        Ok(args) => {
                            let (executable, since) = resolve_game(conn, args.pid, &args.game).await;
                            send(
                                tx,
                                SourceEvent::Updated(Box::new(Activity::from_gamemode(
                                    args.pid,
                                    &executable,
                                    since,
                                ))),
                            )
                            .await;
                        }
                        Err(e) => warn!(error = %e, "Failed to parse GameRegistered signal"),
                    },
                    None => return Ok(()), // stream closed: treat as name loss
                }
            }
            signal = unregistered.next() => {
                match signal {
                    Some(signal) => match signal.args() {
                        Ok(args) => send(
                            tx,
                            SourceEvent::Removed {
                                id: format!("pid_{}", args.pid),
                                source: Source::GameMode,
                            },
                        )
                        .await,
                        Err(e) => warn!(error = %e, "Failed to parse GameUnregistered signal"),
                    },
                    None => return Ok(()),
                }
            }
            change = owner_changes.next() => {
                match change {
                    Some(change) => match change.args() {
                        Ok(args) => {
                            if args.new_owner().is_none() {
                                // gamemoded left the bus: all gamemode-sourced records die.
                                send(
                                    tx,
                                    SourceEvent::SourceLost {
                                        source: Source::GameMode,
                                    },
                                )
                                .await;
                                return Ok(());
                            }
                            // new_owner present: name (re)appeared; only relevant
                            // while waiting, handled by wait_for_owner.
                        }
                        Err(e) => warn!(error = %e, "Failed to parse NameOwnerChanged signal"),
                    },
                    None => return Ok(()),
                }
            }
            _ = reseed.notified() => {
                // Tick-driven reseed: re-emit every registered pid through the
                // normal path. Failure is non-fatal - the next tick retries,
                // and until then behaviour degrades to the pre-reseed daemon.
                match seed_games(conn, &proxy, tx).await {
                    Ok(count) => debug!(count, "Reseeded games from GameMode"),
                    Err(e) => debug!(error = %e, "GameMode reseed failed"),
                }
            }
        }
    }
}

/// Check whether the GameMode name currently has an owner on the bus.
async fn name_is_owned(fdo: &fdo::DBusProxy<'_>) -> bool {
    match fdo
        .name_has_owner(GAMEMODE_NAME.try_into().expect("valid bus name"))
        .await
    {
        Ok(owned) => owned,
        Err(e) => {
            debug!(error = %e, "NameHasOwner failed");
            false
        }
    }
}

/// Block until the GameMode name gains an owner.
async fn wait_for_owner(stream: &mut fdo::NameOwnerChangedStream<'_>) {
    while let Some(change) = stream.next().await {
        match change.args() {
            Ok(args) => {
                if args.new_owner().is_some() {
                    info!("gamemoded appeared on the bus");
                    return;
                }
            }
            Err(e) => warn!(error = %e, "Failed to parse NameOwnerChanged signal"),
        }
    }
}

/// Send an event, logging if the core has gone away.
async fn send(tx: &mpsc::Sender<SourceEvent>, event: SourceEvent) {
    if let Err(e) = tx.send(event).await {
        warn!(error = %e, "Failed to forward GameMode event to daemon core");
    }
}
