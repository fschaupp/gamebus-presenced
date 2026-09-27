//! Client identities, the owner key, and the ledger.
//!
//! The ledger is an append-only, hash-chained JSON-lines file. It records
//! every edit made over MCP (and, per policy, from the CLI and the TUI), and
//! it IS the policy: the owner-signed genesis carries the initial settings,
//! and every later settings change, client approval or removal is another
//! owner-signed entry. The effective policy is whatever replaying the
//! ledger yields, so editing a file by hand changes nothing but the chain's
//! validity.
//!
//! The owner key is derived from a passphrase with argon2 and never stored;
//! only its public half is on disk. That passphrase is the one secret an
//! agent running as this user does not have, which is what lets owner
//! actions (approving a client, changing policy, resetting the ledger) stay
//! the human's. Everything else here identifies and attributes cooperating
//! clients; it cannot stop a process running as this user from rewriting
//! files. Replacing the whole trust root is possible that way, but not
//! quietly: the genesis and every signature stop matching.
//!
//! Each line is `{"entry": <entry>, "sig": "<hex>"}`. The chain hashes the
//! whole line as written; an owner signature covers the entry's exact bytes
//! (kept raw, so no re-serialisation can change what was signed).

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use gamebus_coupler::{FindingChange, FindingVerb, MissChange, MissVerb};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::umu_report::today;

/// Where the owner record and the ledger live.
#[derive(Debug, Clone)]
pub struct AuthPaths {
    pub dir: PathBuf,
}

impl AuthPaths {
    pub fn default_paths() -> Option<Self> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(Self {
            dir: data.join("gamebus-presenced").join("auth"),
        })
    }

    fn owner(&self) -> PathBuf {
        self.dir.join("owner.json")
    }

    fn ledger(&self) -> PathBuf {
        self.dir.join("ledger.jsonl")
    }

    fn pending(&self) -> PathBuf {
        self.dir.join("pending.json")
    }
}

/// argon2id cost. The owner derives the key only for owner actions, so a
/// fraction of a second is fine.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Kdf {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

const KDF: Kdf = Kdf {
    m_kib: 64 * 1024,
    t: 3,
    p: 1,
};

/// The public half of the owner key and what derives the private half.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OwnerFile {
    public_key: String,
    salt: String,
    kdf: Kdf,
}

fn derive(passphrase: &str, salt: &[u8], kdf: Kdf) -> Result<SigningKey, String> {
    let params = argon2::Params::new(kdf.m_kib, kdf.t, kdf.p, Some(32))
        .map_err(|e| format!("argon2 parameters: {e}"))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut seed = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut seed)
        .map_err(|e| format!("argon2: {e}"))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// How a client that is not on record gets its first key accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Registration {
    /// The first key under a name is remembered; a different one is refused.
    TrustOnFirstUse,
    /// An unknown key gets no edits until the owner approves it.
    ExplicitApproval,
}

/// Which edits the ledger records. Owner actions and client registrations
/// are always recorded: they are the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Strategy {
    /// MCP and CLI edits; the TUI's own edits are not recorded.
    McpEdits,
    /// No edits recorded.
    OptIn,
    /// Every edit, including the TUI's.
    Everything,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub registration: Registration,
    pub strategy: Strategy,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            registration: Registration::TrustOnFirstUse,
            strategy: Strategy::McpEdits,
        }
    }
}

/// Where an edit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    Owner,
    Client,
    Cli,
    Tui,
}

/// One ancestor of the process that made the change. A note for the reader,
/// never an identity: the key is the identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The client's public key, hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub process: Vec<Proc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum Action {
    Genesis {
        owner_key: String,
        settings: Settings,
        /// Clients carried over from a previous ledger by a reset.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        clients: BTreeMap<String, ClientRecord>,
    },
    SetSettings {
        settings: Settings,
    },
    ApproveClient {
        name: String,
        key: String,
    },
    ForgetClient {
        name: String,
    },
    /// Trust on first use: unsigned, valid only while that mode is set and
    /// the name is new.
    RegisterClient {
        name: String,
        key: String,
    },
    EditMiss {
        key: String,
        verb: MissVerb,
        change: MissChange,
    },
    EditFinding {
        key: String,
        verb: FindingVerb,
        change: FindingChange,
    },
    /// Findings taken in from beisl's inbox.
    DrainInbox {
        keys: Vec<String>,
    },
}

impl Action {
    fn owner_signed(&self) -> bool {
        matches!(
            self,
            Action::Genesis { .. }
                | Action::SetSettings { .. }
                | Action::ApproveClient { .. }
                | Action::ForgetClient { .. }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    /// Hex SHA-256 of the previous line; empty for the genesis.
    pub prev: String,
    pub date: String,
    pub unix: u64,
    pub actor: Actor,
    #[serde(flatten)]
    pub action: Action,
    /// What the edit replaced, for undo.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub prior: Value,
}

#[derive(Serialize, Deserialize)]
struct Line<'a> {
    #[serde(borrow)]
    entry: &'a RawValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sig: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRecord {
    pub key: String,
    /// Approved by the owner, as opposed to remembered on first use.
    pub approved: bool,
}

/// What replaying a verified ledger yields.
#[derive(Debug, Clone)]
pub struct Ledger {
    pub id: String,
    pub head: String,
    pub seq: u64,
    pub settings: Settings,
    pub clients: BTreeMap<String, ClientRecord>,
}

#[derive(Debug)]
pub enum LedgerState {
    /// No owner key: `auth init` has not run.
    Uninitialized,
    /// An owner key, but no ledger file.
    Missing,
    /// The chain or a signature does not verify.
    Unverified(String),
    Ok(Ledger),
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

pub fn verifying_key(hex_key: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = unhex(hex_key)?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

fn signature(hex_sig: &str) -> Option<Signature> {
    let bytes: [u8; 64] = unhex(hex_sig)?.try_into().ok()?;
    Some(Signature::from_bytes(&bytes))
}

/// A short, readable fingerprint of a public key.
pub fn fingerprint(hex_key: &str) -> String {
    hex(&Sha256::digest(hex_key.as_bytes())[..8])
}

fn line_hash(line: &str) -> String {
    hex(&Sha256::digest(line.as_bytes()))
}

fn read_owner(paths: &AuthPaths) -> Option<OwnerFile> {
    let raw = std::fs::read_to_string(paths.owner()).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Verify the whole ledger and replay it into the effective policy.
pub fn open(paths: &AuthPaths) -> LedgerState {
    let Some(owner) = read_owner(paths) else {
        return LedgerState::Uninitialized;
    };
    let Ok(file) = File::open(paths.ledger()) else {
        return LedgerState::Missing;
    };
    match replay(BufReader::new(file), &owner.public_key) {
        Ok(ledger) => LedgerState::Ok(ledger),
        Err(e) => LedgerState::Unverified(e),
    }
}

fn replay(reader: impl BufRead, owner_hex: &str) -> Result<Ledger, String> {
    let owner_key = verifying_key(owner_hex).ok_or("the owner key on disk is malformed")?;
    let mut ledger: Option<Ledger> = None;
    for (n, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("line {}: {e}", n + 1))?;
        let parsed: Line =
            serde_json::from_str(&line).map_err(|e| format!("line {}: {e}", n + 1))?;
        let entry: Entry =
            serde_json::from_str(parsed.entry.get()).map_err(|e| format!("line {}: {e}", n + 1))?;
        let at = |what: &str| format!("line {}: {what}", n + 1);

        if entry.action.owner_signed() {
            let sig = parsed
                .sig
                .as_deref()
                .and_then(signature)
                .ok_or_else(|| at("an owner action without a valid signature"))?;
            owner_key
                .verify(parsed.entry.get().as_bytes(), &sig)
                .map_err(|_| at("the owner signature does not verify"))?;
        }

        match (&mut ledger, &entry.action) {
            (
                None,
                Action::Genesis {
                    owner_key: k,
                    settings,
                    clients,
                },
            ) => {
                if k != owner_hex {
                    return Err(at("the genesis names a different owner key"));
                }
                if entry.seq != 0 || !entry.prev.is_empty() {
                    return Err(at("a malformed genesis"));
                }
                ledger = Some(Ledger {
                    id: line_hash(&line),
                    head: line_hash(&line),
                    seq: 0,
                    settings: *settings,
                    clients: clients.clone(),
                });
                continue;
            }
            (None, _) => return Err(at("the ledger does not start with a genesis")),
            (Some(_), Action::Genesis { .. }) => return Err(at("a second genesis")),
            (Some(l), action) => {
                if entry.prev != l.head || entry.seq != l.seq + 1 {
                    return Err(at("the chain is broken here"));
                }
                apply(l, &entry.actor, action).map_err(|e| at(&e))?;
                l.head = line_hash(&line);
                l.seq = entry.seq;
            }
        }
    }
    ledger.ok_or_else(|| "the ledger is empty".to_string())
}

fn apply(l: &mut Ledger, actor: &Actor, action: &Action) -> Result<(), String> {
    match action {
        Action::Genesis { .. } => unreachable!("handled by the caller"),
        Action::SetSettings { settings } => l.settings = *settings,
        Action::ApproveClient { name, key } => {
            l.clients.insert(
                name.clone(),
                ClientRecord {
                    key: key.clone(),
                    approved: true,
                },
            );
        }
        Action::ForgetClient { name } => {
            l.clients.remove(name);
        }
        Action::RegisterClient { name, key } => {
            if l.settings.registration != Registration::TrustOnFirstUse {
                return Err("a first-use registration while approval was required".into());
            }
            if l.clients.contains_key(name) {
                return Err(format!("a second first-use registration for {name}"));
            }
            l.clients.insert(
                name.clone(),
                ClientRecord {
                    key: key.clone(),
                    approved: false,
                },
            );
        }
        Action::EditMiss { .. } | Action::EditFinding { .. } | Action::DrainInbox { .. } => {
            if actor.origin == Origin::Client {
                let name = actor.name.as_deref().unwrap_or_default();
                let known = l.clients.get(name).map(|c| &c.key);
                if known.is_none() || known != actor.key.as_ref() {
                    return Err(format!("an edit by {name}, whose key is not on record"));
                }
            }
        }
    }
    Ok(())
}

/// Serialise one entry into its ledger line, signing it when a key is given.
fn render(entry: &Entry, owner: Option<&SigningKey>) -> Result<String, String> {
    let body = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    let raw = RawValue::from_string(body).map_err(|e| e.to_string())?;
    let sig = owner.map(|k| hex(&k.sign(raw.get().as_bytes()).to_bytes()));
    serde_json::to_string(&Line { entry: &raw, sig }).map_err(|e| e.to_string())
}

/// An exclusive lock on the auth directory, held while a writer re-verifies
/// and appends, so two writers can never both extend the same head.
struct DirLock {
    _file: File,
}

impl DirLock {
    fn take(paths: &AuthPaths) -> Result<Self, String> {
        std::fs::create_dir_all(&paths.dir).map_err(|e| e.to_string())?;
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(paths.dir.join(".lock"))
            .map_err(|e| e.to_string())?;
        // SAFETY: flock on a descriptor we own; released when the file closes.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err("cannot lock the ledger".into());
        }
        Ok(Self { _file: f })
    }
}

/// Append one entry after re-verifying the ledger under the lock. The owner
/// key is required for owner actions and ignored otherwise.
pub fn append(
    paths: &AuthPaths,
    actor: Actor,
    action: Action,
    prior: Value,
    owner: Option<&SigningKey>,
) -> Result<Ledger, AppendError> {
    let _lock = DirLock::take(paths).map_err(AppendError::Io)?;
    let mut ledger = match open(paths) {
        LedgerState::Ok(l) => l,
        LedgerState::Uninitialized => return Err(AppendError::Uninitialized),
        LedgerState::Missing => return Err(AppendError::Missing),
        LedgerState::Unverified(e) => return Err(AppendError::Unverified(e)),
    };
    if action.owner_signed() && owner.is_none() {
        return Err(AppendError::Io(
            "an owner action needs the owner key".into(),
        ));
    }
    apply(&mut ledger, &actor, &action).map_err(AppendError::Refused)?;
    let entry = Entry {
        seq: ledger.seq + 1,
        prev: ledger.head.clone(),
        date: today(),
        unix: unix_now(),
        actor,
        action,
        prior,
    };
    let line =
        render(&entry, owner.filter(|_| entry.action.owner_signed())).map_err(AppendError::Io)?;
    let mut f = OpenOptions::new()
        .append(true)
        .open(paths.ledger())
        .map_err(|e| AppendError::Io(e.to_string()))?;
    writeln!(f, "{line}").map_err(|e| AppendError::Io(e.to_string()))?;
    f.sync_data().map_err(|e| AppendError::Io(e.to_string()))?;
    ledger.head = line_hash(&line);
    ledger.seq = entry.seq;
    Ok(ledger)
}

#[derive(Debug)]
pub enum AppendError {
    Uninitialized,
    Missing,
    Unverified(String),
    /// The entry itself is not valid against the current policy.
    Refused(String),
    Io(String),
}

impl std::fmt::Display for AppendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uninitialized => write!(f, "auth is not initialised - run `gamebus-setup auth init`"),
            Self::Missing => write!(f, "the ledger is missing - re-initialise it with `gamebus-setup auth reset-ledger`"),
            Self::Unverified(e) => write!(f, "the ledger does not verify ({e}) - re-initialise it with `gamebus-setup auth reset-ledger`"),
            Self::Refused(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Record an edit made outside MCP (the CLI, or the TUI under the
/// `everything` strategy). Before `auth init` there is no ledger and nothing
/// to record, so the edit simply proceeds. After it, a missing or unverified
/// ledger refuses the edit: the policy that would allow it cannot be read.
pub fn record_local(origin: Origin, action: Action, prior: Value) -> Result<(), AppendError> {
    // A unit test must never append to the user's real ledger.
    let paths = if cfg!(test) {
        None
    } else {
        AuthPaths::default_paths()
    };
    match paths {
        Some(paths) => record_local_at(&paths, origin, action, prior),
        None => Ok(()),
    }
}

fn record_local_at(
    paths: &AuthPaths,
    origin: Origin,
    action: Action,
    prior: Value,
) -> Result<(), AppendError> {
    let ledger = match open(paths) {
        LedgerState::Uninitialized => return Ok(()),
        LedgerState::Missing => return Err(AppendError::Missing),
        LedgerState::Unverified(e) => return Err(AppendError::Unverified(e)),
        LedgerState::Ok(l) => l,
    };
    let recorded = match ledger.settings.strategy {
        Strategy::OptIn => false,
        Strategy::McpEdits => origin == Origin::Cli,
        Strategy::Everything => true,
    };
    if !recorded {
        return Ok(());
    }
    let actor = Actor {
        origin,
        name: Some(
            if origin == Origin::Cli {
                "unauthenticated CLI"
            } else {
                "TUI"
            }
            .into(),
        ),
        key: None,
        process: process_tree(),
    };
    append(paths, actor, action, prior, None).map(|_| ())
}

/// Create the owner key and a fresh genesis. Refuses when an owner exists.
pub fn init(paths: &AuthPaths, passphrase: &str) -> Result<Ledger, String> {
    let _lock = DirLock::take(paths)?;
    if paths.owner().exists() {
        return Err("auth is already initialised".into());
    }
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| format!("randomness: {e}"))?;
    let key = derive(passphrase, &salt, KDF)?;
    let owner = OwnerFile {
        public_key: hex(key.verifying_key().as_bytes()),
        salt: hex(&salt),
        kdf: KDF,
    };
    write_genesis(
        paths,
        &key,
        &owner.public_key,
        Settings::default(),
        BTreeMap::new(),
    )?;
    let body = serde_json::to_string_pretty(&owner).map_err(|e| e.to_string())?;
    std::fs::write(paths.owner(), body).map_err(|e| e.to_string())?;
    match open(paths) {
        LedgerState::Ok(l) => Ok(l),
        other => Err(format!("the new ledger does not verify: {other:?}")),
    }
}

fn write_genesis(
    paths: &AuthPaths,
    key: &SigningKey,
    owner_hex: &str,
    settings: Settings,
    clients: BTreeMap<String, ClientRecord>,
) -> Result<(), String> {
    let entry = Entry {
        seq: 0,
        prev: String::new(),
        date: today(),
        unix: unix_now(),
        actor: Actor {
            origin: Origin::Owner,
            name: None,
            key: None,
            process: process_tree(),
        },
        action: Action::Genesis {
            owner_key: owner_hex.to_string(),
            settings,
            clients,
        },
        prior: Value::Null,
    };
    let line = render(&entry, Some(key))?;
    let tmp = paths.ledger().with_extension("jsonl.tmp");
    std::fs::write(&tmp, format!("{line}\n")).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, paths.ledger()).map_err(|e| e.to_string())
}

/// Derive the owner key from a passphrase, refusing one that does not match
/// the public key on record.
pub fn owner_key(paths: &AuthPaths, passphrase: &str) -> Result<SigningKey, String> {
    let owner = read_owner(paths).ok_or("auth is not initialised")?;
    let salt = unhex(&owner.salt).ok_or("the salt on disk is malformed")?;
    let key = derive(passphrase, &salt, owner.kdf)?;
    if hex(key.verifying_key().as_bytes()) != owner.public_key {
        return Err("wrong passphrase".into());
    }
    Ok(key)
}

/// Start a new ledger under the existing owner key. The old one is kept
/// beside it. Settings and clients carry over when the old ledger still
/// verifies; otherwise the defaults apply and clients start afresh.
pub fn reset(paths: &AuthPaths, key: &SigningKey) -> Result<Ledger, String> {
    let _lock = DirLock::take(paths)?;
    let owner = read_owner(paths).ok_or("auth is not initialised")?;
    let (settings, clients) = match open(paths) {
        LedgerState::Ok(l) => (l.settings, l.clients),
        _ => (Settings::default(), BTreeMap::new()),
    };
    if paths.ledger().exists() {
        let kept = paths.dir.join(format!("ledger.{}.jsonl", unix_now()));
        std::fs::rename(paths.ledger(), kept).map_err(|e| e.to_string())?;
    }
    write_genesis(paths, key, &owner.public_key, settings, clients)?;
    match open(paths) {
        LedgerState::Ok(l) => Ok(l),
        other => Err(format!("the new ledger does not verify: {other:?}")),
    }
}

/// Whether a line with this hash is in the current ledger file. What a
/// second party asks to tell "the chain moved on" from "the chain was
/// rewritten": a head it remembered must still be in there.
pub fn contains(paths: &AuthPaths, hash: &str) -> bool {
    let Ok(file) = File::open(paths.ledger()) else {
        return false;
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| line_hash(&line) == hash)
}

/// Every entry of a verified or unverified ledger, for the audit views.
pub fn entries(paths: &AuthPaths) -> Result<Vec<(Entry, bool)>, String> {
    let file = File::open(paths.ledger()).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let parsed: Line = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        let entry: Entry = serde_json::from_str(parsed.entry.get()).map_err(|e| e.to_string())?;
        out.push((entry, parsed.sig.is_some()));
    }
    Ok(out)
}

/// A client refused under explicit approval, waiting for the owner. Not
/// part of the ledger: nothing is decided until the owner approves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    pub key: String,
    pub process: Vec<Proc>,
    pub date: String,
}

pub fn note_pending(paths: &AuthPaths, name: &str, pending: Pending) {
    let mut all = pending_requests(paths);
    all.insert(name.to_string(), pending);
    if let Ok(body) = serde_json::to_string_pretty(&all) {
        let _ = std::fs::create_dir_all(&paths.dir);
        let _ = std::fs::write(paths.pending(), body);
    }
}

pub fn pending_requests(paths: &AuthPaths) -> BTreeMap<String, Pending> {
    std::fs::read_to_string(paths.pending())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn clear_pending(paths: &AuthPaths, name: &str) {
    let mut all = pending_requests(paths);
    if all.remove(name).is_some() {
        if let Ok(body) = serde_json::to_string_pretty(&all) {
            let _ = std::fs::write(paths.pending(), body);
        }
    }
}

/// This process's ancestors, nearest first: over stdio the first one is the
/// client that spawned us.
pub fn process_tree() -> Vec<Proc> {
    let mut out = Vec::new();
    let mut pid = parent_of(std::process::id());
    while let Some(p) = pid.filter(|p| *p > 1) {
        if out.len() == 8 {
            break;
        }
        let name = std::fs::read_to_string(format!("/proc/{p}/comm"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let exe = std::fs::read_link(format!("/proc/{p}/exe"))
            .ok()
            .map(|e| e.display().to_string());
        out.push(Proc { pid: p, name, exe });
        pid = parent_of(p);
    }
    out
}

fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The name is parenthesised and may itself contain spaces or parens.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Read a passphrase from the terminal without echoing it.
pub fn read_passphrase(prompt: &str) -> Result<String, String> {
    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|e| format!("a passphrase needs a terminal: {e}"))?;
    let fd = tty.as_raw_fd();
    // SAFETY: termios calls on a terminal descriptor we own; the original
    // mode is restored before returning.
    let mut original: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
        return Err("cannot read the terminal mode".into());
    }
    let mut silent = original;
    silent.c_lflag &= !libc::ECHO;
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &silent) };
    let mut out = &tty;
    let _ = write!(out, "{prompt}");
    let mut line = String::new();
    let read = BufReader::new(&tty).read_line(&mut line);
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &original) };
    let _ = writeln!(out);
    read.map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// The owner actions, shared by `gamebus-setup auth` and the TUI. Each
/// derives the owner key from the passphrase (refusing a wrong one) and
/// appends one owner-signed entry recording what it replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerOp {
    /// Approve the key waiting under this name: a new client under explicit
    /// approval, or a changed key after a reinstall. With nothing waiting,
    /// promotes a first-use registration to approved.
    Approve(String),
    Forget(String),
    SetSettings(Settings),
}

pub fn owner_op(paths: &AuthPaths, passphrase: &str, op: &OwnerOp) -> Result<String, String> {
    let ledger = match open(paths) {
        LedgerState::Ok(l) => l,
        LedgerState::Uninitialized => {
            return Err("Auth is not initialised: run `gamebus-setup auth init`.".into())
        }
        LedgerState::Missing => {
            return Err(
                "The ledger is missing: `gamebus-setup auth reset-ledger` starts a new one.".into(),
            )
        }
        LedgerState::Unverified(e) => return Err(format!("The ledger does not verify ({e}).")),
    };
    let owner = Actor {
        origin: Origin::Owner,
        name: None,
        key: None,
        process: process_tree(),
    };
    let (action, prior, done) = match op {
        OwnerOp::Approve(name) => {
            let key = match pending_requests(paths).remove(name) {
                Some(p) => p.key,
                None => ledger
                    .clients
                    .get(name)
                    .map(|r| r.key.clone())
                    .ok_or_else(|| {
                        format!("No request from {name} is waiting; let it connect once first.")
                    })?,
            };
            let prior = ledger
                .clients
                .get(name)
                .map_or(Value::Null, |r| serde_json::json!(r));
            let done = format!("Approved {name} ({}).", fingerprint(&key));
            (
                Action::ApproveClient {
                    name: name.clone(),
                    key,
                },
                prior,
                done,
            )
        }
        OwnerOp::Forget(name) => {
            let rec = ledger
                .clients
                .get(name)
                .ok_or_else(|| format!("{name} is not on record."))?;
            (
                Action::ForgetClient { name: name.clone() },
                serde_json::json!(rec),
                format!("Forgot {name}; its next connection is treated as new."),
            )
        }
        OwnerOp::SetSettings(settings) => {
            if *settings == ledger.settings {
                return Ok("Nothing to change.".into());
            }
            (
                Action::SetSettings {
                    settings: *settings,
                },
                serde_json::json!(ledger.settings),
                "Policy changed and recorded.".to_string(),
            )
        }
    };
    let key = owner_key(paths, passphrase)?;
    append(paths, owner, action, prior, Some(&key)).map_err(|e| e.to_string())?;
    if let OwnerOp::Approve(name) | OwnerOp::Forget(name) = op {
        clear_pending(paths, name);
    }
    Ok(done)
}

/// An entry in words: one sentence for what happened, and labelled lines
/// for only the fields it touched, each with the value it replaced. Shared
/// by the audit tab and `auth log`.
pub fn describe(e: &Entry) -> (String, Vec<(&'static str, String)>) {
    let mut lines: Vec<(&'static str, String)> = Vec::new();
    let prior = &e.prior;
    let was = |field: &str| -> String {
        match prior.get(field) {
            None | Some(Value::Null) => "unset".to_string(),
            Some(Value::String(s)) => format!("'{s}'"),
            Some(v) => v.to_string(),
        }
    };
    let sentence = match &e.action {
        Action::Genesis {
            settings, clients, ..
        } => {
            lines.push(("Policy", settings_words(*settings)));
            if !clients.is_empty() {
                let names: Vec<&str> = clients.keys().map(String::as_str).collect();
                lines.push(("Carried over", names.join(", ")));
            }
            "Ledger started".to_string()
        }
        Action::SetSettings { settings } => {
            lines.push(("Now", settings_words(*settings)));
            if let Ok(before) = serde_json::from_value::<Settings>(prior.clone()) {
                lines.push(("Was", settings_words(before)));
            }
            "Policy changed".to_string()
        }
        Action::ApproveClient { name, key } => {
            lines.push(("Key", fingerprint(key)));
            match prior.get("key").and_then(Value::as_str) {
                Some(old) if old != key => lines.push(("Replaced key", fingerprint(old))),
                Some(_) => lines.push(("Was", "remembered on first use".into())),
                None => {}
            }
            format!("Approved {name}")
        }
        Action::ForgetClient { name } => {
            if let Some(old) = prior.get("key").and_then(Value::as_str) {
                lines.push(("Had key", fingerprint(old)));
            }
            format!("Forgot {name}")
        }
        Action::RegisterClient { name, key } => {
            lines.push(("Key", fingerprint(key)));
            format!("{name} remembered on first use")
        }
        Action::EditMiss { key, verb, change } => {
            lines.push(("Entry", key.clone()));
            if let Some(title) = prior
                .get("title_override")
                .and_then(Value::as_str)
                .or_else(|| prior.get("title").and_then(Value::as_str))
            {
                lines.push(("Game", title.to_string()));
            }
            let sentence = match verb {
                MissVerb::Dismiss => {
                    lines.push(("Dismissed was", was("dismissed")));
                    "Dismissed an identity miss".to_string()
                }
                MissVerb::Undismiss => {
                    lines.push(("Dismissed was", was("dismissed")));
                    "Restored an identity miss".to_string()
                }
                MissVerb::Promote | MissVerb::Demote => {
                    lines.push(("Promoted was", was("umu_promoted")));
                    if matches!(verb, MissVerb::Promote) {
                        "Promoted into the umu pipeline".to_string()
                    } else {
                        "Took the umu promotion back".to_string()
                    }
                }
                MissVerb::SetTitle { title } => {
                    lines.push(("Title", format!("'{title}'")));
                    lines.push(("Override was", was("title_override")));
                    "Corrected the title".to_string()
                }
                MissVerb::SetStore { store } => {
                    lines.push(("Store", store.clone()));
                    lines.push(("Override was", was("store_override")));
                    "Corrected the store".to_string()
                }
                MissVerb::SetIdentity { store, codename } => {
                    lines.push(("Codename", codename.clone()));
                    lines.push(("Codename was", was("codename_override")));
                    if let Some(st) = store {
                        lines.push(("Store", st.clone()));
                    }
                    "Set the store identity".to_string()
                }
                MissVerb::AssignId { id } => {
                    lines.push(("Umu id", id.clone()));
                    let before = prior
                        .pointer("/drafted_id/id")
                        .and_then(Value::as_str)
                        .map_or("none".to_string(), |d| format!("'{d}'"));
                    lines.push(("Draft was", before));
                    "Assigned a umu id".to_string()
                }
                MissVerb::PickEntry {
                    store,
                    codename,
                    umu_id,
                } => {
                    lines.push(("Picked", format!("{umu_id} ({store}/{codename})")));
                    "Picked a umu-database entry".to_string()
                }
            };
            lines.push(("Result", change_words(change)));
            sentence
        }
        Action::EditFinding { key, verb, .. } => {
            lines.push(("Finding", key.clone()));
            if let Some(title) = prior.get("title").and_then(Value::as_str) {
                lines.push(("Game", title.to_string()));
            }
            match verb {
                FindingVerb::Dismiss => {
                    lines.push(("Dismissed was", was("dismissed")));
                    "Dismissed a compat finding".to_string()
                }
                FindingVerb::Undismiss => {
                    lines.push(("Dismissed was", was("dismissed")));
                    "Restored a compat finding".to_string()
                }
                FindingVerb::MarkReported { target } => {
                    let before = prior
                        .pointer(&format!("/reported/{target}"))
                        .and_then(Value::as_str)
                        .map_or("not reported".to_string(), |d| format!("reported {d}"));
                    lines.push(("Before", before));
                    format!("Marked reported to {target}")
                }
            }
        }
        Action::DrainInbox { keys } => {
            lines.push(("Findings", keys.join(", ")));
            format!("Took {} finding(s) from the inbox", keys.len())
        }
    };
    (sentence, lines)
}

fn change_words(c: &MissChange) -> String {
    match c {
        MissChange::Dismissed => "dismissed".into(),
        MissChange::Restored => "restored".into(),
        MissChange::Promoted => "promoted".into(),
        MissChange::Demoted => "promotion taken back".into(),
        MissChange::TitleSet { .. } => "title overridden".into(),
        MissChange::TitleReset => "back to the resolver's title".into(),
        MissChange::StoreSet { guessed } => format!("store overridden (daemon guessed {guessed})"),
        MissChange::StoreReset => "back to the daemon's store guess".into(),
        MissChange::IdentitySet => "identity set".into(),
        MissChange::IdAssigned => "id drafted by hand".into(),
        MissChange::AlreadyInDatabase => "already in the database (launcher-side miss)".into(),
        MissChange::CrossStoreId => "shares another store's id".into(),
    }
}

fn settings_words(s: Settings) -> String {
    let registration = match s.registration {
        Registration::TrustOnFirstUse => "new clients remembered on first use",
        Registration::ExplicitApproval => "new clients need approval",
    };
    let strategy = match s.strategy {
        Strategy::McpEdits => "MCP and CLI edits recorded",
        Strategy::OptIn => "no edits recorded",
        Strategy::Everything => "MCP, CLI and TUI edits recorded",
    };
    format!("{registration}; {strategy}")
}

/// Where a client stands, for the TUI's auth tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientStatus {
    Approved,
    Remembered,
    /// A key waiting for the owner: new under explicit approval, or a
    /// changed key under a name already on record.
    Waiting {
        date: String,
        from: String,
        replaces: bool,
    },
}

#[derive(Debug, Clone)]
pub struct ClientRow {
    pub name: String,
    pub fingerprint: String,
    pub status: ClientStatus,
}

/// Everything the auth and audit tabs show, loaded off the render path.
#[derive(Debug, Clone, Default)]
pub struct AuthView {
    pub dir: String,
    /// `None` before `auth init`; `Some(Err)` when the ledger is missing or
    /// does not verify.
    pub state: Option<Result<(), String>>,
    pub ledger_id: Option<String>,
    pub head: Option<String>,
    pub initialised: Option<String>,
    pub settings: Option<Settings>,
    pub clients: Vec<ClientRow>,
    /// Newest first, with whether each carries an owner signature.
    pub entries: Vec<(Entry, bool)>,
}

pub fn view(paths: &AuthPaths) -> AuthView {
    let mut v = AuthView {
        dir: paths.dir.display().to_string(),
        ..AuthView::default()
    };
    let state = open(paths);
    if matches!(state, LedgerState::Uninitialized) {
        return v;
    }
    v.entries = entries(paths).unwrap_or_default();
    if let Some((genesis, _)) = v.entries.first() {
        let chain: Vec<&str> = genesis
            .actor
            .process
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        v.initialised = Some(format!("{} from {}", genesis.date, chain.join(" <- ")));
    }
    v.entries.reverse();
    let pending = pending_requests(paths);
    match state {
        LedgerState::Ok(l) => {
            v.state = Some(Ok(()));
            v.ledger_id = Some(l.id);
            v.head = Some(l.head);
            v.settings = Some(l.settings);
            for (name, rec) in &l.clients {
                v.clients.push(ClientRow {
                    name: name.clone(),
                    fingerprint: fingerprint(&rec.key),
                    status: if rec.approved {
                        ClientStatus::Approved
                    } else {
                        ClientStatus::Remembered
                    },
                });
            }
            for (name, p) in pending {
                let replaces = l.clients.contains_key(&name);
                v.clients.push(ClientRow {
                    name,
                    fingerprint: fingerprint(&p.key),
                    status: ClientStatus::Waiting {
                        date: p.date,
                        from: p
                            .process
                            .first()
                            .map(|p| p.name.clone())
                            .unwrap_or_default(),
                        replaces,
                    },
                });
            }
        }
        LedgerState::Missing => v.state = Some(Err("the ledger is missing".into())),
        LedgerState::Unverified(e) => {
            v.state = Some(Err(format!("the ledger does not verify: {e}")))
        }
        LedgerState::Uninitialized => unreachable!("returned above"),
    }
    v
}

#[cfg(test)]
pub(crate) fn test_paths(tag: &str) -> AuthPaths {
    let dir = std::env::temp_dir().join(format!("gamebus-auth-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    AuthPaths { dir }
}

#[cfg(test)]
pub(crate) fn fast_init(paths: &AuthPaths, passphrase: &str) -> SigningKey {
    // The real cost makes a test suite crawl; the chain rules are the same.
    std::fs::create_dir_all(&paths.dir).unwrap();
    let salt = [7u8; 16];
    let kdf = Kdf {
        m_kib: 8,
        t: 1,
        p: 1,
    };
    let key = derive(passphrase, &salt, kdf).unwrap();
    let owner = OwnerFile {
        public_key: hex(key.verifying_key().as_bytes()),
        salt: hex(&salt),
        kdf,
    };
    write_genesis(
        paths,
        &key,
        &owner.public_key,
        Settings::default(),
        BTreeMap::new(),
    )
    .unwrap();
    std::fs::write(paths.owner(), serde_json::to_string(&owner).unwrap()).unwrap();
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(name: &str, key: &str) -> Actor {
        Actor {
            origin: Origin::Client,
            name: Some(name.into()),
            key: Some(key.into()),
            process: vec![],
        }
    }

    fn owner_actor() -> Actor {
        Actor {
            origin: Origin::Owner,
            name: None,
            key: None,
            process: vec![],
        }
    }

    fn state(paths: &AuthPaths) -> Ledger {
        match open(paths) {
            LedgerState::Ok(l) => l,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn states_before_and_after_init() {
        let paths = test_paths("states");
        assert!(matches!(open(&paths), LedgerState::Uninitialized));
        fast_init(&paths, "pw");
        let l = state(&paths);
        assert_eq!(l.id, l.head);
        assert_eq!(l.settings, Settings::default());
        std::fs::remove_file(paths.ledger()).unwrap();
        assert!(matches!(open(&paths), LedgerState::Missing));
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn tofu_remembers_the_first_key_and_edits_chain_on() {
        let paths = test_paths("tofu");
        fast_init(&paths, "pw");
        let id = state(&paths).id;
        let reg = Action::RegisterClient {
            name: "beisl-compat".into(),
            key: "aa".into(),
        };
        append(&paths, client("beisl-compat", "aa"), reg, Value::Null, None).unwrap();
        // The same name again is refused: only the owner can change a key.
        let again = Action::RegisterClient {
            name: "beisl-compat".into(),
            key: "bb".into(),
        };
        assert!(matches!(
            append(
                &paths,
                client("beisl-compat", "bb"),
                again,
                Value::Null,
                None
            ),
            Err(AppendError::Refused(_))
        ));
        let edit = Action::EditFinding {
            key: "steam:1".into(),
            verb: FindingVerb::Dismiss,
            change: FindingChange::Dismissed,
        };
        let l = append(
            &paths,
            client("beisl-compat", "aa"),
            edit,
            Value::Null,
            None,
        )
        .unwrap();
        assert_eq!(l.seq, 2);
        assert_eq!(l.id, id, "appending never changes the ledger's identity");
        // An edit under a key that is not on record breaks nothing: it is refused.
        let forged = Action::EditFinding {
            key: "steam:1".into(),
            verb: FindingVerb::Undismiss,
            change: FindingChange::Restored,
        };
        assert!(append(
            &paths,
            client("beisl-compat", "cc"),
            forged,
            Value::Null,
            None
        )
        .is_err());
        assert_eq!(state(&paths).seq, 2);
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn owner_actions_need_the_owner_and_a_hand_edit_breaks_the_chain() {
        let paths = test_paths("owner");
        let key = fast_init(&paths, "pw");
        let explicit = Action::SetSettings {
            settings: Settings {
                registration: Registration::ExplicitApproval,
                strategy: Strategy::McpEdits,
            },
        };
        assert!(append(&paths, owner_actor(), explicit.clone(), Value::Null, None).is_err());
        append(&paths, owner_actor(), explicit, Value::Null, Some(&key)).unwrap();
        assert_eq!(
            state(&paths).settings.registration,
            Registration::ExplicitApproval
        );
        // Now no first-use registration is valid.
        let reg = Action::RegisterClient {
            name: "agent".into(),
            key: "aa".into(),
        };
        assert!(append(&paths, client("agent", "aa"), reg, Value::Null, None).is_err());

        // Flip the policy back by hand: the chain no longer verifies.
        let raw = std::fs::read_to_string(paths.ledger()).unwrap();
        std::fs::write(
            paths.ledger(),
            raw.replace("explicit-approval", "trust-on-first-use"),
        )
        .unwrap();
        assert!(matches!(open(&paths), LedgerState::Unverified(_)));
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn a_reset_gets_a_new_identity_and_keeps_the_old_file() {
        let paths = test_paths("reset");
        let key = fast_init(&paths, "pw");
        let old = state(&paths).id;
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let new = reset(&paths, &key).unwrap();
        assert_ne!(new.id, old);
        let kept = std::fs::read_dir(&paths.dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("ledger."))
            .count();
        assert_eq!(kept, 2, "the old ledger is kept beside the new one");
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn init_at_the_real_cost_and_only_the_right_passphrase_derives_the_owner() {
        let paths = test_paths("realinit");
        let ledger = init(&paths, "correct horse").unwrap();
        assert_eq!(ledger.seq, 0);
        assert!(init(&paths, "again").is_err(), "a second init must refuse");
        assert!(owner_key(&paths, "correct horse").is_ok());
        assert_eq!(
            owner_key(&paths, "wrong").err().as_deref(),
            Some("wrong passphrase")
        );
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn a_reinstalled_client_is_waiting_until_the_owner_approves_its_new_key() {
        let paths = test_paths("reinstall");
        fast_init(&paths, "pw");
        let reg = Action::RegisterClient {
            name: "beisl-compat".into(),
            key: "aa".into(),
        };
        append(&paths, client("beisl-compat", "aa"), reg, Value::Null, None).unwrap();
        // The server parks the new key when it sees the mismatch.
        note_pending(
            &paths,
            "beisl-compat",
            Pending {
                key: "bb".into(),
                process: vec![],
                date: "2026-09-27".into(),
            },
        );
        let v = view(&paths);
        assert!(v
            .clients
            .iter()
            .any(|c| matches!(c.status, ClientStatus::Waiting { replaces: true, .. })));

        let op = OwnerOp::Approve("beisl-compat".into());
        assert_eq!(
            owner_op(&paths, "nope", &op).err().as_deref(),
            Some("wrong passphrase")
        );
        assert_eq!(
            state(&paths).clients["beisl-compat"].key,
            "aa",
            "a wrong passphrase changed nothing"
        );

        owner_op(&paths, "pw", &op).unwrap();
        let l = state(&paths);
        assert_eq!(l.clients["beisl-compat"].key, "bb");
        assert!(l.clients["beisl-compat"].approved);
        assert!(pending_requests(&paths).is_empty());
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn the_view_reads_newest_first_and_names_the_origin() {
        let paths = test_paths("view");
        assert!(view(&paths).state.is_none());
        let key = fast_init(&paths, "pw");
        let set = OwnerOp::SetSettings(Settings {
            registration: Registration::TrustOnFirstUse,
            strategy: Strategy::Everything,
        });
        owner_op(&paths, "pw", &set).unwrap();
        let _ = key;
        let v = view(&paths);
        assert_eq!(v.state, Some(Ok(())));
        assert_eq!(v.entries.len(), 2);
        assert_eq!(v.entries[0].0.seq, 1, "newest first");
        assert!(v.entries[0].1, "a policy change is owner-signed");
        assert!(v.initialised.is_some());
        assert_eq!(v.settings.unwrap().strategy, Strategy::Everything);
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn local_edits_follow_the_strategy_and_a_broken_ledger_blocks_them() {
        let paths = test_paths("local");
        let edit = || Action::EditFinding {
            key: "steam:1".into(),
            verb: FindingVerb::Dismiss,
            change: FindingChange::Dismissed,
        };
        // Before init there is nothing to record into; the edit proceeds.
        record_local_at(&paths, Origin::Cli, edit(), Value::Null).unwrap();
        fast_init(&paths, "pw");
        record_local_at(&paths, Origin::Cli, edit(), Value::Null).unwrap();
        record_local_at(&paths, Origin::Tui, edit(), Value::Null).unwrap();
        let recorded: Vec<Origin> = entries(&paths)
            .unwrap()
            .iter()
            .skip(1)
            .map(|(e, _)| e.actor.origin)
            .collect();
        assert_eq!(
            recorded,
            [Origin::Cli],
            "the default records the CLI, not the TUI"
        );

        std::fs::remove_file(paths.ledger()).unwrap();
        assert!(matches!(
            record_local_at(&paths, Origin::Tui, edit(), Value::Null),
            Err(AppendError::Missing)
        ));
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn entries_read_as_words_with_what_they_replaced() {
        let paths = test_paths("words");
        fast_init(&paths, "pw");
        let reg = Action::RegisterClient {
            name: "beisl-compat".into(),
            key: "aa".into(),
        };
        append(&paths, client("beisl-compat", "aa"), reg, Value::Null, None).unwrap();
        let edit = Action::EditMiss {
            key: "lutris:severed-steel".into(),
            verb: MissVerb::SetTitle {
                title: "Severed Steel".into(),
            },
            change: MissChange::TitleSet {
                resolved: Some("ThankYouVeryCool".into()),
            },
        };
        let prior = serde_json::json!({"title": "ThankYouVeryCool", "title_override": null});
        append(&paths, client("beisl-compat", "aa"), edit, prior, None).unwrap();

        let all = entries(&paths).unwrap();
        let (what, facts) = describe(&all[2].0);
        assert_eq!(what, "Corrected the title");
        let facts: BTreeMap<_, _> = facts.into_iter().collect();
        assert_eq!(facts["Game"], "ThankYouVeryCool");
        assert_eq!(facts["Title"], "'Severed Steel'");
        assert_eq!(facts["Override was"], "unset");
        assert_eq!(facts["Result"], "title overridden");
        // Nothing reads as raw JSON.
        assert!(facts.values().all(|v| !v.starts_with('{')));
        assert_eq!(describe(&all[0].0).0, "Ledger started");
        assert_eq!(
            describe(&all[1].0).0,
            "beisl-compat remembered on first use"
        );
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn the_process_tree_starts_at_our_parent() {
        let tree = process_tree();
        assert!(!tree.is_empty());
        assert_ne!(tree[0].pid, std::process::id());
    }
}
