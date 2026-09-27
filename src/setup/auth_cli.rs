//! `gamebus-setup auth`: the owner's side of client identities and the
//! ledger. Every command that changes policy asks for the passphrase on the
//! terminal; none takes it from an argument or the environment, where an
//! agent could supply it.

use std::process::ExitCode;

use serde_json::{json, Value};

use super::auth::{
    self, Action, Actor, AuthPaths, Ledger, LedgerState, Origin, Registration, Settings, Strategy,
};

pub fn run(args: &[String]) -> ExitCode {
    let Some(paths) = AuthPaths::default_paths() else {
        eprintln!("Cannot resolve the data directory (no HOME).");
        return ExitCode::FAILURE;
    };
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let result = match rest.first().copied() {
        Some("init") => init(&paths),
        Some("status") | None => status(&paths),
        Some("approve") => approve(&paths, &rest[1..]),
        Some("forget") => forget(&paths, &rest[1..]),
        Some("set") => set(&paths, &rest[1..]),
        Some("reset-ledger") => reset(&paths),
        Some("log") => log(&paths, rest.contains(&"--json")),
        Some(other) => Err(format!("unknown auth command: {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn owner() -> Actor {
    Actor {
        origin: Origin::Owner,
        name: None,
        key: None,
        process: auth::process_tree(),
    }
}

fn verified(paths: &AuthPaths) -> Result<Ledger, String> {
    match auth::open(paths) {
        LedgerState::Ok(l) => Ok(l),
        LedgerState::Uninitialized => Err("Auth is not initialised: run `gamebus-setup auth init`.".into()),
        LedgerState::Missing => Err("The ledger is missing: `gamebus-setup auth reset-ledger` starts a new one.".into()),
        LedgerState::Unverified(e) => Err(format!(
            "The ledger does not verify ({e}). `gamebus-setup auth reset-ledger` starts a new one; the old file is kept."
        )),
    }
}

fn init(paths: &AuthPaths) -> Result<(), String> {
    if !matches!(auth::open(paths), LedgerState::Uninitialized) {
        return Err("Auth is already initialised. `gamebus-setup auth status` shows it.".into());
    }
    println!("This creates the owner key for gamebus-setup's MCP server, in");
    println!("  {}", paths.dir.display());
    println!("The passphrase is the one thing an agent running as you does not have:");
    println!("approving clients, changing the policy and resetting the ledger need it.");
    println!("It is never stored. Lose it and only a full re-init restores owner control.");
    let first = auth::read_passphrase("New passphrase: ")?;
    if first.len() < 8 {
        return Err("Use at least 8 characters.".into());
    }
    let second = auth::read_passphrase("Again: ")?;
    if first != second {
        return Err("The two entries differ; nothing was created.".into());
    }
    let ledger = auth::init(paths, &first)?;
    println!("Initialised. Ledger {}.", short(&ledger.id));
    println!(
        "New clients are remembered on first use; `gamebus-setup auth set --registration explicit`"
    );
    println!("requires your approval instead.");
    Ok(())
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

fn status(paths: &AuthPaths) -> Result<(), String> {
    let ledger = match auth::open(paths) {
        LedgerState::Uninitialized => {
            println!("Auth is not initialised: MCP clients can read but not edit.");
            println!("`gamebus-setup auth init` sets it up.");
            return Ok(());
        }
        _ => verified(paths)?,
    };
    println!(
        "Ledger {} (head {}, {} entries)",
        short(&ledger.id),
        short(&ledger.head),
        ledger.seq + 1
    );
    println!("Auth directory: {}", paths.dir.display());
    // Whoever runs `auth init` first becomes the owner, and nothing at this
    // layer can stop another process running as this user from doing it
    // first. So say who did, where the owner will look.
    if let Some((genesis, _)) = auth::entries(paths).ok().and_then(|e| e.into_iter().next()) {
        let chain: Vec<&str> = genesis
            .actor
            .process
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        println!("Initialised {} from {}", genesis.date, chain.join(" <- "));
    }
    println!(
        "Registration: {}   Recorded edits: {}",
        match ledger.settings.registration {
            Registration::TrustOnFirstUse => "first use remembered",
            Registration::ExplicitApproval => "owner approval required",
        },
        match ledger.settings.strategy {
            Strategy::McpEdits => "MCP and CLI",
            Strategy::OptIn => "none",
            Strategy::Everything => "MCP, CLI and TUI",
        }
    );
    if ledger.clients.is_empty() {
        println!("No clients on record.");
    }
    for (name, rec) in &ledger.clients {
        println!(
            "  {name:<20} {}  {}",
            auth::fingerprint(&rec.key),
            if rec.approved {
                "approved"
            } else {
                "remembered on first use"
            }
        );
    }
    for (name, p) in auth::pending_requests(paths) {
        let from = p.process.first().map(|p| p.name.as_str()).unwrap_or("?");
        println!(
            "  {name:<20} {}  WAITING for approval since {} (from {from})",
            auth::fingerprint(&p.key),
            p.date
        );
    }
    Ok(())
}

fn approve(paths: &AuthPaths, args: &[&str]) -> Result<(), String> {
    let name = args
        .first()
        .ok_or("Usage: gamebus-setup auth approve <name>")?;
    let ledger = verified(paths)?;
    let key = match auth::pending_requests(paths).remove(*name) {
        Some(p) => p.key,
        None => {
            let rec = ledger.clients.get(*name).ok_or_else(|| {
                format!("No request from {name} is waiting; let it connect once first.")
            })?;
            rec.key.clone()
        }
    };
    println!("Approving {name} with key {}.", auth::fingerprint(&key));
    let owner_key = auth::owner_key(paths, &auth::read_passphrase("Passphrase: ")?)?;
    let prior = ledger.clients.get(*name).map_or(Value::Null, |r| json!(r));
    let action = Action::ApproveClient {
        name: name.to_string(),
        key,
    };
    auth::append(paths, owner(), action, prior, Some(&owner_key)).map_err(|e| e.to_string())?;
    auth::clear_pending(paths, name);
    println!("Approved.");
    Ok(())
}

fn forget(paths: &AuthPaths, args: &[&str]) -> Result<(), String> {
    let name = args
        .first()
        .ok_or("Usage: gamebus-setup auth forget <name>")?;
    let ledger = verified(paths)?;
    let Some(rec) = ledger.clients.get(*name) else {
        auth::clear_pending(paths, name);
        return Err(format!("{name} is not on record."));
    };
    let owner_key = auth::owner_key(paths, &auth::read_passphrase("Passphrase: ")?)?;
    let action = Action::ForgetClient {
        name: name.to_string(),
    };
    auth::append(paths, owner(), action, json!(rec), Some(&owner_key))
        .map_err(|e| e.to_string())?;
    println!("Forgot {name}. Its next connection is treated as new.");
    Ok(())
}

fn set(paths: &AuthPaths, args: &[&str]) -> Result<(), String> {
    let ledger = verified(paths)?;
    let mut settings: Settings = ledger.settings;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match (*flag, *value) {
            ("--registration", "tofu") => settings.registration = Registration::TrustOnFirstUse,
            ("--registration", "explicit") => {
                settings.registration = Registration::ExplicitApproval
            }
            ("--strategy", "mcp") => settings.strategy = Strategy::McpEdits,
            ("--strategy", "opt-in") => settings.strategy = Strategy::OptIn,
            ("--strategy", "everything") => settings.strategy = Strategy::Everything,
            _ => {
                return Err(format!(
                    "Unknown setting {flag} {value}. --registration tofu|explicit, \
                     --strategy mcp|opt-in|everything."
                ))
            }
        }
    }
    if settings == ledger.settings {
        println!("Nothing to change.");
        return Ok(());
    }
    let owner_key = auth::owner_key(paths, &auth::read_passphrase("Passphrase: ")?)?;
    let action = Action::SetSettings { settings };
    auth::append(
        paths,
        owner(),
        action,
        json!(ledger.settings),
        Some(&owner_key),
    )
    .map_err(|e| e.to_string())?;
    println!("Policy changed and recorded.");
    Ok(())
}

fn reset(paths: &AuthPaths) -> Result<(), String> {
    if matches!(auth::open(paths), LedgerState::Uninitialized) {
        return Err("Auth is not initialised: run `gamebus-setup auth init`.".into());
    }
    println!("This starts a new ledger with a new identity; the old file is kept beside it.");
    println!("Anything that remembered the old ledger (beisl) will re-anchor to the new one.");
    let owner_key = auth::owner_key(paths, &auth::read_passphrase("Passphrase: ")?)?;
    let ledger = auth::reset(paths, &owner_key)?;
    println!("New ledger {}.", short(&ledger.id));
    Ok(())
}

fn log(paths: &AuthPaths, as_json: bool) -> Result<(), String> {
    let entries = auth::entries(paths)?;
    let state = match auth::open(paths) {
        LedgerState::Ok(_) => "verified".to_string(),
        LedgerState::Unverified(e) => format!("DOES NOT VERIFY: {e}"),
        _ => "missing".to_string(),
    };
    if as_json {
        let rows: Vec<Value> = entries
            .iter()
            .map(|(e, signed)| json!({"entry": e, "owner_signed": signed}))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"state": state, "entries": rows}))
                .unwrap_or_default()
        );
        return Ok(());
    }
    println!("Ledger {state}.");
    for (e, _) in &entries {
        let who = match (&e.actor.origin, &e.actor.name) {
            (Origin::Client, Some(n)) => n.clone(),
            (origin, _) => format!("{origin:?}").to_lowercase(),
        };
        let from = e
            .actor
            .process
            .first()
            .map(|p| p.name.as_str())
            .unwrap_or("-");
        let what = serde_json::to_value(&e.action)
            .ok()
            .and_then(|v| v.get("action").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default();
        println!(
            "{:>4} {} {:<16} via {:<16} {what}",
            e.seq, e.date, who, from
        );
    }
    Ok(())
}
