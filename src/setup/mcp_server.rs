//! gamebus-setup as an MCP server: the stash, gamedb and compat findings for
//! front-ends (beisl's window) and agents alike.
//!
//! The same small subset `beisl-mcp` and our own client in [`super::mcp`]
//! speak: newline-delimited JSON-RPC over stdio, `initialize`, `tools/list`,
//! `tools/call`, one client, synchronous. No ports, no async runtime.
//!
//! Tools come in tiers, and a registration opts into each tier by flag:
//! reads always, edits with `--allow-edits`, network with `--allow-network`.
//! `tools/list` shows only what the flags permit. The flags scope what a
//! client was configured to do; they are NOT a security boundary. Anything
//! running as this user can write the stash files directly. What limits the
//! damage is reversibility: dismissals and promotions are flags, not
//! deletions, and nothing here ever submits anywhere.
//!
//! A read tool never writes, not even the inbox drain `compat --json` does
//! as a side effect; it reports how many drops are waiting instead.
//!
//! Edits need a client identity. `initialize` hands out a one-time challenge
//! in `_meta`; the client signs it with its own Ed25519 key in
//! `gamebus/authenticate`. The key is remembered on first use or approved
//! by the owner, per the ledger's policy, and every edit is recorded in the
//! ledger under that identity (see [`super::auth`]). The key identifies and
//! separates cooperating clients; it does not prove who is at the keyboard.

use std::cell::RefCell;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use ed25519_dalek::Verifier;
use gamebus_coupler::{
    apply_finding, apply_miss, ErrorReason, FindingVerb, MissRow, MissVerb, Refusal, ToolError,
};
use serde_json::{json, Value};

use super::auth::{self, Action, Actor, AppendError, AuthPaths, Ledger, LedgerState, Origin};
use crate::compat::CompatStash;
use crate::umu_report::{today, UmuReport};

/// The MCP revision this server was written against. The client's requested
/// version is echoed instead, as beisl-mcp does: every revision to date
/// speaks this subset identically.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// What a registration opted into.
#[derive(Debug, Clone, Copy, Default)]
pub struct Grants {
    pub edits: bool,
    pub network: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Read,
    Edit,
    // The network tools (verify, fetch, store lookups) land in the next phase.
    #[allow(dead_code)]
    Network,
}

struct Tool {
    name: &'static str,
    tier: Tier,
    description: &'static str,
    schema: fn() -> Value,
}

fn no_args() -> Value {
    json!({"type": "object", "properties": {}, "additionalProperties": false})
}

fn verb_arg(what: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "key": {"type": "string", "description": "The stash key"},
            "verb": {"type": "object", "description": what}
        },
        "required": ["key", "verb"],
        "additionalProperties": false
    })
}

fn miss_verb_arg() -> Value {
    verb_arg("A gamebus_coupler MissVerb, e.g. {\"verb\": \"dismiss\"} or {\"verb\": \"set-title\", \"title\": \"...\"}")
}

fn finding_verb_arg() -> Value {
    verb_arg("A gamebus_coupler FindingVerb, e.g. {\"verb\": \"mark-reported\", \"target\": \"protondb\"}")
}

fn hash_arg() -> Value {
    json!({
        "type": "object",
        "properties": {"hash": {"type": "string", "description": "A ledger line hash (hex SHA-256), e.g. a head you remembered"}},
        "required": ["hash"],
        "additionalProperties": false
    })
}

fn key_arg() -> Value {
    json!({
        "type": "object",
        "properties": {"key": {"type": "string", "description": "The stash key, e.g. steam:4809930"}},
        "required": ["key"],
        "additionalProperties": false
    })
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "list_misses",
        tier: Tier::Read,
        description: "Every identity miss in the stash: launches with no authoritative \
                      identity, with the review annotations and whether each is a umu \
                      candidate.",
        schema: no_args,
    },
    Tool {
        name: "get_miss",
        tier: Tier::Read,
        description: "One identity miss by its stash key.",
        schema: key_arg,
    },
    Tool {
        name: "list_findings",
        tier: Tier::Read,
        description: "Every compat finding (a wall that stopped a game) with the targets \
                      that still want it (`needs`). Also reports how many inbox drops \
                      are waiting to be taken in.",
        schema: no_args,
    },
    Tool {
        name: "get_finding",
        tier: Tier::Read,
        description: "One compat finding by its stash key.",
        schema: key_arg,
    },
    Tool {
        name: "ledger_contains",
        tier: Tier::Read,
        description: "Whether a ledger line with this hash is still in the chain. A \
                      second party remembers the head it last saw and asks this: \
                      present means the chain only moved on, absent under the same \
                      ledger id means it was rewritten.",
        schema: hash_arg,
    },
    Tool {
        name: "apply_miss",
        tier: Tier::Edit,
        description: "Apply one review edit to an identity miss (dismiss, promote, set a \
                      title, store or identity, assign a umu id, pick a database entry). \
                      Reversible; recorded in the ledger under this client.",
        schema: miss_verb_arg,
    },
    Tool {
        name: "apply_finding",
        tier: Tier::Edit,
        description: "Dismiss, restore, or mark a compat finding reported to a target. \
                      Recorded in the ledger under this client. Marking reported does not \
                      submit anything; filing stays the user's act.",
        schema: finding_verb_arg,
    },
    Tool {
        name: "drain_inbox",
        tier: Tier::Edit,
        description: "Take the findings waiting in the compat inbox into the stash.",
        schema: no_args,
    },
];

/// Where the server reads and writes. The real locations in production;
/// temp files in tests, so a test never touches the user's stash.
#[derive(Debug, Clone)]
pub struct Stores {
    pub misses: Option<PathBuf>,
    pub findings: Option<PathBuf>,
    pub inbox: Option<PathBuf>,
    pub auth: Option<AuthPaths>,
}

impl Stores {
    pub fn default_paths() -> Self {
        Self {
            misses: UmuReport::default_path(),
            findings: CompatStash::default_path(),
            inbox: super::inbox::dir(),
            auth: AuthPaths::default_paths(),
        }
    }
}

/// One connection's state: the pending challenge and, once proven, who the
/// client is.
#[derive(Debug, Default)]
struct Session {
    challenge: Option<[u8; 32]>,
    /// `clientInfo.name` from `initialize`: the client's own suggestion.
    label: Option<String>,
    client: Option<(String, String)>,
}

pub struct Server {
    grants: Grants,
    stores: Stores,
    session: RefCell<Session>,
}

impl Server {
    pub fn new(grants: Grants, stores: Stores) -> Self {
        Self {
            grants,
            stores,
            session: RefCell::default(),
        }
    }

    fn granted(&self, tier: Tier) -> bool {
        match tier {
            Tier::Read => true,
            Tier::Edit => self.grants.edits,
            Tier::Network => self.grants.network,
        }
    }

    /// Pump the stdio transport until EOF; the client hanging up is the
    /// shutdown.
    pub fn serve(&self, input: impl BufRead, mut out: impl Write) -> io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(resp) = self.handle_line(&line) {
                writeln!(out, "{resp}")?;
                out.flush()?;
            }
        }
        Ok(())
    }

    /// One JSON-RPC message in, at most one out. Notifications (no `id`) get
    /// no response.
    pub fn handle_line(&self, line: &str) -> Option<String> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Some(rpc_error(Value::Null, -32700, "parse error")),
        };
        let id = match msg.get("id") {
            Some(i) if !i.is_null() => i.clone(),
            _ => return None,
        };
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => self.initialize(&params),
            "ping" => json!({}),
            "tools/list" => self.tools_list(),
            "tools/call" => self.tools_call(&params),
            "gamebus/authenticate" => match self.authenticate(&params) {
                Ok(v) => v,
                Err(e) => return Some(rpc_tool_error(id, &e)),
            },
            other => return Some(rpc_error(id, -32601, &format!("method not found: {other}"))),
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string())
    }

    fn tools_list(&self) -> Value {
        let tools: Vec<Value> = TOOLS
            .iter()
            .filter(|t| self.granted(t.tier))
            .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": (t.schema)()}))
            .collect();
        json!({"tools": tools})
    }

    fn tools_call(&self, params: &Value) -> Value {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let outcome = match TOOLS.iter().find(|t| t.name == name) {
            None => Err(ToolError::new(
                ErrorReason::UnknownTool,
                format!("unknown tool: {name}"),
            )),
            Some(t) if !self.granted(t.tier) => Err(not_granted(t.tier)),
            Some(_) => self.dispatch(name, &args),
        };
        match outcome {
            Ok(value) => {
                json!({"content": [{"type": "text", "text": value.to_string()}], "isError": false})
            }
            Err(e) => {
                let text = serde_json::to_string(&e).unwrap_or_default();
                json!({"content": [{"type": "text", "text": text}], "isError": true})
            }
        }
    }
}

impl Server {
    fn initialize(&self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSION);
        let mut challenge = [0u8; 32];
        let challenge_hex = match getrandom::fill(&mut challenge) {
            Ok(()) => Some(auth::hex(&challenge)),
            Err(_) => None,
        };
        {
            let mut s = self.session.borrow_mut();
            s.challenge = challenge_hex.as_ref().map(|_| challenge);
            s.label = params
                .pointer("/clientInfo/name")
                .and_then(Value::as_str)
                .map(str::to_string);
            s.client = None;
        }
        json!({
            "protocolVersion": requested,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "gamebus-setup", "version": env!("CARGO_PKG_VERSION")},
            "instructions":
                "gamebus-setup houses the identity-miss stash (games launched with no \
                 authoritative identity, and their review), the compat findings (walls that \
                 stopped a game) and gamebus-gamedb. Start with list_misses or \
                 list_findings. Edits need gamebus/authenticate first. A failed call \
                 answers with a JSON object {reason, message}; each reason has one cure.",
            "_meta": {
                "gamebus/challenge": challenge_hex,
                "gamebus/ledger": self.ledger_anchor(),
            }
        })
    }

    /// `{id, head}` of a verified ledger, or null. What a second party
    /// remembers to notice a rewrite.
    fn ledger_anchor(&self) -> Value {
        match self.stores.auth.as_ref().map(auth::open) {
            Some(LedgerState::Ok(l)) => json!({"id": l.id, "head": l.head}),
            _ => Value::Null,
        }
    }

    fn ledger(&self) -> Result<Ledger, ToolError> {
        let Some(paths) = &self.stores.auth else {
            return Err(ToolError::new(
                ErrorReason::AuthUninitialized,
                "no data directory to keep auth in",
            ));
        };
        match auth::open(paths) {
            LedgerState::Ok(l) => Ok(l),
            LedgerState::Uninitialized => Err(ToolError::new(
                ErrorReason::AuthUninitialized,
                "auth is not initialised: run `gamebus-setup auth init`",
            )),
            LedgerState::Missing => Err(ToolError::new(
                ErrorReason::LedgerMissing,
                "the ledger is missing: re-initialise it with `gamebus-setup auth reset-ledger`",
            )),
            LedgerState::Unverified(e) => Err(ToolError::new(
                ErrorReason::LedgerUnverified,
                format!(
                    "the ledger does not verify ({e}): re-initialise it with \
                     `gamebus-setup auth reset-ledger`"
                ),
            )),
        }
    }

    /// `gamebus/authenticate {name, public_key, signature}`: prove the key by
    /// signing this connection's challenge, then get it remembered or
    /// recognised.
    fn authenticate(&self, params: &Value) -> Result<Value, ToolError> {
        let bad = |m: &str| ToolError::new(ErrorReason::BadArguments, m);
        let field = |f: &str| params.get(f).and_then(Value::as_str);
        let name = field("name")
            .ok_or_else(|| bad("missing `name`"))?
            .to_string();
        let key_hex = field("public_key").ok_or_else(|| bad("missing `public_key`"))?;
        let sig_hex = field("signature").ok_or_else(|| bad("missing `signature`"))?;
        let challenge = self
            .session
            .borrow_mut()
            .challenge
            .take()
            .ok_or_else(|| bad("no challenge: call initialize first (each is used once)"))?;
        let key = auth::verifying_key(key_hex).ok_or_else(|| bad("malformed `public_key`"))?;
        let sig_bytes: [u8; 64] = auth::unhex(sig_hex)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| bad("malformed `signature`"))?;
        key.verify(
            &challenge,
            &ed25519_dalek::Signature::from_bytes(&sig_bytes),
        )
        .map_err(|_| bad("the signature does not match the challenge"))?;

        let ledger = self.ledger()?;
        let paths = self.stores.auth.as_ref().expect("ledger() checked it");
        let process = auth::process_tree();
        let approved = match ledger.clients.get(&name) {
            Some(rec) if rec.key == key_hex => rec.approved,
            Some(rec) => {
                return Err(ToolError::new(
                    ErrorReason::ClientKeyMismatch,
                    format!(
                        "gamebus knows a different key for {name} (on record {}, presented {}): \
                         re-approve it with `gamebus-setup auth approve {name}`",
                        auth::fingerprint(&rec.key),
                        auth::fingerprint(key_hex)
                    ),
                ));
            }
            None if ledger.settings.registration == auth::Registration::TrustOnFirstUse => {
                let actor = Actor {
                    origin: Origin::Client,
                    name: Some(name.clone()),
                    key: Some(key_hex.to_string()),
                    process,
                };
                let action = Action::RegisterClient {
                    name: name.clone(),
                    key: key_hex.to_string(),
                };
                auth::append(paths, actor, action, Value::Null, None).map_err(append_error)?;
                false
            }
            None => {
                auth::note_pending(
                    paths,
                    &name,
                    auth::Pending {
                        key: key_hex.to_string(),
                        process,
                        date: today(),
                    },
                );
                return Err(ToolError::new(
                    ErrorReason::ClientUnapproved,
                    format!(
                        "{name} is not approved yet: approve it with \
                         `gamebus-setup auth approve {name}`"
                    ),
                ));
            }
        };
        self.session.borrow_mut().client = Some((name.clone(), key_hex.to_string()));
        Ok(json!({
            "client": name,
            "approved": approved,
            "fingerprint": auth::fingerprint(key_hex),
            "ledger": self.ledger_anchor(),
        }))
    }

    /// The gate every edit passes: an authenticated client, still on record
    /// with the same key, under a verified ledger.
    fn editor(&self) -> Result<(Ledger, Actor), ToolError> {
        let (name, key) = self.session.borrow().client.clone().ok_or_else(|| {
            ToolError::new(
                ErrorReason::NotAuthenticated,
                "call gamebus/authenticate before an edit",
            )
        })?;
        let ledger = self.ledger()?;
        match ledger.clients.get(&name) {
            Some(rec) if rec.key == key => {}
            Some(_) => {
                return Err(ToolError::new(
                    ErrorReason::ClientKeyMismatch,
                    format!("{name}'s key changed on record since this connection authenticated"),
                ))
            }
            None => {
                return Err(ToolError::new(
                    ErrorReason::ClientUnapproved,
                    format!("{name} is no longer on record"),
                ))
            }
        }
        let actor = Actor {
            origin: Origin::Client,
            name: Some(name),
            key: Some(key),
            process: auth::process_tree(),
        };
        Ok((ledger, actor))
    }

    /// Record an edit, unless the policy records none. Before the stash is
    /// written: an edit the ledger refused never happens.
    fn record(
        &self,
        ledger: &Ledger,
        actor: Actor,
        action: Action,
        prior: Value,
    ) -> Result<(), ToolError> {
        if ledger.settings.strategy == auth::Strategy::OptIn {
            return Ok(());
        }
        let paths = self.stores.auth.as_ref().expect("editor() checked it");
        auth::append(paths, actor, action, prior, None)
            .map(|_| ())
            .map_err(append_error)
    }

    fn ledger_contains(&self, args: &Value) -> Result<Value, ToolError> {
        let hash = args.get("hash").and_then(Value::as_str).ok_or_else(|| {
            ToolError::new(ErrorReason::BadArguments, "missing string argument `hash`")
        })?;
        let Some(paths) = &self.stores.auth else {
            return Ok(json!({"present": false, "verified": false, "ledger": null}));
        };
        let (verified, anchor) = match auth::open(paths) {
            LedgerState::Ok(l) => (true, json!({"id": l.id, "head": l.head})),
            _ => (false, Value::Null),
        };
        Ok(json!({
            "present": auth::contains(paths, hash),
            "verified": verified,
            "ledger": anchor,
        }))
    }

    fn apply_miss(&self, args: &Value) -> Result<Value, ToolError> {
        let key = key(args)?;
        let verb: MissVerb = verb(args)?;
        let (ledger, actor) = self.editor()?;
        let path = self.stores.misses.clone().ok_or_else(no_store)?;
        let mut report = UmuReport::annotating_at(path);
        if let Some(e) = report.load_error() {
            return Err(ToolError::new(ErrorReason::StashUnreadable, e));
        }
        let prior = report.entries().get(&key).cloned().ok_or_else(|| {
            ToolError::new(
                ErrorReason::NotFound,
                format!("no identity miss under {key}"),
            )
        })?;
        if let MissVerb::AssignId { id } = &verb {
            let id = id.trim().to_lowercase();
            super::umu_misses::check_id(prior.effective_title(), &id)
                .map_err(|e| ToolError::new(ErrorReason::Refused, e))?;
        }
        let mut edited = prior.clone();
        let change = apply_miss(&mut edited, &verb, &today()).map_err(refused)?;
        let action = Action::EditMiss {
            key: key.clone(),
            verb,
            change: change.clone(),
        };
        self.record(&ledger, actor, action, json!(prior))?;
        report.update(&key, |m| *m = edited.clone());
        report.save();
        Ok(json!({"change": change, "row": miss_row(&key, &edited)}))
    }

    fn apply_finding(&self, args: &Value) -> Result<Value, ToolError> {
        let key = key(args)?;
        let verb: FindingVerb = verb(args)?;
        let (ledger, actor) = self.editor()?;
        let mut stash = self.findings()?;
        let prior = stash.findings().get(&key).cloned().ok_or_else(|| {
            ToolError::new(
                ErrorReason::NotFound,
                format!("no compat finding under {key}"),
            )
        })?;
        let mut edited = prior.clone();
        let change = apply_finding(&mut edited, &verb, &today()).map_err(refused)?;
        let action = Action::EditFinding {
            key: key.clone(),
            verb,
            change: change.clone(),
        };
        self.record(&ledger, actor, action, json!(prior))?;
        stash.update(&key, |f| *f = edited.clone());
        stash.save();
        Ok(json!({"change": change, "row": super::compat::finding_json(&key, &edited)}))
    }

    fn drain_inbox(&self) -> Result<Value, ToolError> {
        let (ledger, actor) = self.editor()?;
        let mut stash = self.findings()?;
        let Some(dir) = self.stores.inbox.clone() else {
            return Ok(json!({"recorded": [], "rejected": [], "held": false}));
        };
        let drained = super::compat::drain_dir(&mut stash, &dir);
        if !drained.recorded.is_empty() {
            let action = Action::DrainInbox {
                keys: drained.recorded.clone(),
            };
            self.record(&ledger, actor, action, Value::Null)?;
        }
        let rejected: Vec<Value> = drained
            .rejected
            .iter()
            .map(|(p, why)| json!({"path": p.display().to_string(), "why": why}))
            .collect();
        Ok(json!({"recorded": drained.recorded, "rejected": rejected, "held": drained.held}))
    }
}

fn verb<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    let raw = args
        .get("verb")
        .cloned()
        .ok_or_else(|| ToolError::new(ErrorReason::BadArguments, "missing `verb`"))?;
    serde_json::from_value(raw)
        .map_err(|e| ToolError::new(ErrorReason::BadArguments, format!("not a verb: {e}")))
}

fn refused(r: Refusal) -> ToolError {
    let message = match &r {
        Refusal::NotAUmuMiss => {
            "a launcher launch never went through umu; nothing to promote".to_string()
        }
        Refusal::EmptyTitle => "an empty title".to_string(),
        Refusal::EmptyId => "an empty id".to_string(),
        Refusal::UnknownTarget { target } => format!("unknown target {target}"),
    };
    ToolError::new(ErrorReason::Refused, message)
}

fn no_store() -> ToolError {
    ToolError::new(
        ErrorReason::StashUnreadable,
        "no data directory for the stash",
    )
}

fn append_error(e: AppendError) -> ToolError {
    let reason = match &e {
        AppendError::Uninitialized => ErrorReason::AuthUninitialized,
        AppendError::Missing => ErrorReason::LedgerMissing,
        AppendError::Unverified(_) => ErrorReason::LedgerUnverified,
        AppendError::Refused(_) => ErrorReason::Refused,
        AppendError::Io(_) => ErrorReason::LedgerUnverified,
    };
    ToolError::new(reason, e.to_string())
}

fn rpc_tool_error(id: Value, e: &ToolError) -> String {
    json!({"jsonrpc": "2.0", "id": id,
           "error": {"code": -32001, "message": e.message, "data": e}})
    .to_string()
}

fn not_granted(tier: Tier) -> ToolError {
    match tier {
        Tier::Network => ToolError::new(
            ErrorReason::NetworkNotEnabled,
            "this server was started without --allow-network",
        ),
        _ => ToolError::new(
            ErrorReason::EditsNotEnabled,
            "this server was started without --allow-edits",
        ),
    }
}

fn key(args: &Value) -> Result<String, ToolError> {
    args.get("key")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ToolError::new(ErrorReason::BadArguments, "missing string argument `key`"))
}

fn miss_row(key: &str, m: &gamebus_coupler::Miss) -> MissRow {
    MissRow {
        key: key.to_string(),
        is_umu_miss: m.is_umu_miss(),
        umu_candidate: gamebus_coupler::umu_candidate(m),
        miss: m.clone(),
    }
}

impl Server {
    fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        match name {
            "list_misses" => self.list_misses(),
            "get_miss" => self.get_miss(&key(args)?),
            "list_findings" => self.list_findings(),
            "get_finding" => self.get_finding(&key(args)?),
            "ledger_contains" => self.ledger_contains(args),
            "apply_miss" => self.apply_miss(args),
            "apply_finding" => self.apply_finding(args),
            "drain_inbox" => self.drain_inbox(),
            other => Err(ToolError::new(
                ErrorReason::UnknownTool,
                format!("unknown tool: {other}"),
            )),
        }
    }

    fn misses(&self) -> Result<UmuReport, ToolError> {
        let Some(path) = self.stores.misses.clone() else {
            return Ok(UmuReport::default());
        };
        let report = UmuReport::from_path(path);
        match report.load_error() {
            Some(e) => Err(ToolError::new(ErrorReason::StashUnreadable, e)),
            None => Ok(report),
        }
    }

    fn findings(&self) -> Result<CompatStash, ToolError> {
        let Some(path) = self.stores.findings.clone() else {
            return Ok(CompatStash::default());
        };
        let stash = CompatStash::from_path(path);
        match stash.load_error() {
            Some(e) => Err(ToolError::new(ErrorReason::StashUnreadable, e)),
            None => Ok(stash),
        }
    }

    fn list_misses(&self) -> Result<Value, ToolError> {
        let report = self.misses()?;
        let mut keys: Vec<&String> = report.entries().keys().collect();
        keys.sort();
        let rows: Vec<MissRow> = keys
            .into_iter()
            .map(|k| miss_row(k, &report.entries()[k]))
            .collect();
        Ok(json!({"misses": rows}))
    }

    fn get_miss(&self, key: &str) -> Result<Value, ToolError> {
        let report = self.misses()?;
        let m = report.entries().get(key).ok_or_else(|| {
            ToolError::new(
                ErrorReason::NotFound,
                format!("no identity miss under {key}"),
            )
        })?;
        Ok(json!(miss_row(key, m)))
    }

    fn list_findings(&self) -> Result<Value, ToolError> {
        let stash = self.findings()?;
        let mut keys: Vec<&String> = stash.findings().keys().collect();
        keys.sort();
        let rows: Vec<Value> = keys
            .into_iter()
            .map(|k| super::compat::finding_json(k, &stash.findings()[k]))
            .collect();
        let waiting = self
            .stores
            .inbox
            .as_deref()
            .map(|d| super::inbox::pending(d).len())
            .unwrap_or(0);
        Ok(json!({"findings": rows, "inbox_pending": waiting}))
    }

    fn get_finding(&self, key: &str) -> Result<Value, ToolError> {
        let stash = self.findings()?;
        let f = stash.findings().get(key).ok_or_else(|| {
            ToolError::new(
                ErrorReason::NotFound,
                format!("no compat finding under {key}"),
            )
        })?;
        Ok(super::compat::finding_json(key, f))
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// `gamebus-setup mcp [--allow-edits] [--allow-network]`: serve stdio until
/// the client hangs up.
pub fn run(args: &[String]) -> std::process::ExitCode {
    let grants = Grants {
        edits: args.iter().any(|a| a == "--allow-edits"),
        network: args.iter().any(|a| a == "--allow-network"),
    };
    let server = Server::new(grants, Stores::default_paths());
    let stdin = io::stdin();
    match server.serve(stdin.lock(), io::stdout().lock()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mcp: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gamebus_coupler::FindingRow;

    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("gamebus-mcp-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("inbox")).unwrap();
            Self { dir }
        }

        fn stores(&self) -> Stores {
            Stores {
                misses: Some(self.dir.join("umu-misses.json")),
                findings: Some(self.dir.join("compat-findings.json")),
                inbox: Some(self.dir.join("inbox")),
                auth: Some(AuthPaths {
                    dir: self.dir.join("auth"),
                }),
            }
        }

        fn write(&self, name: &str, body: &str) {
            std::fs::write(self.dir.join(name), body).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn call(server: &Server, tool: &str, args: Value) -> (bool, Value) {
        let req = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                         "params": {"name": tool, "arguments": args}});
        let resp: Value =
            serde_json::from_str(&server.handle_line(&req.to_string()).unwrap()).unwrap();
        let result = &resp["result"];
        let text = result["content"][0]["text"].as_str().unwrap();
        (
            result["isError"].as_bool().unwrap(),
            serde_json::from_str(text).unwrap(),
        )
    }

    const MISSES: &str = r#"{"lutris:severed-steel": {
        "title": "Severed Steel", "store": "gog", "codename": "1242122770",
        "umu_id": "umu-default", "title_source": "lutris-wrapper", "confidence": "medium",
        "executable": null, "first_seen": "2026-09-23", "last_seen": "2026-09-23",
        "fix": {"umu_id": "umu-1227690", "fixes": [], "local": ["/x/umu-1227690.py"], "checked": "2026-09-23"}
    }}"#;

    const FINDINGS: &str = r#"{"steam:244850": {
        "wall": "wine-stub", "first_seen": "2026-09-05", "last_seen": "2026-09-05",
        "observations": [{"source": "beisl", "observed": "2026-09-05", "beisl_version": "0.3.0"}]
    }}"#;

    use ed25519_dalek::{Signer, SigningKey};

    fn rpc(server: &Server, method: &str, params: Value) -> Value {
        let req = json!({"jsonrpc": "2.0", "id": 9, "method": method, "params": params});
        serde_json::from_str(&server.handle_line(&req.to_string()).unwrap()).unwrap()
    }

    /// Initialize and authenticate as `name` with a key made from `seed`.
    fn authed(server: &Server, name: &str, seed: u8) -> Value {
        let init = rpc(server, "initialize", json!({"clientInfo": {"name": name}}));
        let challenge = init["result"]["_meta"]["gamebus/challenge"]
            .as_str()
            .unwrap();
        let key = SigningKey::from_bytes(&[seed; 32]);
        let sig = key.sign(&auth::unhex(challenge).unwrap());
        rpc(
            server,
            "gamebus/authenticate",
            json!({
                "name": name,
                "public_key": auth::hex(key.verifying_key().as_bytes()),
                "signature": auth::hex(&sig.to_bytes()),
            }),
        )
    }

    fn editing(fx: &Fixture) -> Server {
        Server::new(
            Grants {
                edits: true,
                network: false,
            },
            fx.stores(),
        )
    }

    fn auth_paths(fx: &Fixture) -> AuthPaths {
        fx.stores().auth.unwrap()
    }

    #[test]
    fn before_auth_init_reads_work_and_edits_say_how_to_start() {
        let fx = Fixture::new("uninit");
        fx.write("compat-findings.json", FINDINGS);
        let server = editing(&fx);
        let resp = authed(&server, "beisl-compat", 1);
        assert_eq!(resp["error"]["data"]["reason"], "auth-uninitialized");
        let (err, _) = call(&server, "list_findings", json!({}));
        assert!(!err);
        let (err, body) = call(
            &server,
            "apply_finding",
            json!({"key": "steam:244850", "verb": {"verb": "dismiss"}}),
        );
        assert!(err);
        assert_eq!(body["reason"], "not-authenticated");
    }

    #[test]
    fn a_remembered_client_edits_and_every_edit_is_ledgered_with_its_prior() {
        let fx = Fixture::new("tofu");
        fx.write("compat-findings.json", FINDINGS);
        auth::fast_init(&auth_paths(&fx), "pw");
        let server = editing(&fx);
        let resp = authed(&server, "beisl-compat", 1);
        assert_eq!(resp["result"]["client"], "beisl-compat", "{resp}");
        assert_eq!(resp["result"]["approved"], false);

        let (err, body) = call(
            &server,
            "apply_finding",
            json!({"key": "steam:244850", "verb": {"verb": "mark-reported", "target": "protondb"}}),
        );
        assert!(!err, "{body}");
        assert_eq!(body["change"]["change"], "reported");
        assert_eq!(body["row"]["needs"], json!(["gamedb"]));

        let entries = auth::entries(&auth_paths(&fx)).unwrap();
        let (last, _) = entries.last().unwrap();
        assert_eq!(last.actor.name.as_deref(), Some("beisl-compat"));
        assert!(
            last.prior.get("reported").is_none(),
            "the prior is the finding before the edit"
        );
        assert!(matches!(auth::open(&auth_paths(&fx)), LedgerState::Ok(_)));
    }

    #[test]
    fn a_new_key_under_a_known_name_is_a_mismatch_with_both_fingerprints() {
        let fx = Fixture::new("mismatch");
        auth::fast_init(&auth_paths(&fx), "pw");
        let server = editing(&fx);
        authed(&server, "beisl-compat", 1);
        let resp = authed(&server, "beisl-compat", 2);
        assert_eq!(resp["error"]["data"]["reason"], "client-key-mismatch");
        let msg = resp["error"]["data"]["message"].as_str().unwrap();
        assert!(
            msg.contains("on record") && msg.contains("presented"),
            "{msg}"
        );
    }

    #[test]
    fn under_explicit_approval_a_new_client_waits_for_the_owner() {
        let fx = Fixture::new("explicit");
        let paths = auth_paths(&fx);
        let owner = auth::fast_init(&paths, "pw");
        let explicit = Action::SetSettings {
            settings: auth::Settings {
                registration: auth::Registration::ExplicitApproval,
                strategy: auth::Strategy::McpEdits,
            },
        };
        let me = Actor {
            origin: Origin::Owner,
            name: None,
            key: None,
            process: vec![],
        };
        auth::append(&paths, me.clone(), explicit, Value::Null, Some(&owner)).unwrap();
        let server = editing(&fx);
        let resp = authed(&server, "agent", 3);
        assert_eq!(resp["error"]["data"]["reason"], "client-unapproved");
        let waiting = auth::pending_requests(&paths);
        let key = waiting["agent"].key.clone();
        auth::append(
            &paths,
            me,
            Action::ApproveClient {
                name: "agent".into(),
                key,
            },
            Value::Null,
            Some(&owner),
        )
        .unwrap();
        let resp = authed(&server, "agent", 3);
        assert_eq!(resp["result"]["approved"], true, "{resp}");
    }

    #[test]
    fn a_tampered_ledger_blocks_every_edit() {
        let fx = Fixture::new("tamper");
        fx.write("umu-misses.json", MISSES);
        let paths = auth_paths(&fx);
        auth::fast_init(&paths, "pw");
        let server = editing(&fx);
        authed(&server, "beisl-compat", 1);
        let ledger = paths.dir.join("ledger.jsonl");
        let raw = std::fs::read_to_string(&ledger).unwrap();
        std::fs::write(&ledger, raw.replace("mcp-edits", "opt-in")).unwrap();
        let (err, body) = call(
            &server,
            "apply_miss",
            json!({"key": "lutris:severed-steel", "verb": {"verb": "dismiss"}}),
        );
        assert!(err);
        assert_eq!(body["reason"], "ledger-unverified");
        let stash = std::fs::read_to_string(fx.dir.join("umu-misses.json")).unwrap();
        assert!(
            !stash.contains("dismissed"),
            "a blocked edit reached the stash"
        );
    }

    #[test]
    fn a_remembered_head_stays_present_until_the_chain_is_rewritten() {
        let fx = Fixture::new("contains");
        fx.write("compat-findings.json", FINDINGS);
        let paths = auth_paths(&fx);
        auth::fast_init(&paths, "pw");
        let server = editing(&fx);
        let first = authed(&server, "beisl-compat", 1);
        let remembered = first["result"]["ledger"]["head"]
            .as_str()
            .unwrap()
            .to_string();
        call(
            &server,
            "apply_finding",
            json!({"key": "steam:244850", "verb": {"verb": "dismiss"}}),
        );
        let (_, body) = call(&server, "ledger_contains", json!({"hash": remembered}));
        assert_eq!(
            body["present"], true,
            "an edit moves the head on; the old one stays in"
        );
        assert_ne!(body["ledger"]["head"], json!(remembered));

        // Rewrite history: drop every line after the genesis.
        let ledger = paths.dir.join("ledger.jsonl");
        let genesis = std::fs::read_to_string(&ledger)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_string();
        std::fs::write(&ledger, format!("{genesis}\n")).unwrap();
        let (_, body) = call(&server, "ledger_contains", json!({"hash": remembered}));
        assert_eq!(
            body["present"], false,
            "a truncated chain lost the remembered head"
        );
    }

    #[test]
    fn a_coupler_refusal_writes_nothing() {
        let fx = Fixture::new("refusal");
        fx.write("compat-findings.json", FINDINGS);
        let paths = auth_paths(&fx);
        auth::fast_init(&paths, "pw");
        let server = editing(&fx);
        authed(&server, "beisl-compat", 1);
        let before = auth::entries(&paths).unwrap().len();
        let (err, body) = call(
            &server,
            "apply_finding",
            json!({"key": "steam:244850", "verb": {"verb": "mark-reported", "target": "protndb"}}),
        );
        assert!(err);
        assert_eq!(body["reason"], "refused");
        assert_eq!(auth::entries(&paths).unwrap().len(), before);
    }

    #[test]
    fn initialize_echoes_the_version_and_notifications_get_no_reply() {
        let server = Server::new(Grants::default(), Fixture::new("init").stores());
        let resp = server
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#)
            .unwrap();
        let resp: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(resp["result"]["serverInfo"]["name"], "gamebus-setup");
        assert!(server
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
    }

    #[test]
    fn tools_list_offers_only_the_granted_tiers() {
        let server = Server::new(Grants::default(), Fixture::new("list").stores());
        let resp: Value = serde_json::from_str(
            &server
                .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
                .unwrap(),
        )
        .unwrap();
        let names: Vec<&str> = resp["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"list_misses"));
        for t in TOOLS.iter().filter(|t| t.tier != Tier::Read) {
            assert!(
                !names.contains(&t.name),
                "{} offered without its grant",
                t.name
            );
        }
    }

    #[test]
    fn misses_come_back_as_typed_rows_with_the_engines_judgements() {
        let fx = Fixture::new("misses");
        fx.write("umu-misses.json", MISSES);
        let server = Server::new(Grants::default(), fx.stores());
        let (err, body) = call(&server, "list_misses", json!({}));
        assert!(!err, "{body}");
        let rows: Vec<MissRow> = serde_json::from_value(body["misses"].clone()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key, "lutris:severed-steel");
        // A local protonfix makes it a umu candidate.
        assert!(rows[0].is_umu_miss && rows[0].umu_candidate);

        let (err, body) = call(&server, "get_miss", json!({"key": "nope"}));
        assert!(err);
        assert_eq!(body["reason"], "not-found");
    }

    #[test]
    fn findings_match_compat_json_and_count_the_waiting_inbox() {
        let fx = Fixture::new("findings");
        fx.write("compat-findings.json", FINDINGS);
        fx.write("inbox/1-run.json", "{}");
        let server = Server::new(Grants::default(), fx.stores());
        let (err, body) = call(&server, "list_findings", json!({}));
        assert!(!err, "{body}");
        assert_eq!(body["inbox_pending"], 1);
        let row: FindingRow = serde_json::from_value(body["findings"][0].clone()).unwrap();
        assert_eq!(row.key, "steam:244850");
        // A wine stub is not AreWeAntiCheatYet's business.
        assert_eq!(row.needs, ["protondb", "gamedb"]);
        // A field this build does not know survives the trip.
        assert!(row.finding.observations[0]
            .extra
            .contains_key("beisl_version"));
        // Reading never drains: the drop is still there.
        assert!(fx.dir.join("inbox/1-run.json").exists());
    }

    #[test]
    fn an_unreadable_stash_is_an_error_never_an_empty_list() {
        let fx = Fixture::new("corrupt");
        fx.write("umu-misses.json", "{ not json");
        fx.write("compat-findings.json", "[");
        let server = Server::new(Grants::default(), fx.stores());
        for tool in ["list_misses", "list_findings"] {
            let (err, body) = call(&server, tool, json!({}));
            assert!(err, "{tool} hid a corrupt stash");
            assert_eq!(body["reason"], "stash-unreadable", "{tool}");
        }
    }

    #[test]
    fn a_missing_stash_is_simply_empty() {
        let server = Server::new(Grants::default(), Fixture::new("empty").stores());
        let (err, body) = call(&server, "list_misses", json!({}));
        assert!(!err);
        assert_eq!(body["misses"], json!([]));
    }

    #[test]
    fn unknown_tools_and_bad_arguments_have_their_own_reasons() {
        let server = Server::new(Grants::default(), Fixture::new("bad").stores());
        let (err, body) = call(&server, "drop_tables", json!({}));
        assert!(err);
        assert_eq!(body["reason"], "unknown-tool");
        let (err, body) = call(&server, "get_miss", json!({}));
        assert!(err);
        assert_eq!(body["reason"], "bad-arguments");
    }
}
