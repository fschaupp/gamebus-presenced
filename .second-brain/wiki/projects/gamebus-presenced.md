---
date: 2026-08-04
type: project
status: in-progress
updated: 2026-08-04
tags: [project, rust, d-bus, discord, gamemode]
related-people: []
related-projects: []
job: Personal
repo: /media/Data/Projekte/gamebus-presenced
graduated-from: ""
ai-first: true
---

## For future Claude

`gamebus-presenced` is a Rust project implementing a unified "what is this machine playing" presence on the Linux session bus. It collects fragments from multiple sources (GameMode for pid/executable, Discord IPC for title/chapter text, Steam for appid) and correlates them by pid into a single activity record published via D-Bus interface `org.gamebus.Presence.v1`. S0 (D-Bus surface), S1 (GameMode source) and S2 (Discord IPC listener) are now COMPLETE; S3 (proxy + correlator) is next. This note tracks the project's status, decisions, and recent activity.

## Overview

### The Problem
Every piece of the answer exists on a Linux desktop, but no two pieces are in the same place:
- **GameMode** (`com.feralinteractive.GameMode`) knows pid + executable, not the game's name
- **Steam** knows appid, nothing else
- **Discord Rich Presence** (`$XDG_RUNTIME_DIR/discord-ipc-0`) knows title, chapter text, elapsed time, artwork - but it's a write-only proprietary socket
- **Lutris/Heroic** publish nothing; they just forward to Discord

Today, a status bar, stream overlay, or desk pet needs four integrations to learn what a single game launch already told the system four times over.

### The Solution
One session-bus service that:
1. Collects every source independently
2. Correlates them by pid into a single activity record
3. Publishes on `org.gamebus.Presence.v1` the way MPRIS publishes media

### Architecture
```
Discord IPC listener → transparent proxy to real Discord (or standalone)
GameMode watcher → pid/executable signals
Steam probe → registry.vdf + /proc environ
         ↓ all feed into correlator
    D-Bus surface: org.gamebus.Presence.v1.Manager
```

## Status

**S0, S1 and S2 implementation completed. S3 (proxy + correlator) next.**

- Repo created: 2026-08-03
- Design doc: `docs/design/gamebus-presence.md` (complete)
- Roadmap: `PLAN.md` (S0-S2 done, S3-S4 planned)
- S0: D-Bus surface foundation implemented and verified on session bus (2026-08-04)
- S1: GameMode source implemented and verified end-to-end on session bus (2026-08-04)
- S2: Discord IPC listener implemented and verified with a genuine discord-rich-presence client (2026-08-04)

## Slices (from PLAN.md)

### S0 — D-Bus surface foundation (COMPLETE)
Extracted from the original S1. D-Bus interface foundation: `org.gamebus.Presence.v1.Manager` with `ListActivities`, `HasActivity`, `Version` properties; `Activity` type; zbus v4 bindings; service verified on session bus.

### S1 — GameMode source (COMPLETE)
The `org.gamebus.Presence.v1` surface fed by the GameMode watcher: `GameRegistered`/`GameUnregistered` on `com.feralinteractive.GameMode`, seeded by `ListGames`. Per-game executable + registration timestamp resolved from `com.feralinteractive.GameMode.Game` object properties (`/proc/<pid>/exe` fallback). Per-activity objects at `.../Activity/pid_<pid>` with `ActivityAdded`/`ActivityRemoved` signals and `HasActivity` change emission; gamemoded availability tracked via `NameOwnerChanged`. Verified by integration test (`gamemoderun sleep 30`) and busctl acceptance.

### S2 — Discord IPC listener, no upstream (COMPLETE)
Standalone Discord Rich Presence listener on `discord-ipc-0` (stale-socket unlink, live-owner degrade). Handshake/READY, lock-step echo, `SET_ACTIVITY` parsed with the pinned `rsrpc` crate's `rsrpc::cmd` model (+ its `fix()` normalisation) and published as activity objects at `.../Activity/discord_<pid>`; pid from `SO_PEERCRED`, never client-supplied. `ActivityInterface` is now mutable (`PropertiesChanged` in place for mid-session updates); sources unified under one `SourceEvent` channel. Verified with a genuine discord-rich-presence client.

### S3 — Proxy + correlator (PLANNED)
Forward frames verbatim to a real Discord (which will have taken `ipc-1`) so this works alongside a running Discord. Plus the correlator: pid join across sources, merge and split rules, and the pid + `/proc/<pid>/stat` start-time runtime cache that re-adopts records across a daemon restart.

### S4 — Enrichment and packaging (PLANNED)
Steam appid from `/proc/<pid>/environ`, `detectable.json` naming (Discord's `applications/detectable`, ~1834 entries), a `gamebus-presence monitor` CLI, systemd user unit and D-Bus activation file.

## Key Decisions

### D-Bus Interface Design
- Bus name: `org.gamebus.Presence.v1` (the `.v1` spelling is deliberate for versioning)
- Root path: `/org/gamebus/Presence/v1`
- Manager interface: `org.gamebus.Presence.v1.Manager`
- Activity interface: `org.gamebus.Presence.v1.Activity`

See `docs/design/gamebus-presence.md` § The D-Bus surface for full details.

### Versioning Strategy
- Version is a separate element: `…Presence.v1`, not `…Presence1`
- Additive changes bump the `Version` property on Manager
- Incompatible changes earn a `.v2` name that runs beside `.v1`

### Correlator Design
- Sources joined by pid (Discord IPC gives pid via `SO_PEERCRED`, GameMode hands over `(pid, executable)`)
- With a pid, `/proc/<pid>/environ` yields `SteamAppId`, `/proc/<pid>/exe` the real binary
- One activity record can carry Discord's title/chapter, GameMode's executable, and Steam's appid
- Sources that cannot be tied to a pid publish lower-confidence records

### D-Bus Implementation Decisions
- **Library**: Use zbus v4 for D-Bus bindings with tokio runtime
- **Activity Type**: Use simplified Activity type without lifetime parameters to avoid zvariant::Value Clone issue
- **Manager Interface**: Includes `ListActivities` method, `HasActivity` and `Version` properties
- **Activity Interface**: Per-activity objects with properties exposed via D-Bus
- **Activity IDs**: `pid_<pid>` with underscore - hyphens are illegal in D-Bus object path elements (ADR-005)
- **GameMode details**: signals carry per-game object paths; executable/timestamp come from `com.feralinteractive.GameMode.Game` properties (ADR-005)

### Discord Proxy Strategy
- Bind `discord-ipc-0` at session start
- Discord starts later, finds 0 taken and binds ipc-1
- With Discord present: forward every frame verbatim upstream
- Without Discord: answer handshake and echo commands ourselves (arRPC behaviour)
- Unlink stale sockets on start; losing the race to Discord is a warning, never a crash

### Restart Survival
- Other sources (GameMode, Steam) re-derive from scratch on startup
- Runtime cache keyed by pid + process start-time from `/proc/<pid>/stat` (field 22)
- On startup, re-adopt records whose process is still alive AND start-time still matches

## Open Decisions

- **MPRIS as a source** — deliberately deferred. Already a good standard with its own consumers; wrapping it mostly duplicates. Move into S4 if needed.
- **arRPC-compatible bridge on 1337** — would let this replace arRPC for Vesktop users. Cheap once S2 exists; not currently in scope.
- **Licence** — MIT OR Apache-2.0 proposed in the design doc.

## Non-Goals

- Publishing presence *to* Discord (that direction is well served)
- Being a Discord client mod
- Shipping UI of any kind
- Windows support (unix-socket and D-Bus shaped throughout)

## Recent Activity

- 2026-08-04: S2 implementation completed - standalone Discord IPC listener. `discord-ipc-0` bound with stale-socket handling, handshake/READY, lock-step echo, `SET_ACTIVITY` via pinned `rsrpc` crate payload model, `SO_PEERCRED` pid, `discord_<pid>` objects, mutable `ActivityInterface` with `PropertiesChanged`. See [[wiki/logs/2026-08-04 - gamebus-presenced S2]] and [[wiki/decisions/adr-006-rsrpc-crate-dependency]].
- 2026-08-04: S1 implementation completed - GameMode source feeding the D-Bus surface. Watcher with `NameOwnerChanged` availability tracking, per-activity objects at `.../Activity/pid_<pid>`, `ActivityAdded`/`ActivityRemoved` signals, `HasActivity` change emission, `ListActivities` as `ao`. Two live discoveries recorded in [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]]. Verified by integration test + busctl acceptance. See [[wiki/logs/2026-08-04 - gamebus-presenced S1]] for details.
- 2026-08-04: S0 implementation completed - D-Bus interface foundation (`org.gamebus.Presence.v1.Manager` with `ListActivities`, `HasActivity`, `Version` properties; `Activity` type; zbus v4 bindings; service verified on session bus). See [[wiki/logs/2026-08-04 - gamebus-presenced S0]] for details.
- Dev logs: [[wiki/logs/2026-08-04 - gamebus-presenced S0]], [[wiki/logs/2026-08-04 - gamebus-presenced S1]], [[wiki/logs/2026-08-04 - gamebus-presenced S2]]
- Kanban board: [[boards/gamebus-presenced]]
- ADRs: [[wiki/decisions/adr-001-zbus-v4-tokio-runtime]], [[wiki/decisions/adr-002-simplified-activity-type]], [[wiki/decisions/adr-003-d-bus-service-naming]], [[wiki/decisions/adr-004-manager-and-activity-interfaces]], [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]], [[wiki/decisions/adr-006-rsrpc-crate-dependency]]

## Dependencies

- `zbus` - D-Bus bindings
- `tokio` - Async runtime
- `serde`/`serde_json` - Serialization
- `libc` - `SO_PEERCRED` via getsockopt
- `rsrpc` (MIT, pinned git rev `062e0fd`) - Discord IPC payload model (`rsrpc::cmd`), used since S2
- `discord-rich-presence` (dev) - genuine RPC client for integration tests

Note: `pog5/rsrpc` is GPLv3 — fine to learn from, not to vendor.

## Related

- [[GameMode]] - Feral Interactive's game mode daemon
- [[Discord IPC]] - Discord's local RPC protocol
- [[D-Bus]] - Freedesktop's message bus system
- [[MPRIS]] - Media Player Remote Interfacing Specification (model for presence interface)

## Sources

- Design doc: `docs/design/gamebus-presence.md` (local)
- PLAN.md: `PLAN.md` (local)
- Prior art: [rsRPC](https://github.com/SpikeHD/rsRPC), [arRPC](https://github.com/OpenAsar/arrpc), [GameMode](https://github.com/FeralInteractive/gamemode)
