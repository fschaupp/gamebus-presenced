//! A minimal MCP client: enough of the protocol to read another tool's
//! structured output, and nothing more.
//!
//! The point is leverage. beisl already decides what counts as a compat wall
//! and publishes it over MCP as structured JSON; re-deriving that here would
//! be a second implementation of someone else's expertise, drifting from the
//! day it was written. So this tool speaks to beisl's own server and takes
//! its answers.
//!
//! No LLM is involved. MCP is used purely as the machine-readable interface
//! it already is: spawn the server, `initialize`, `tools/call`, read the JSON
//! back. The transport is newline-delimited JSON-RPC over the child's stdio -
//! no ports, no sockets, no network.
//!
//! # Why a [`ServerSpec`] rather than a hardcoded `beisl-mcp`
//!
//! Detection and report generation are exactly the kind of thing that gets
//! more sophisticated over time, and past some point hardcoding every source
//! stops being sensible. A server is therefore named data - a command and its
//! arguments - not a compiled-in constant, so registering further servers
//! later is a config surface rather than a rewrite. Only beisl is wired up
//! today ([`ServerSpec::beisl`]); the registry is future work.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use serde_json::{json, Value};

/// The protocol version this client asks for. beisl echoes back whatever it
/// is given, and every server in practice accepts a dated version string.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// How long to wait for one response. A server that has wedged must not wedge
/// gamebus-setup with it: the scan is a convenience, never the only way to
/// get a finding in (`compat --record` is the push path).
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How to start one MCP server.
#[derive(Debug, Clone)]
pub struct ServerSpec {
    /// How this server is referred to in messages, and the `source` recorded
    /// on the observations it produces.
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

impl ServerSpec {
    /// beisl's server. `GAMEBUS_BEISL_MCP` overrides the command, which is
    /// how a developer points at a build tree without installing.
    pub fn beisl() -> Self {
        let command = std::env::var("GAMEBUS_BEISL_MCP").unwrap_or_else(|_| "beisl-mcp".into());
        Self {
            name: "beisl".into(),
            command,
            args: Vec::new(),
        }
    }
}

/// A running server, shut down when this is dropped.
pub struct Client {
    spec: ServerSpec,
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl Client {
    /// Start the server and complete the handshake.
    ///
    /// The child's stderr is inherited rather than captured: a server that
    /// complains should complain where the user can see it, and this tool has
    /// no business swallowing another program's diagnostics.
    pub fn start(spec: ServerSpec) -> Result<Self, String> {
        let mut child = Command::new(&spec.command)
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", spec.command))?;

        let stdin = child.stdin.take().ok_or("no stdin on the server")?;
        let stdout = child.stdout.take().ok_or("no stdout on the server")?;

        // A reader thread so a silent server times out instead of blocking
        // the process forever on a read that never returns.
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break; // client dropped
                }
            }
        });

        let mut client = Self {
            spec,
            child,
            stdin,
            lines,
            next_id: 0,
        };
        client.handshake()?;
        Ok(client)
    }

    fn handshake(&mut self) -> Result<(), String> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "gamebus-setup", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        // A notification: no id, and by definition no response to wait for.
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }))?;
        Ok(())
    }

    /// Call a tool and return its text payload parsed as JSON.
    ///
    /// MCP wraps every result in a content envelope; the servers this reads
    /// put one JSON document in one text block, so anything else is a
    /// protocol surprise worth reporting rather than papering over.
    pub fn call_json(&mut self, tool: &str, args: Value) -> Result<Value, String> {
        let result = self.request("tools/call", json!({ "name": tool, "arguments": args }))?;
        let text = tool_text(&result).map_err(|e| format!("{tool}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("{tool}: result is not JSON: {e}"))
    }

    /// The tool names this server offers, for a capability check before use.
    pub fn tool_names(&mut self) -> Result<Vec<String>, String> {
        let result = self.request("tools/list", json!({}))?;
        Ok(result
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|t| t.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn name(&self) -> &str {
        &self.spec.name
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;

        // Skip anything that is not the response to this id: a server may
        // interleave notifications of its own, which are not ours to read.
        loop {
            let line = match self.lines.recv_timeout(CALL_TIMEOUT) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "{} did not answer {method} in time",
                        self.spec.name
                    ))
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!(
                        "{} exited before answering {method}",
                        self.spec.name
                    ))
                }
            };
            match response_for(&line, id) {
                Some(result) => return result,
                None => continue,
            }
        }
    }

    fn send(&mut self, msg: &Value) -> Result<(), String> {
        writeln!(self.stdin, "{msg}")
            .map_err(|e| format!("cannot write to {}: {e}", self.spec.name))?;
        self.stdin
            .flush()
            .map_err(|e| format!("cannot flush to {}: {e}", self.spec.name))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Closing stdin is the documented shutdown (the server pumps until
        // EOF); the kill is only for one that ignores it.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Match one transport line against a pending request id.
///
/// `None` means "not ours, keep reading" - a notification, another id, or a
/// line that is not JSON at all. `Some` carries the result or the server's
/// error. Pure, so the protocol rules are testable without a process.
fn response_for(line: &str, id: u64) -> Option<Result<Value, String>> {
    let msg: Value = serde_json::from_str(line).ok()?;
    if msg.get("id").and_then(Value::as_u64) != Some(id) {
        return None;
    }
    if let Some(error) = msg.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Some(Err(format!("server error: {message}")));
    }
    Some(Ok(msg.get("result").cloned().unwrap_or(Value::Null)))
}

/// Unwrap MCP's content envelope to the single text block inside it.
///
/// `isError` is the server saying the CALL failed while the transport
/// succeeded; the text is its explanation, so it becomes ours.
fn tool_text(result: &Value) -> Result<String, String> {
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .ok_or("result carried no content")?;
    let text = content
        .iter()
        .find_map(|c| c.get("text").and_then(Value::as_str))
        .ok_or("result carried no text block")?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(text.to_string());
    }
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_to_another_id_is_not_ours() {
        let line = r#"{"jsonrpc":"2.0","id":7,"result":{}}"#;
        assert!(response_for(line, 1).is_none());
        assert!(response_for(line, 7).is_some());
    }

    #[test]
    fn a_notification_is_skipped_rather_than_mistaken_for_a_reply() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/progress"}"#;
        assert!(response_for(line, 1).is_none());
    }

    #[test]
    fn a_non_json_line_never_derails_the_read() {
        assert!(response_for("beisl-mcp: warming up", 1).is_none());
    }

    #[test]
    fn a_server_error_is_reported_not_swallowed() {
        let line =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#;
        let e = response_for(line, 1).unwrap().unwrap_err();
        assert!(e.contains("method not found"), "{e}");
    }

    #[test]
    fn the_content_envelope_unwraps_to_its_text() {
        let result = json!({"content":[{"type":"text","text":"{\"traces\":[]}"}],"isError":false});
        assert_eq!(tool_text(&result).unwrap(), "{\"traces\":[]}");
    }

    #[test]
    fn an_is_error_result_becomes_our_error() {
        let result =
            json!({"content":[{"type":"text","text":"unknown tool: nope"}],"isError":true});
        let e = tool_text(&result).unwrap_err();
        assert!(e.contains("unknown tool"), "{e}");
    }

    #[test]
    fn a_result_with_no_text_block_is_a_protocol_surprise() {
        let result = json!({"content":[{"type":"image"}],"isError":false});
        assert!(tool_text(&result).is_err());
    }

    #[test]
    fn the_beisl_command_can_be_pointed_at_a_build_tree() {
        // Serial with the default case below only by construction: this test
        // sets the var and the assertion reads it back immediately.
        std::env::set_var("GAMEBUS_BEISL_MCP", "/tmp/beisl-mcp");
        assert_eq!(ServerSpec::beisl().command, "/tmp/beisl-mcp");
        std::env::remove_var("GAMEBUS_BEISL_MCP");
    }
}
