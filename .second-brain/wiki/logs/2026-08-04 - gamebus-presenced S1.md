---
date: 2026-08-04
type: devlog
tags: [devlog, gamebus-presenced, s1, gamemode, d-bus]
ai-first: true
related-projects: [gamebus-presenced]
---

## For future Claude

This dev log documents the S1 implementation for `gamebus-presenced` - the GameMode source feeding the S0 D-Bus surface. S1 watches `com.feralinteractive.GameMode` (`GameRegistered`/`GameUnregistered` signals + `ListGames` seed), publishes per-activity objects at `/org/gamebus/Presence/v1/Activity/pid_<pid>`, and emits `ActivityAdded`/`ActivityRemoved` signals. Verified end-to-end on the real session bus with `gamemoderun sleep 30`.

## Overview

S1 makes the daemon independently useful: pid + executable presence with no Discord code, which is everything `epaper-hubd` needs. The D-Bus surface itself was extracted as S0 (see [[wiki/logs/2026-08-04 - gamebus-presenced S0]]).

## Implementation Details

### Architecture

```
GameMode watcher task --mpsc--> daemon core (Manager + ObjectServer)
                                      |- register/remove Activity objects
                                      |- emit ActivityAdded/Removed + HasActivity changes
```

### New/changed modules

- `src/sources/mod.rs` - `GameModeEvent` enum: `Registered{pid, executable, since}`, `Unregistered{pid}`, `SourceLost`
- `src/sources/gamemode.rs` - zbus proxies for `com.feralinteractive.GameMode` and per-game `...Game` objects; watcher task; `NameOwnerChanged`-driven availability tracking
- `src/dbus/types.rs` - `Activity::from_gamemode(pid, executable, since)` pure constructor; `path_for_id` helper
- `src/dbus/activity.rs` - `ActivityInterface`: all read-only properties from the design (Sources, Kind, Name, Details, State, ProcessId, Executable, AppIds, Since, Until, images, Party*, Extra as a{sv})
- `src/dbus/manager.rs` - `ActivityAdded`/`ActivityRemoved` signals via `SignalContext`, `HasActivity` change emission on empty flips, `ListActivities` now returns `ao` (was `as` in S0), idempotent add, `remove_by_source`
- `src/main.rs` - daemon core event loop consuming watcher events

### Key decisions (see [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]])

- Activity IDs are `pid_<pid>`: hyphens are illegal in D-Bus object path elements (runtime error from zbus).
- GameMode signals carry the per-game object path, NOT the executable. Executable + registration timestamp are read from the `com.feralinteractive.GameMode.Game` object's `Executable`/`Timestamp` properties; `/proc/<pid>/exe` is the fallback. (Design doc corrected.)
- `Since` uses gamemoded's registration `Timestamp`.
- On `NameOwnerChanged` loss of gamemoded, all gamemode-sourced records die ("the record dies with the last source"); watcher re-seeds on its return.

### Bug found while here

S0's `connection.rs` test used `ROOT_PATH` without importing it - fixed (import scoped to the test module).

## Verification

- 13 unit tests pass (`from_gamemode` mapping, exe-stem name fallback incl. trailing-slash normalisation, idempotent add/remove, `remove_by_source`)
- Integration test (`tests/gamemode_integration.rs`): spawns the real daemon, registers a real game with `gamemoderun sleep 30`, asserts `ActivityAdded`, `ListActivities`, `HasActivity`, and property values (`ProcessId` == child pid, `Name` == "sleep", `Executable` == "/usr/bin/sleep", `Sources` == ["gamemode"], `Kind` == "game", `Since` > 0), then asserts `ActivityRemoved` after explicit `UnregisterGameByPID`. Runs in ~0.2s; skips gracefully without a session bus or gamemoded.
- Manual acceptance per the design doc: `busctl --user monitor org.gamebus.Presence.v1` shows both signals; `HasActivity` flips true/false correctly across a game's lifetime.
- Noted live: SIGKILLed games are reaped by gamemoded on its internal interval (observed 4-18s); explicit unregister is immediate. Also observed: this machine preloads `libgamemodeauto` broadly - short-lived tools (bash, chmod) register/unregister constantly, and the fallback path handles their already-dead executables correctly.

## Next Steps

S2 - Discord IPC listener with no upstream: bind `discord-ipc-0`, answer the handshake, decode frames, turn `SET_ACTIVITY` into activity objects, `SO_PEERCRED` pid. Independent of S1's code paths.

## Sources

- Project note: [[wiki/projects/gamebus-presenced]]
- Design doc: `docs/design/gamebus-presence.md` (local)
- PLAN.md: `PLAN.md` (local)
- GameMode D-Bus API verified live via `busctl --user introspect com.feralinteractive.GameMode` (2026-08-04)
