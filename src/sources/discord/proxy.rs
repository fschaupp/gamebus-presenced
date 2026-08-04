//! Transparent proxy mode: when a real Discord client holds a socket
//! further down the range (`discord-ipc-1..=9`, since we bound `ipc-0` first),
//! client connections are forwarded to it verbatim and we observe the traffic.
//!
//! Rules:
//! - Frames are forwarded whole - never split a write.
//! - Responses come from upstream; the standalone handshake/echo path only
//!   runs when there is no upstream.
//! - Upstream loss mid-connection closes the client connection. Reconnect
//!   behaviour is the client's choice; a reconnect lands in standalone mode.
//! - Tapping traffic is passive: parse problems are logged, never propagated.

use super::protocol::{Frame, PacketType};
use super::{handle_set_activity, runtime_dir};
use crate::sources::SourceEvent;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::{debug, info};

/// Highest `discord-ipc-N` candidate probed for an upstream Discord.
const MAX_UPSTREAM_INDEX: u32 = 9;

/// Connect to a running Discord's IPC socket, if one exists.
///
/// Discord walks `discord-ipc-0..=` trying to bind, so a real client that
/// started after us sits on `ipc-1` (or further if several run). We hold
/// `ipc-0`, so candidates start at 1. Stale sockets fail to connect and are
/// skipped; the first connectable candidate wins.
pub async fn find_upstream() -> Option<UnixStream> {
    let dir = runtime_dir();
    for n in 1..=MAX_UPSTREAM_INDEX {
        let candidate: PathBuf = dir.join(format!("discord-ipc-{n}"));
        if !candidate.exists() {
            debug!(path = %candidate.display(), "Upstream candidate does not exist");
            continue;
        }
        match UnixStream::connect(&candidate).await {
            Ok(stream) => {
                info!(path = %candidate.display(), "Discord upstream found; proxying");
                return Some(stream);
            }
            Err(e) => {
                debug!(error = %e, path = %candidate.display(), "Upstream candidate not connectable");
            }
        }
    }
    debug!("No Discord upstream found; standalone mode");
    None
}

/// Pump frames between a game client and the real Discord, verbatim, while
/// tapping `SET_ACTIVITY` for our own activity tracking.
///
/// Returns when either side closes. Upstream loss closes the client
/// connection implicitly: both streams are dropped here, the client sees EOF,
/// and a reconnect is re-evaluated (standalone if Discord stayed away).
pub async fn pump(
    mut client: UnixStream,
    mut upstream: UnixStream,
    tx: mpsc::Sender<SourceEvent>,
    pid: i32,
) -> std::io::Result<()> {
    let id = format!("discord_{pid}");
    let mut client_id = String::new();
    let mut asserted = false;
    info!(pid, "Proxying Discord IPC client to upstream");

    let result = pump_loop(
        &mut client,
        &mut upstream,
        &tx,
        pid,
        &mut client_id,
        &mut asserted,
    )
    .await;

    if asserted {
        // The upstream never sees our cleanup and the client is gone; drop
        // the record ourselves, same as standalone mode does.
        super::remove(&tx, &id).await?;
    }
    info!(pid, "Discord IPC proxied connection ended");
    result
}

async fn pump_loop(
    client: &mut UnixStream,
    upstream: &mut UnixStream,
    tx: &mpsc::Sender<SourceEvent>,
    pid: i32,
    client_id: &mut String,
    asserted: &mut bool,
) -> std::io::Result<()> {
    loop {
        tokio::select! {
            frame = Frame::read_from(client) => {
                match frame {
                    Ok(frame) => {
                        tap(&frame, pid, client_id, asserted, tx).await?;
                        // encode() reproduces the exact wire bytes: the header
                        // is derived from opcode + payload length, so a parsed
                        // frame re-serialises identically to what was read.
                        upstream.write_all(&frame.encode()).await?;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
            frame = Frame::read_from(upstream) => {
                match frame {
                    Ok(frame) => client.write_all(&frame.encode()).await?,
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        info!(pid, "Discord upstream closed; dropping client connection");
                        return Ok(());
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
}

/// Passively observe one client-bound frame: extract the handshake's
/// `client_id` and turn `SET_ACTIVITY` payloads into source events.
/// Best-effort - a malformed frame must not break the proxy.
async fn tap(
    frame: &Frame,
    pid: i32,
    client_id: &mut String,
    asserted: &mut bool,
    tx: &mpsc::Sender<SourceEvent>,
) -> std::io::Result<()> {
    match frame.packet_type {
        PacketType::Handshake => {
            match serde_json::from_slice::<super::protocol::Handshake>(&frame.payload) {
                Ok(handshake) => *client_id = handshake.client_id,
                Err(e) => debug!(error = %e, pid, "Unparseable handshake tapped"),
            }
        }
        PacketType::Frame => {
            match serde_json::from_slice::<rsrpc::cmd::ActivityCmd>(&frame.payload) {
                Ok(mut command) if command.cmd == "SET_ACTIVITY" => {
                    handle_set_activity(&mut command, pid, client_id, asserted, tx).await?;
                }
                Ok(_) => {}
                Err(e) => debug!(error = %e, pid, "Unparseable command frame tapped"),
            }
        }
        _ => {}
    }
    Ok(())
}
