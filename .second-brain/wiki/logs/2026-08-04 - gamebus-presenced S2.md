---
date: 2026-08-04
type: devlog
tags: [devlog, gamebus-presenced, s2, discord, ipc]
ai-first: true
related-projects: [gamebus-presenced]
---

## For future Claude

This dev log documents the S2 implementation for `gamebus-presenced` - the standalone Discord IPC listener. S2 binds `$XDG_RUNTIME_DIR/discord-ipc-0`, answers the handshake, decodes `SET_ACTIVITY` frames using the pinned `rsrpc` crate's payload model, and publishes activity objects at `/org/gamebus/Presence/v1/Activity/discord_<pid>` with the pid taken from `SO_PEERCRED`. Verified end-to-end with a genuine `discord-rich-presence` client.

## Overview

S2 gives full rich presence on a machine with no Discord client installed (the dev machine's state). Games connect to our socket; we see title, chapter text, timestamps, artwork and party data. The upstream-proxy half is S3.

## Implementation Details

### Architecture

```
Discord IPC listener (tokio UnixListener on discord-ipc-0)
  |- accept -> SO_PEERCRED -> pid (kernel-verified, args.pid ignored)
  |- handshake -> READY
  |- frame loop: SET_ACTIVITY -> rsrpc::cmd::ActivityCmd + fix() -> map -> SourceEvent
  |- Close/EOF -> SourceEvent::Removed, Ping -> Pong, echo every frame
```

### New/changed modules

- `src/sources/mod.rs` - unified `SourceEvent` (`Updated(Box<Activity>)` / `Removed { id, source }` / `SourceLost { source }`); the core is now source-agnostic
- `src/sources/discord/protocol.rs` - frame codec (`u32 LE opcode | u32 LE len | JSON`), `PacketType`, `Handshake`, READY `CONNECTION_RESPONSE`
- `src/sources/discord/mod.rs` - listener: stale-socket unlink, live-owner degrade, per-connection tasks, `SO_PEERCRED` via `libc::getsockopt`
- `src/dbus/types.rs` - `Activity::from_discord(pid, client_id, &rsrpc::cmd::Activity)`; timestamps arrive normalised to ms by rsRPC's `fix()` and are stored as seconds
- `src/dbus/activity.rs` - `ActivityInterface::update()` - mutable in place, emits `PropertiesChanged` for all properties (SET_ACTIVITY is infrequent)
- `src/sources/gamemode.rs`, `src/main.rs` - refactored onto the unified `SourceEvent` channel

### Key decisions (see [[wiki/decisions/adr-006-rsrpc-crate-dependency]])

- Depend on `rsrpc` (git, rev `062e0fd9fe34a475640a822facfee68296ba22e3`) for `rsrpc::cmd` - the crate's public, battle-tested payload model. Its own listener is private/threaded/no-`SO_PEERCRED`, so transport is ours.
- Discord IDs are `discord_<pid>`; GameMode keeps `pid_<pid>`; S3's correlator merges by pid.
- Transitive (never-executed) deps accepted: simple-websockets, tokio-tungstenite, interprocess, chrono, serde_with, aho-corasick.

## Verification

- 18 unit tests pass (frame round-trip, oversized-payload guard, handshake parse, `from_discord` mapping incl. empty-field rules, rsRPC `fix()` timestamp path)
- Integration test (`tests/discord_integration.rs`, ~0.2s): spawns the daemon, connects a real `discord-rich-presence` client (blocking, in `spawn_blocking`; `SO_PEERCRED` yields the test pid), asserts `ListActivities`, all mapped properties (`Name`, `Details`, `State`, `Sources`, `Kind`, `ProcessId`, `AppIds["discord"]`, `Since` in seconds, assets, party), a mid-session update visible in place, and removal after `clear_activity()`. Skips without a session bus.
- Note: `HasActivity == false` is NOT asserted on this machine - `libgamemodeauto` is preloaded globally, so unrelated processes legitimately hold GameMode-sourced activities during the test.

## Next Steps

S3 - transparent proxy to a running Discord (games keep working when a real client takes `ipc-1`), plus the correlator: pid join across GameMode/Discord/Steam, merge and split rules, and the pid + `/proc/<pid>/stat` start-time runtime cache.

## Sources

- Project note: [[wiki/projects/gamebus-presenced]]
- Design doc: `docs/design/gamebus-presence.md` (local)
- rsRPC crate: https://github.com/SpikeHD/rsRPC (MIT, rev `062e0fd`)
- Client used for verification: `discord-rich-presence` crate v1.1.0
