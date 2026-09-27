//! `gamebus-setup auth`: the owner's side of client identities and the
//! ledger. Every command that changes policy asks for the passphrase on the
//! terminal; none takes it from an argument or the environment, where an
//! agent could supply it.

use std::process::ExitCode;

use serde_json::{json, Value};

use super::auth::{self, AuthPaths, Ledger, LedgerState, Origin, Registration, Settings, Strategy};

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
    owner(paths, auth::OwnerOp::Approve(name.to_string()))
}

fn forget(paths: &AuthPaths, args: &[&str]) -> Result<(), String> {
    let name = args
        .first()
        .ok_or("Usage: gamebus-setup auth forget <name>")?;
    owner(paths, auth::OwnerOp::Forget(name.to_string()))
}

fn set(paths: &AuthPaths, args: &[&str]) -> Result<(), String> {
    let mut settings: Settings = verified(paths)?.settings;
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
    owner(paths, auth::OwnerOp::SetSettings(settings))
}

/// Ask for the passphrase and run one owner action.
fn owner(paths: &AuthPaths, op: auth::OwnerOp) -> Result<(), String> {
    verified(paths)?;
    let passphrase = auth::read_passphrase("Passphrase: ")?;
    println!("{}", auth::owner_op(paths, &passphrase, &op)?);
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
        let (what, facts) = auth::describe(e);
        println!(
            "{:>4} {} {:<16} via {:<16} {what}",
            e.seq, e.date, who, from
        );
        for (label, value) in facts {
            println!("{:>8}{label}: {value}", "");
        }
    }
    Ok(())
}
