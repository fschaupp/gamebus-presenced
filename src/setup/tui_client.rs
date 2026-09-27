//! The TUI as a client of its own MCP server.
//!
//! The TUI reads and edits the stash the way every other front-end does:
//! through `gamebus-setup mcp`, spawned as a child over stdio, with its own
//! client key. So each change to the MCP surface is exercised here first,
//! and every TUI edit is attributed and ledgered like any client's.
//!
//! The key lives in `$XDG_CONFIG_HOME/gamebus-presenced/tui-client.key`,
//! created 0600. It identifies this TUI to the ledger; it is not a secret
//! from anything running as this user.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Mutex;

use ed25519_dalek::SigningKey;
use serde_json::Value;

use super::mcp::{CallError, Client, ServerSpec};

/// The name this TUI registers under.
pub const CLIENT_NAME: &str = "gamebus-tui";

/// One connection: started on first use, restarted after a transport fault,
/// authenticated once before its first edit.
pub struct Session {
    spec: ServerSpec,
    key: SigningKey,
    client: Option<Client>,
    authenticated: bool,
}

impl Session {
    pub fn new(spec: ServerSpec, key: SigningKey) -> Self {
        Self {
            spec,
            key,
            client: None,
            authenticated: false,
        }
    }

    fn client(&mut self) -> Result<&mut Client, CallError> {
        if self.client.is_none() {
            let client =
                Client::start_as(self.spec.clone(), CLIENT_NAME).map_err(CallError::Transport)?;
            self.client = Some(client);
            self.authenticated = false;
        }
        Ok(self.client.as_mut().expect("just started"))
    }

    /// One call, retried once on a fresh connection when the transport
    /// failed: a server that died between two keypresses is not the user's
    /// problem.
    fn call(&mut self, tool: &str, args: &Value, edit: bool) -> Result<Value, CallError> {
        for attempt in 0..2 {
            let result = self.try_call(tool, args, edit);
            match result {
                Err(CallError::Transport(_)) if attempt == 0 => {
                    self.client = None;
                }
                other => return other,
            }
        }
        unreachable!("the second attempt returns")
    }

    fn try_call(&mut self, tool: &str, args: &Value, edit: bool) -> Result<Value, CallError> {
        if edit && !self.authenticated {
            let key = self.key.clone();
            self.client()?.authenticate(CLIENT_NAME, &key)?;
            self.authenticated = true;
        }
        self.client()?.call_tool(tool, args.clone())
    }

    pub fn read(&mut self, tool: &str, args: Value) -> Result<Value, CallError> {
        self.call(tool, &args, false)
    }

    pub fn edit(&mut self, tool: &str, args: Value) -> Result<Value, CallError> {
        self.call(tool, &args, true)
    }

    /// Drop the connection, so the next call starts (and authenticates)
    /// afresh. After an owner action changes who is approved, or after
    /// `auth init`, a cached refusal must not linger.
    pub fn reset(&mut self) {
        self.client = None;
        self.authenticated = false;
    }
}

/// This binary serving MCP with edits enabled. The child inherits our
/// environment, so it sees the same stash and ledger.
fn own_server() -> Result<ServerSpec, String> {
    let exe =
        std::env::current_exe().map_err(|e| format!("cannot find gamebus-setup itself: {e}"))?;
    Ok(ServerSpec {
        name: "gamebus-setup mcp".into(),
        command: exe.display().to_string(),
        args: vec!["mcp".into(), "--allow-edits".into()],
        env: Vec::new(),
    })
}

fn key_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("gamebus-presenced").join("tui-client.key"))
}

/// Load the TUI's key, creating it on first use. Created with mode 0600 in
/// the same call rather than tightened afterwards, so it is never readable
/// by others even for an instant.
pub fn load_or_create_key(path: &std::path::Path) -> Result<SigningKey, String> {
    if let Ok(raw) = std::fs::read_to_string(path) {
        let seed: [u8; 32] = super::auth::unhex(raw.trim())
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| {
                format!(
                    "{} is not a key; delete it to make a new one",
                    path.display()
                )
            })?;
        return Ok(SigningKey::from_bytes(&seed));
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("randomness: {e}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    writeln!(f, "{}", super::auth::hex(&seed)).map_err(|e| e.to_string())?;
    Ok(SigningKey::from_bytes(&seed))
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// Run `f` on the TUI's shared session, starting it on first use.
pub fn with_session<R>(
    f: impl FnOnce(&mut Session) -> Result<R, CallError>,
) -> Result<R, CallError> {
    let mut guard = SESSION.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        let path = key_path()
            .ok_or_else(|| CallError::Transport("no config directory (no HOME)".into()))?;
        let key = load_or_create_key(&path).map_err(CallError::Transport)?;
        let spec = own_server().map_err(CallError::Transport)?;
        *guard = Some(Session::new(spec, key));
    }
    f(guard.as_mut().expect("just set"))
}

/// Forget the shared connection (see [`Session::reset`]).
pub fn reset() {
    if let Some(s) = SESSION.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        s.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// The whole path the TUI takes: spawn gamebus-setup mcp, authenticate,
    /// edit, and find the edit in the ledger under gamebus-tui. Against the
    /// built binary and a scratch home; skipped when the binary is absent.
    #[test]
    fn the_tui_edits_through_its_own_server_and_is_ledgered_as_gamebus_tui() {
        let bin =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/gamebus-setup");
        if !bin.exists() {
            return;
        }
        let home = std::env::temp_dir().join(format!("gamebus-tui-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let data = home.join("data");
        std::fs::create_dir_all(data.join("gamebus-presenced")).unwrap();
        std::fs::write(
            data.join("gamebus-presenced/umu-misses.json"),
            r#"{"lutris:x": {"title": "X", "store": "gog", "codename": "1", "umu_id": "umu-default",
                "title_source": "stem", "confidence": "low", "executable": null,
                "first_seen": "2026-09-27", "last_seen": "2026-09-27"}}"#,
        )
        .unwrap();
        let auth = super::super::auth::AuthPaths {
            dir: data.join("gamebus-presenced/auth"),
        };
        let spec = ServerSpec {
            name: "test".into(),
            command: bin.display().to_string(),
            args: vec!["mcp".into(), "--allow-edits".into()],
            env: vec![
                ("XDG_DATA_HOME".into(), data.display().to_string()),
                ("HOME".into(), home.display().to_string()),
            ],
        };
        let mut session = Session::new(spec, SigningKey::from_bytes(&[5; 32]));

        // Before auth init the refusal is typed, so the TUI can name its cure.
        let args = serde_json::json!({"key": "lutris:x", "verb": {"verb": "dismiss"}});
        match session.edit("apply_miss", args.clone()) {
            Err(CallError::Refused(r)) => {
                assert_eq!(r.reason, gamebus_coupler::ErrorReason::AuthUninitialized)
            }
            other => panic!("{other:?}"),
        }

        super::super::auth::fast_init(&auth, "pw");
        session.reset();
        let answer = session.edit("apply_miss", args).unwrap();
        assert_eq!(answer["change"]["change"], "dismissed");
        let rows = session.read("list_misses", serde_json::json!({})).unwrap();
        assert!(rows["misses"][0]["dismissed"].is_string());

        let entries = super::super::auth::entries(&auth).unwrap();
        let names: Vec<_> = entries
            .iter()
            .filter_map(|(e, _)| e.actor.name.clone())
            .collect();
        assert_eq!(
            names,
            [CLIENT_NAME, CLIENT_NAME],
            "registered, then the edit"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_key_is_created_private_and_read_back_the_same() {
        let dir = std::env::temp_dir().join(format!("gamebus-tui-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("tui-client.key");
        let first = load_or_create_key(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let again = load_or_create_key(&path).unwrap();
        assert_eq!(first.to_bytes(), again.to_bytes());
        std::fs::write(&path, "garbage").unwrap();
        assert!(
            load_or_create_key(&path).is_err(),
            "a damaged key is reported, not replaced"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
