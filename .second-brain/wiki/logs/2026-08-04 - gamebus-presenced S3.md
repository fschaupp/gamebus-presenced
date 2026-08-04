---
date: 2026-08-04
type: devlog
tags: [devlog, gamebus-presenced, s3, proxy, correlator, cache]
ai-first: true
related-projects: [gamebus-presenced]
---

## For future Claude

This dev log documents the S3 implementation for `gamebus-presenced` - transparent proxy to a real Discord, the pid-join correlator, and the restart-survival cache. S3 makes the daemon work alongside a running Discord client (frames forwarded verbatim, traffic tapped), merges GameMode and Discord fragments into one record per pid (`pid_<pid>` absorbing `discord_<pid>`), and survives a daemon restart via a start-time-validated cache in `$XDG_RUNTIME_DIR`. Landing order was proxy -> correlator -> cache; all decisions in [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]].

## Overview

S3 completes the design doc's core promise: "the correlator is the whole point". One activity record now carries Discord's title/chapter and GameMode's executable at once, joined by pid. The proxy means a real Discord client and gamebus-presenced coexist - games connect to us on `discord-ipc-0`, we pump frames whole to Discord on `ipc-1..9` and pipe responses back, observing everything.

## Implementation Details

### Proxy (`src/sources/discord/proxy.rs`)

- `find_upstream()`: walks `discord-ipc-1..=9` at each client connect; first connectable socket wins (stale candidates fail connect and are skipped).
- `pump()`: `tokio::select!` over both directions, forwarding whole frames. `Frame::encode()` of a parsed frame reproduces the exact wire bytes (header derives from opcode + payload length), so frame-level forwarding is byte-identical by construction - and the fidelity test asserts it.
- Tap: handshake frames yield `client_id`; `SET_ACTIVITY` goes through the same `handle_set_activity` path as standalone (rsRPC `fix()` + map). Tap is passive: parse problems are debug-logged, never propagated - observing must not break the pipe.
- Upstream loss mid-connection: the upstream read returns EOF, `pump` returns, both streams drop, client sees EOF. Reconnect lands in standalone mode (verified in the test).
- Standalone mode is untouched when no upstream exists.

### Correlator (`src/correlator.rs`, new)

- `Correlator { partials: HashMap<pid, Partials>, published: pid -> id, id_index: id -> pid }`. Pure state + functions returning `Effect::{PublishNew, UpdateInPlace, Remove}`; `main.rs` just executes effects. No D-Bus, no sockets - the merge/split rules are unit-tested in isolation, as the design doc demands.
- `id_index` keeps every id a pid ever published under: after an absorb, the absorbed source still says goodbye referencing its own scoped id (`discord_X`), and the lookup must resolve. Indexed at partial-arrival time, not publish time (found via a failing test: gamemode-first order never indexed `discord_X`).
- `merge()`: name/discord-fields from the Discord partial, executable from GameMode, `since` = Discord's first nonzero else GameMode's, app_ids/extra union, kind = game if any says game, sources in canonical [gamemode, discord] order.
- Absorb both directions: solo `discord_X` + gamemode arrival -> `[Remove(discord_X), PublishNew(pid_X)]`; solo `pid_X` + discord arrival -> `[UpdateInPlace(pid_X)]`. Split: merged loses discord -> UpdateInPlace; merged loses gamemode -> `[Remove(pid_X), PublishNew(discord_X)]`. Empty partials -> `[Remove(published)]`.
- pid-0 records pass through unjoined (no source emits them today; Steam's shape is an S4 question).
- `Manager.remove_by_source` deleted: source-loss degrades via the correlator now. Testing a method no caller reaches asserts code that never runs (flagged to owner).
- `Activity` gained `PartialEq, Eq, Serialize, Deserialize`.

### Restart cache (`src/cache.rs`, new)

- `$XDG_RUNTIME_DIR/gamebus-presenced/cache.json`, write-then-rename, written after every applied effect batch (`sync_cache` in main.rs).
- Key: pid + `/proc/<pid>/stat` field-22 start-time. Parsing skips the parenthesised comm (which can contain spaces/parens) via the LAST ')' on the line; starttime is token index 19 of the remainder.
- Startup: `cache::load_in` validates each record (alive + start-time match) and the correlator `adopt()`s it BEFORE sources spawn, so re-derived records merge into adopted ones instead of racing.
- Adopted records are not source partials: nothing kills them when the process later dies (lingering accepted, best-effort per design doc).

## Verification

- 31 unit tests pass (8 correlator, 6 cache, plus S0-S2's).
- `tests/discord_proxy.rs` (~0.1s): fixture upstream (recording unix-socket server) in an isolated XDG_RUNTIME_DIR; asserts client->upstream and upstream->client byte-identity, SET_ACTIVITY tapped onto the bus in proxy mode, upstream-loss EOF, and standalone fallback on reconnect.
- `tests/correlator_integration.rs` (~0.4s): genuine discord-rich-presence client (pid = test process) + `busctl RegisterGameByPID` for the same pid -> solo `discord_<pid>` absorbed into `pid_<pid>` with sources [gamemode, discord]; clear degrades in place (name regresses to exe stem); unregister removes the record. Pre-cleans the test pid's registration for determinism on libgamemodeauto-preload machines.
- `tests/restart_cache.rs` (~0.3s): SIGKILL daemon 1 with a live record, spawn daemon 2, record re-adopted with Discord fields intact and no client reconnect.
- `cargo clippy --all-targets` fully clean (zero warnings) after the post-S3 error.rs vocabulary prune (`NameAcquisition`/`Config`/`Internal` removed, owner-confirmed 2026-08-04).
- umu research (owner question): umu-launcher assigns identity at launch (`UMU_ID`, `STORE` env), it does not detect appids of running processes - nothing to reuse for the S3 join; `UMU_ID` enrichment deferred to S4 (numeric `umu-<N>` implies steam appid N). Both umu repos are GPL-3.0: query/interop only.

## Next Steps

S4 - enrichment and packaging: Steam appid + `UMU_ID`/`STORE` from `/proc/<pid>/environ`, `detectable.json` naming (umu-database as cached secondary), `gamebus-presence monitor` CLI, systemd user unit + D-Bus activation. Candidate if measured: bounded ancestor-walk join for the umu/Proton wrapper-tree case.

## Sources

- Project note: [[wiki/projects/gamebus-presenced]]
- Design doc: `docs/design/gamebus-presence.md` (local)
- Decisions: [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]]
- umu-launcher: https://github.com/Open-Wine-Components/umu-launcher (GPL-3.0, fetched 2026-08-04)
- umu-database: https://github.com/Open-Wine-Components/umu-database (GPL-3.0, fetched 2026-08-04)
