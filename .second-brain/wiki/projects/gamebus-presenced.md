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

`gamebus-presenced` is a Rust project implementing a unified "what is this machine playing" presence on the Linux session bus. It collects fragments from multiple sources (GameMode for pid/executable, Discord IPC for title/chapter text, Steam for appid) and correlates them by pid into a single activity record published via D-Bus interface `org.gamebus.Presence.v1`. S0 (D-Bus surface), S1 (GameMode source), S2 (Discord IPC listener), S3 (proxy + correlator + restart cache), S4a (Steam enrichment via Enricher middleware), S4b (naming via detectable.json), S4c (packaging: CLI + systemd), and S4d (ancestor-walk join for wrapper-tree dedup) are now COMPLETE. This note tracks the project's status, decisions, and recent activity.

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

**S0, S1, S2, S3, S4a, S4b, S4c, and S4d implementation completed.**

- Repo created: 2026-08-03
- Design doc: `docs/design/gamebus-presence.md` (complete)
- Roadmap: `PLAN.md` (S0-S4 done)
- S0: D-Bus surface foundation implemented and verified on session bus (2026-08-04)
- S1: GameMode source implemented and verified end-to-end on session bus (2026-08-04)
- S2: Discord IPC listener implemented and verified with a genuine discord-rich-presence client (2026-08-04)
- S3: proxy + correlator + restart cache implemented and verified (byte-identity vs fixture, same-pid join, SIGKILL/respawn re-adoption) (2026-08-04)
- S4a: Steam enrichment via Enricher middleware (reactive /proc/<pid>/environ probe, tied to last non-Steam source) (2026-08-04)
- S4b: naming enrichment via detectable.json (enrichment-only fallback, never overrides more authoritative source) (2026-08-04)
- S4c: packaging (gamebus-presence CLI with monitor + fetch-detectable, systemd user unit, D-Bus activation) (2026-08-04)
- S4d: ancestor-walk join for wrapper-tree dedup (ppid-chain walk, descendant absorbs ancestor) (2026-08-04)

## Slices (from PLAN.md)

### S0 — D-Bus surface foundation (COMPLETE)
Extracted from the original S1. D-Bus interface foundation: `org.gamebus.Presence.v1.Manager` with `ListActivities`, `HasActivity`, `Version` properties; `Activity` type; zbus v4 bindings; service verified on session bus.

### S1 — GameMode source (COMPLETE)
The `org.gamebus.Presence.v1` surface fed by the GameMode watcher: `GameRegistered`/`GameUnregistered` on `com.feralinteractive.GameMode`, seeded by `ListGames`. Per-game executable + registration timestamp resolved from `com.feralinteractive.GameMode.Game` object properties (`/proc/<pid>/exe` fallback). Per-activity objects at `.../Activity/pid_<pid>` with `ActivityAdded`/`ActivityRemoved` signals and `HasActivity` change emission; gamemoded availability tracked via `NameOwnerChanged`. Verified by integration test (`gamemoderun sleep 30`) and busctl acceptance.

### S2 — Discord IPC listener, no upstream (COMPLETE)
Standalone Discord Rich Presence listener on `discord-ipc-0` (stale-socket unlink, live-owner degrade). Handshake/READY, lock-step echo, `SET_ACTIVITY` parsed with the pinned `rsrpc` crate's `rsrpc::cmd` model (+ its `fix()` normalisation) and published as activity objects at `.../Activity/discord_<pid>`; pid from `SO_PEERCRED`, never client-supplied. `ActivityInterface` is now mutable (`PropertiesChanged` in place for mid-session updates); sources unified under one `SourceEvent` channel. Verified with a genuine discord-rich-presence client.

### S3 — Proxy + correlator (COMPLETE)
Transparent proxy to a real Discord on `ipc-1..9`: frames forwarded whole (byte-identity verified against a fixture), `SET_ACTIVITY` tapped passively, upstream loss closes the client connection (reconnect lands in standalone mode). Correlator in `src/correlator.rs`: per-source partials keyed by exact pid, published activity is a derived view; `pid_<pid>` absorbs `discord_<pid>` on a join; Discord's human-facing fields win, GameMode owns executable; degrade-in-place, die with last source. Restart cache at `$XDG_RUNTIME_DIR/gamebus-presenced/cache.json` keyed pid + `/proc/<pid>/stat` start-time; startup re-adoption before sources spawn. umu/Proton wrapper-tree join is a documented miss (S4 candidate: bounded ancestor-walk). See [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]].

### S4 — Enrichment and packaging (COMPLETE 2026-08-04)

Brainstormed via 6-question Socratic interview — see [[wiki/concepts/2026-08-04 - Brainstorm - S4 Enrichment and Packaging]].

**S4a — Steam source (reactive via Enricher middleware)** ✅
New `src/enricher.rs`: `Enricher` struct sits between sources and correlator (`sources → Enricher → Vec<SourceEvent> → Correlator`). On `SourceEvent::Updated` with a pid, probes `/proc/<pid>/environ` for `SteamAppId`/`SteamGameId`/`UMU_ID`/`STORE`; if found, emits `SourceEvent::Updated(Source::Steam)`. Steam partial removal tied to last non-Steam source (pid-reuse safe — Enricher tracks `{pid: Set<Source>}`). `Activity::from_steam(pid, appid)` maps to `app_ids["steam"]`. Correlator gains `steam` slot. `tracing::debug!` on join-miss for ancestor-walk decision. Integration test: `sleep` spawned with `SteamAppId=480`, registered via `RegisterGameByPID`, merged record carries `app_ids["steam"]`.

**S4b — Naming enrichment (enrichment-only fallback)** ✅
`build.rs` fetches `detectable.json` from Discord's `applications/detectable` endpoint (23858 entries, 12.3MB); ships as installation data file (`$PREFIX/share/gamebus-presenced/detectable.json`, not binary-embedded). `gamebus-presence fetch-detectable` CLI refreshes to `$XDG_CACHE_HOME`. Naming precedence: Discord name > detectable.json lookup > executable stem. Anti-goal: never overrides a more authoritative source. Naming DB loaded after sources spawn to avoid blocking the Discord listener.

**S4c — Packaging** ✅
`gamebus-presence` CLI binary (`src/bin/gamebus-presence.rs`): `monitor` pretty-prints bus state (activities, sources, names, appids); `fetch-detectable` downloads Discord's detectable.json to `$XDG_CACHE_HOME`. systemd user unit (`data/gamebus-presenced.service`) and D-Bus activation file (`data/org.gamebus.Presence.v1.service`) for session-start activation.

**S4d — Ancestor-walk join (wrapper-tree dedup)** ✅
Bounded ppid-chain walk (`MAX_ANCESTOR_DEPTH` = 10) for the umu/Proton wrapper-tree case. The Enricher tracks `{pid: steam_appid}` and, on a new Steam probe, checks if another tracked pid shares the same appid AND is in the same process tree. The descendant absorbs the ancestor. Three bugs found during live testing with Brotato: merge direction reversed, cache-adopted records couldn't be removed, `SteamAppId=default` false positive. See [[wiki/logs/2026-08-04 - gamebus-presenced S4]] for details.

**S4e — Game identification + Steam process scan** ✅
Live testing with Brotato, CoD: Black Ops Cold War, Amnesia: The Bunker, and Resident Evil 2 surfaced eleven gaps, all fixed. (1) detectable.json path-prefixed entries (83% of the DB) missed — basename-bucketed index with path-suffix matching + backslash normalisation. (2) Wrapper processes unidentified — three-layer identification: wrapper cmdline, connected descendant walk (exe + Wine cmdline), sandbox-family scan via the umu `var/tmp-XXXXXX` cmdline token (Flatpak-portal severs the tree). (3) Delayed game launches — `unresolved_wrappers` retried every 15s via a main-loop tick. (4) Games without GameMode invisible — bounded `/proc/*/environ` Steam-appid scan in the same tick, closing the S4a reactive-only gap proven real by Resident Evil 2 launched without gamemoderun. (5) Steam scan exploded into ~20 records — `identify_process` filter for utility processes, then **replaced pairwise ancestor-walk merge with `appid_records: HashMap<String, u32>`** (merge key → deepest pid, one record per key, `tree_depth` decides). (6) Merge key generalised beyond Steam — `steam:<appid>` / `lutris:<uuid>` / `umu:<id>`. (7) `SteamAppId=0` rejected. (8) Tracing filter overrode RUST_LOG. (9) Cache re-adopted stale Steam-only records. (10) Wrapper executables shown as game names. (11) Discord integration tests fail when real Discord is running. Final verification: 1 record for Amnesia: The Bunker, 1 record for Resident Evil 2. See [[wiki/logs/2026-08-04 - gamebus-presenced S4]].

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

- 2026-08-04: S4d implementation completed - ancestor-walk join for wrapper-tree dedup. Enricher tracks `{pid: steam_appid}`, `is_ancestor()` ppid-chain walk (bounded to 10 hops), descendant absorbs ancestor. Three bugs found during live Brotato testing: merge direction reversed, cache-adopted records couldn't be removed (correlator `drop_partial` fix), `SteamAppId=default` false positive (numeric-only appids). Also fixed: naming precedence (Steam's detectable.json-enriched name beats GameMode's executable stem). See [[wiki/logs/2026-08-04 - gamebus-presenced S4]].
- 2026-08-04: Dead-code cleanup (owner-confirmed): `Manager::remove_by_source` (superseded by the correlator) and the unused S0 error variants `NameAcquisition`/`Config`/`Internal` removed; `cargo clippy --all-targets` is now fully clean, zero warnings. The test-removal exchange produced the workflow preference [[wiki/concepts/test-deletion-visibility]].
- 2026-08-04: S3 implementation completed - proxy (byte-identical forwarding, tap, upstream-loss close), correlator (`pid_<pid>` absorbs `discord_<pid>`, degrade-in-place, die-with-last-source, exact-pid join), restart cache (pid + start-time, re-adopted before sources spawn). 31 unit + 5 integration tests green. See [[wiki/logs/2026-08-04 - gamebus-presenced S3]] and [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]].
- 2026-08-04: S3 planned with owner - landing order proxy -> correlator -> cache; absorption and upstream-loss-close decisions confirmed; umu research: launcher assigns identity via env vars (no runtime detection to reuse), `UMU_ID` enrichment deferred to S4, wrapper-tree join deferred to S4.
- 2026-08-04: S2 implementation completed - standalone Discord IPC listener. `discord-ipc-0` bound with stale-socket handling, handshake/READY, lock-step echo, `SET_ACTIVITY` via pinned `rsrpc` crate payload model, `SO_PEERCRED` pid, `discord_<pid>` objects, mutable `ActivityInterface` with `PropertiesChanged`. See [[wiki/logs/2026-08-04 - gamebus-presenced S2]] and [[wiki/decisions/adr-006-rsrpc-crate-dependency]].
- 2026-08-04: S1 implementation completed - GameMode source feeding the D-Bus surface. Watcher with `NameOwnerChanged` availability tracking, per-activity objects at `.../Activity/pid_<pid>`, `ActivityAdded`/`ActivityRemoved` signals, `HasActivity` change emission, `ListActivities` as `ao`. Two live discoveries recorded in [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]]. Verified by integration test + busctl acceptance. See [[wiki/logs/2026-08-04 - gamebus-presenced S1]] for details.
- 2026-08-04: S0 implementation completed - D-Bus interface foundation (`org.gamebus.Presence.v1.Manager` with `ListActivities`, `HasActivity`, `Version` properties; `Activity` type; zbus v4 bindings; service verified on session bus). See [[wiki/logs/2026-08-04 - gamebus-presenced S0]] for details.
- Dev logs: [[wiki/logs/2026-08-04 - gamebus-presenced S0]], [[wiki/logs/2026-08-04 - gamebus-presenced S1]], [[wiki/logs/2026-08-04 - gamebus-presenced S2]], [[wiki/logs/2026-08-04 - gamebus-presenced S3]]
- Kanban board: [[boards/gamebus-presenced]]
- ADRs: [[wiki/decisions/adr-001-zbus-v4-tokio-runtime]], [[wiki/decisions/adr-002-simplified-activity-type]], [[wiki/decisions/adr-003-d-bus-service-naming]], [[wiki/decisions/adr-004-manager-and-activity-interfaces]], [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]], [[wiki/decisions/adr-006-rsrpc-crate-dependency]], [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]]

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
