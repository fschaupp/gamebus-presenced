//! Discord IPC source: standalone mode when no Discord client runs,
//! transparent proxy to a real Discord upstream when one does.

pub mod protocol;
mod proxy;

use crate::dbus::types::{Activity, Source};
use crate::sources::SourceEvent;
use protocol::{Frame, Handshake, PacketType, CONNECTION_RESPONSE};
use rsrpc::cmd::ActivityCmd;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Run the standalone Discord IPC listener.
pub async fn listen(tx: mpsc::Sender<SourceEvent>) {
    let path = socket_path();
    let listener = match bind(&path).await {
        Ok(Some(listener)) => listener,
        Ok(None) => return,
        Err(e) => {
            warn!(error = %e, path = %path.display(), "Discord IPC source disabled");
            return;
        }
    };
    let _guard = SocketGuard(path.clone());
    info!(path = %path.display(), "Discord IPC listener ready");

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, tx).await {
                        debug!(error = %e, "Discord IPC connection ended");
                    }
                });
            }
            Err(e) => {
                warn!(error = %e, "Discord IPC accept failed");
                let _ = tx
                    .send(SourceEvent::SourceLost {
                        source: Source::Discord,
                    })
                    .await;
                return;
            }
        }
    }
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn socket_path() -> PathBuf {
    runtime_dir().join("discord-ipc-0")
}

async fn bind(path: &Path) -> std::io::Result<Option<UnixListener>> {
    if path.exists() {
        match UnixStream::connect(path).await {
            Ok(_) => {
                warn!(path = %path.display(), "Discord IPC socket already has a live owner");
                return Ok(None);
            }
            Err(_) => {
                info!(path = %path.display(), "Removing stale Discord IPC socket");
                std::fs::remove_file(path)?;
            }
        }
    }
    UnixListener::bind(path).map(Some)
}

async fn handle_connection(
    mut stream: UnixStream,
    tx: mpsc::Sender<SourceEvent>,
) -> std::io::Result<()> {
    let pid = peer_pid(&stream)?;

    // With a real Discord upstream running, proxy verbatim and only tap
    // the traffic. Without one, we answer the protocol ourselves.
    if let Some(upstream) = proxy::find_upstream().await {
        return proxy::pump(stream, upstream, tx, pid).await;
    }

    let id = format!("discord_{pid}");
    let mut client_id = String::new();
    let mut handshaken = false;
    let mut asserted_activity = false;
    info!(pid, "Discord IPC client connected (standalone mode)");

    loop {
        let frame = match Frame::read_from(&mut stream).await {
            Ok(frame) => frame,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        };

        match frame.packet_type {
            PacketType::Handshake => {
                let handshake: Handshake = serde_json::from_slice(&frame.payload)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                if handshake.v != 1 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unsupported Discord IPC version {}", handshake.v),
                    ));
                }
                client_id = handshake.client_id;
                handshaken = true;
                write_frame(
                    &mut stream,
                    &Frame::new(PacketType::Frame, CONNECTION_RESPONSE.as_bytes().to_vec()),
                )
                .await?;
            }
            PacketType::Frame => {
                if !handshaken {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Discord IPC frame before handshake",
                    ));
                }
                let mut command: ActivityCmd = serde_json::from_slice(&frame.payload)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                if command.cmd == "SET_ACTIVITY" {
                    handle_set_activity(&mut command, pid, &client_id, &mut asserted_activity, &tx)
                        .await?;
                }
                // Discord IPC is lock-step: every command is echoed as its response.
                write_frame(&mut stream, &frame).await?;
            }
            PacketType::Close => break,
            PacketType::Ping => {
                write_frame(&mut stream, &Frame::new(PacketType::Pong, frame.payload)).await?;
            }
            PacketType::Pong => {}
        }
    }

    if asserted_activity {
        remove(&tx, &id).await?;
    }
    info!(pid, "Discord IPC client disconnected");
    Ok(())
}

/// Turn a parsed SET_ACTIVITY command into source events. Shared by the
/// standalone server and the proxy's passive tap. rsRPC's `fix()` normalises
/// timestamps to milliseconds and brings buttons/flags in line with what
/// Discord-compatible servers emit.
async fn handle_set_activity(
    command: &mut ActivityCmd,
    pid: i32,
    client_id: &str,
    asserted: &mut bool,
    tx: &mpsc::Sender<SourceEvent>,
) -> std::io::Result<()> {
    command.fix();
    match command
        .args
        .as_ref()
        .and_then(|args| args.activity.as_ref())
    {
        Some(payload) => {
            let activity = Activity::from_discord(pid, client_id, payload);
            tx.send(SourceEvent::Updated(Box::new(activity)))
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "daemon core stopped")
                })?;
            *asserted = true;
        }
        None => {
            remove(tx, &format!("discord_{pid}")).await?;
            *asserted = false;
        }
    }
    Ok(())
}

async fn write_frame(stream: &mut UnixStream, frame: &Frame) -> std::io::Result<()> {
    stream.write_all(&frame.encode()).await
}

async fn remove(tx: &mpsc::Sender<SourceEvent>, id: &str) -> std::io::Result<()> {
    tx.send(SourceEvent::Removed {
        id: id.to_string(),
        source: Source::Discord,
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "daemon core stopped"))
}

fn peer_pid(stream: &UnixStream) -> std::io::Result<i32> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred and len point to valid writable storage of the exact sizes
    // required by SO_PEERCRED, and stream owns a live Unix socket fd.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if result == 0 {
        Ok(cred.pid)
    } else {
        Err(std::io::Error::last_os_error())
    }
}
