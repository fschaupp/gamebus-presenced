---
date: 2026-08-04
type: decision
title: Depend on the rsrpc crate for the Discord payload model; own only the transport
status: accepted
related-projects: [gamebus-presenced]
ai-first: true
tags: [decision, architecture, discord, ipc, dependencies]
---

## For future Claude

ADR-006: How S2 reuses rsRPC. The daemon depends on the pinned, MIT-licensed `rsrpc` crate for its public `rsrpc::cmd` payload model (ActivityCmd/Activity + `fix()` normalisation) and implements only the transport itself (tokio UnixListener, handshake/echo, `SO_PEERCRED`), because rsRPC's own IPC listener is crate-private, blocking-threaded, and does not read peer credentials. Discord activity objects are keyed `discord_<pid>`.

## Context

S2 needed a Discord IPC listener. The project's standing decision (design doc, project note) was to use [rsRPC](https://github.com/SpikeHD/rsRPC) (MIT) as the base rather than hand-weaving the protocol. Assessed against the actual crate (rev `062e0fd9fe34a475640a822facfee68296ba22e3`, version 0.28.0, fetched 2026-08-04):

- `rsrpc::cmd` is public: `ActivityCmd`, `Activity`, `Assets`, `Timestamps`, `Party`, plus `fix()` (timestamp seconds-vs-ms normalisation, button/flag fixes). This is the battle-tested part.
- `rsrpc::server` is **private**: the IPC listener (`IpcConnector`), frame codec, and `CONNECTION_RESPONSE` cannot be reached from outside.
- `RPCServer` has no public hook for received `SET_ACTIVITY` events, bundles a WebSocket fan-out (`simple-websockets` git dep), process scanner, and uses `std::thread` + `std::sync::mpsc` + `interprocess` - incompatible with this daemon's tokio architecture.
- rsRPC trusts the client-supplied `args.pid`; the design requires kernel-verified `SO_PEERCRED`.

## Decision

1. **Depend on `rsrpc` as a pinned git crate** (`rev = "062e0fd..."`) and use `rsrpc::cmd` for all `SET_ACTIVITY` parsing, including `fix()` before mapping. No hand-written payload structs, no vendored copy.
2. **Own the transport only**: tokio `UnixListener` on `$XDG_RUNTIME_DIR/discord-ipc-0` (stale-socket unlink, live-owner warning + degrade), per-connection tasks, handshake -> READY, lock-step echo, `SO_PEERCRED` via `libc::getsockopt` for the pid.
3. **Discord activity IDs are `discord_<pid>`** (underscore per ADR-005), coexisting with GameMode's `pid_<pid>`; the S3 correlator merges by pid.
4. **Mid-session updates emit `PropertiesChanged` in place** via a now-mutable `ActivityInterface` (no remove+re-add flicker).

## Consequences

- Transitive deps compiled (never executed): `simple-websockets`, `tokio-tungstenite`, `interprocess`, `chrono`, `serde_with`, `aho-corasick`. We never start `RPCServer`, so its WebSocket/process-scan connectors never run. This is the accepted cost of reusing the crate; noted against the design doc's "small dependencies" guidance.
- Timestamps: after `fix()` they are milliseconds; mapping divides by 1000 for the D-Bus `Since`/`Until` seconds. rsRPC's `TimeoutValue` tuple field is private, so values are read back through its `Serialize` impl.
- The ignored client-provided `args.pid` is a deliberate hardening: `SO_PEERCRED` cannot be spoofed by the connecting process.

## Alternatives Considered

- **Run `RPCServer` as-is**: impossible without forking - no public event hook, and it starts unwanted connectors.
- **Vendoring `cmd.rs`**: a verbatim copy would fork the code and lose upstream fixes; rejected after owner direction ("reuse what exists"). A dependency is the reuse.
- **Hand-written payload structs** (the initial S2 draft): rejected - duplicates rsRPC's serde edge-case handling for no benefit.

## Related

- [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]]
- [[wiki/projects/gamebus-presenced]]
- [[wiki/logs/2026-08-04 - gamebus-presenced S2]]
