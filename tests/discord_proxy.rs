//! Integration test for the transparent proxy mode.
//!
//! A fixture "real Discord" (an in-process unix socket server that records
//! every byte) is bound at `discord-ipc-1` inside a temporary
//! `XDG_RUNTIME_DIR`. The daemon under test is spawned with that runtime dir,
//! binds `discord-ipc-0`, and must proxy a raw IPC client through to the
//! fixture:
//!
//! - bytes the client sends arrive at the fixture unchanged,
//! - bytes the fixture sends arrive at the client unchanged,
//! - SET_ACTIVITY is still tapped onto the session bus,
//! - when the fixture dies, the client connection is closed and a reconnect
//!   gets the standalone behaviour.
//!
//! Skips gracefully when there is no session bus.

use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use zbus::zvariant::OwnedObjectPath;
use zbus::{fdo, proxy, Connection};

const GAMEBUS_NAME: &str = "org.gamebus.Presence.v1";
const WAIT: Duration = Duration::from_secs(10);
const CLIENT_ID: &str = "fixture-client-7";

#[proxy(
    interface = "org.gamebus.Presence.v1.Manager",
    default_service = "org.gamebus.Presence.v1",
    default_path = "/org/gamebus/Presence/v1"
)]
trait Manager {
    fn list_activities(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[proxy(
    interface = "org.gamebus.Presence.v1.Activity",
    default_service = "org.gamebus.Presence.v1",
    assume_defaults = false
)]
trait ActivityProps {
    #[zbus(property)]
    fn name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn details(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn process_id(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn app_ids(&self) -> zbus::Result<std::collections::HashMap<String, String>>;
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The fixture "Discord": records raw bytes received and answers each frame
/// with a deterministic response (canned READY for handshakes, echo for
/// everything else - mirroring Discord's lock-step shape).
struct Fixture {
    received: Arc<Mutex<Vec<u8>>>,
    sent: Arc<Mutex<Vec<u8>>>,
    shutdown: watch::Sender<bool>,
    connections: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    dir: std::path::PathBuf,
}

impl Fixture {
    async fn start(dir: std::path::PathBuf) -> Self {
        let path = dir.join("discord-ipc-1");
        let listener = UnixListener::bind(&path).expect("fixture bind failed");
        let received = Arc::new(Mutex::new(Vec::new()));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let connections: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let (shutdown, mut stop) = watch::channel(false);
        let fixture = Self {
            received: received.clone(),
            sent: sent.clone(),
            shutdown,
            connections: connections.clone(),
            dir,
        };
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept = listener.accept() => {
                        match accept {
                            Ok((mut stream, _)) => {
                                let received = received.clone();
                                let sent = sent.clone();
                                let handle = tokio::spawn(async move {
                                    serve(&mut stream, received, sent).await;
                                });
                                connections.lock().unwrap().push(handle);
                            }
                            Err(_) => break,
                        }
                    }
                    _ = stop.changed() => break,
                }
            }
        });
        fixture
    }

    /// Stop accepting, close in-flight connections (simulating Discord
    /// exiting), and remove the socket so later probes find no upstream.
    async fn stop(self) {
        let _ = self.shutdown.send(true);
        for handle in self.connections.lock().unwrap().drain(..) {
            handle.abort();
        }
        let _ = std::fs::remove_file(self.dir.join("discord-ipc-1"));
    }

    fn received(&self) -> Vec<u8> {
        self.received.lock().unwrap().clone()
    }

    fn sent(&self) -> Vec<u8> {
        self.sent.lock().unwrap().clone()
    }
}

const READY_RESPONSE: &str = r#"{"cmd":"DISPATCH","evt":"READY","data":{"v":1}}"#;

async fn serve(stream: &mut UnixStream, received: Arc<Mutex<Vec<u8>>>, sent: Arc<Mutex<Vec<u8>>>) {
    loop {
        let mut header = [0u8; 8];
        if stream.read_exact(&mut header).await.is_err() {
            return;
        }
        let opcode = u32::from_le_bytes(header[0..4].try_into().unwrap());
        let len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let mut payload = vec![0u8; len];
        if stream.read_exact(&mut payload).await.is_err() {
            return;
        }
        received.lock().unwrap().extend_from_slice(&header);
        received.lock().unwrap().extend_from_slice(&payload);

        let (resp_op, resp_payload) = if opcode == 0 {
            (1u32, READY_RESPONSE.as_bytes().to_vec())
        } else {
            (opcode, payload)
        };
        let mut response = Vec::with_capacity(8 + resp_payload.len());
        response.extend_from_slice(&resp_op.to_le_bytes());
        response.extend_from_slice(&(resp_payload.len() as u32).to_le_bytes());
        response.extend_from_slice(&resp_payload);
        sent.lock().unwrap().extend_from_slice(&response);
        if stream.write_all(&response).await.is_err() {
            return;
        }
    }
}

fn frame(opcode: u32, payload: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + payload.len());
    bytes.extend_from_slice(&opcode.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(payload.as_bytes());
    bytes
}

async fn wait_for_name(conn: &Connection, name: &str, timeout: Duration) -> bool {
    let fdo = match fdo::DBusProxy::new(conn).await {
        Ok(p) => p,
        Err(_) => return false,
    };
    let name: zbus::names::BusName = match name.try_into() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if fdo.name_has_owner(name.clone()).await.unwrap_or(false) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_forwards_verbatim_and_taps_activity() {
    let conn = match Connection::session().await {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("SKIP: no session bus: {e}");
            return;
        }
    };

    // Isolated runtime dir: our own discord-ipc-0/1, no interference with the
    // real session's sockets.
    let dir = std::env::temp_dir().join(format!("gamebus-proxy-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let fixture = Fixture::start(dir.clone()).await;

    let daemon_log = std::env::temp_dir().join("gamebus-presenced-test-proxy.log");
    let daemon_log_out = std::fs::File::create(&daemon_log).unwrap();
    let daemon_log_err = daemon_log_out.try_clone().unwrap();
    let daemon = Command::new(env!("CARGO_BIN_EXE_gamebus-presenced"))
        .stdout(Stdio::from(daemon_log_out))
        .stderr(Stdio::from(daemon_log_err))
        .env("RUST_LOG", "debug")
        .env("XDG_RUNTIME_DIR", &dir)
        // The bus address is explicit on this machine, so overriding
        // XDG_RUNTIME_DIR does not cut the daemon off the session bus.
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
        )
        .spawn()
        .expect("failed to spawn gamebus-presenced");
    let mut daemon = ChildGuard(daemon);

    assert!(
        wait_for_name(&conn, GAMEBUS_NAME, Duration::from_secs(5)).await,
        "daemon did not acquire its bus name"
    );
    let sock = dir.join("discord-ipc-0");
    let start = std::time::Instant::now();
    while !sock.exists() && start.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(sock.exists(), "daemon did not bind {}", sock.display());

    // Raw client: handcrafted frames, so the exact bytes on both sides are
    // known and byte-identity can be asserted.
    let handshake = frame(0, &format!(r#"{{"v":1,"client_id":"{CLIENT_ID}"}}"#));
    let set_activity = frame(
        1,
        r#"{"cmd":"SET_ACTIVITY","args":{"pid":1,"activity":{"name":"Proxied Game","details":"Through The Wire","type":0,"timestamps":{"start":1700000000000}}},"nonce":"n-1"}"#,
    );
    let mut sent_by_client = handshake.clone();
    sent_by_client.extend_from_slice(&set_activity);

    let mut client = UnixStream::connect(&sock).await.unwrap();
    client.write_all(&handshake).await.unwrap();

    // Read the proxied READY response (8-byte header + payload).
    let mut response_head = [0u8; 8];
    client.read_exact(&mut response_head).await.unwrap();
    let resp_len = u32::from_le_bytes(response_head[4..8].try_into().unwrap()) as usize;
    let mut response_payload = vec![0u8; resp_len];
    client.read_exact(&mut response_payload).await.unwrap();
    let mut received_by_client = response_head.to_vec();
    received_by_client.extend_from_slice(&response_payload);

    client.write_all(&set_activity).await.unwrap();
    let mut echo_head = [0u8; 8];
    client.read_exact(&mut echo_head).await.unwrap();
    let echo_len = u32::from_le_bytes(echo_head[4..8].try_into().unwrap()) as usize;
    let mut echo_payload = vec![0u8; echo_len];
    client.read_exact(&mut echo_payload).await.unwrap();
    received_by_client.extend_from_slice(&echo_head);
    received_by_client.extend_from_slice(&echo_payload);

    // Byte-identity in both directions.
    let start = std::time::Instant::now();
    while fixture.received().len() < sent_by_client.len() && start.elapsed() < WAIT {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        fixture.received(),
        sent_by_client,
        "client->upstream stream was not forwarded verbatim"
    );
    assert_eq!(
        received_by_client,
        fixture.sent(),
        "upstream->client stream was not forwarded verbatim"
    );

    // The tap: SET_ACTIVITY must appear on the session bus even in proxy mode.
    let manager = ManagerProxy::new(&conn).await.unwrap();
    let test_pid = std::process::id();
    let expected_path = format!("/org/gamebus/Presence/v1/Activity/discord_{test_pid}");
    let start = std::time::Instant::now();
    let mut found = false;
    while start.elapsed() < WAIT {
        let list = manager.list_activities().await.unwrap_or_default();
        if list.iter().any(|p| p.as_str() == expected_path) {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        found,
        "tapped activity {expected_path} did not appear on bus"
    );
    let activity = ActivityPropsProxy::builder(&conn)
        .path(expected_path.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    assert_eq!(activity.name().await.unwrap(), "Proxied Game");
    assert_eq!(activity.details().await.unwrap(), "Through The Wire");
    assert_eq!(activity.process_id().await.unwrap(), test_pid);
    assert_eq!(
        activity.app_ids().await.unwrap().get("discord").unwrap(),
        CLIENT_ID
    );

    // Upstream loss: the fixture dies, the proxied connection must be closed
    // by the daemon (client sees EOF)...
    fixture.stop().await;
    let mut buf = [0u8; 8];
    let eof = tokio::time::timeout(WAIT, client.read_exact(&mut buf)).await;
    match eof {
        Ok(Err(e)) => assert_eq!(
            e.kind(),
            std::io::ErrorKind::UnexpectedEof,
            "expected EOF after upstream loss, got {e}"
        ),
        Ok(Ok(_)) => panic!("expected EOF after upstream loss, read succeeded"),
        Err(_) => panic!("connection was not closed after upstream loss"),
    }
    drop(client);

    // ...and a reconnect lands in standalone mode: our own READY answers the
    // handshake instead of the fixture's.
    let mut client = UnixStream::connect(&sock).await.unwrap();
    client.write_all(&handshake).await.unwrap();
    let mut head = [0u8; 8];
    tokio::time::timeout(WAIT, client.read_exact(&mut head))
        .await
        .expect("no standalone handshake response")
        .unwrap();
    let len = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
    let mut payload = vec![0u8; len];
    client.read_exact(&mut payload).await.unwrap();
    let payload = String::from_utf8(payload).unwrap();
    assert!(
        payload.contains("gamebus-presenced"),
        "expected our standalone READY after upstream loss, got: {payload}"
    );

    drop(client);
    let _ = daemon.0.kill();
    let _ = daemon.0.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
