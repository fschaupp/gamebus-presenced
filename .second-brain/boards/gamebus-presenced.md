---
date: 2026-08-04
type: board
tags: [board, kanban, gamebus-presenced]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

Kanban board for the gamebus-presenced project. Tracks all tasks across slices S0-S4. S0 (D-Bus surface), S1 (GameMode source) and S2 (Discord IPC listener) are complete. Use this board to manage implementation progress.

## gamebus-presenced Kanban Board

### 📥 Backlog

### 📋 This Week

### 🔨 In Progress

### ⏳ Waiting On

### ✅ Done
- [x] ~~🟡 **Create Cargo.toml with dependencies (zbus, tokio, serde, thiserror, tracing)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/main.rs - daemon entry point**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/error.rs - custom error types**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/dbus/mod.rs - module exports**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/dbus/types.rs - core types (Activity, Source, Kind, AppIds)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/dbus/connection.rs - bus connection management**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/dbus/manager.rs - Manager interface implementation**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/dbus/activity.rs - Activity interface stub**~~ ✅ 2026-08-04
- [x] ~~🟡 **Verify compilation with cargo check**~~ ✅ 2026-08-04
- [x] ~~🟡 **Test D-Bus service with busctl**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/sources/gamemode.rs - GameMode proxy + NameOwnerChanged watcher**~~ ✅ 2026-08-04
- [x] ~~🟡 **Resolve per-game details from GameMode.Game object properties (Executable/Timestamp, /proc fallback)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Implement ActivityInterface with all read-only properties**~~ ✅ 2026-08-04
- [x] ~~🟡 **Add ActivityAdded/ActivityRemoved signals + HasActivity change emission**~~ ✅ 2026-08-04
- [x] ~~🟡 **Fix ListActivities to return ao (object paths)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Wire daemon core event loop (mpsc from watcher)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Unit tests: from_gamemode, name fallback, idempotent add/remove, remove_by_source**~~ ✅ 2026-08-04
- [x] ~~🟡 **Integration test: gamemoderun sleep 30 end-to-end**~~ ✅ 2026-08-04
- [x] ~~🟡 **busctl acceptance: monitor signals, HasActivity flip**~~ ✅ 2026-08-04
- [x] ~~🟡 **Unify sources under SourceEvent channel (source-agnostic core)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Add rsrpc crate dependency (pinned rev) for SET_ACTIVITY payload model**~~ ✅ 2026-08-04
- [x] ~~🟡 **Discord frame codec + handshake/READY + lock-step echo**~~ ✅ 2026-08-04
- [x] ~~🟡 **tokio UnixListener on discord-ipc-0 with stale-socket handling**~~ ✅ 2026-08-04
- [x] ~~🟡 **SO_PEERCRED pid via libc::getsockopt (client args.pid ignored)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Activity::from_discord mapping (name, details/state, timestamps, assets, party)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Mutable ActivityInterface with PropertiesChanged for mid-session updates**~~ ✅ 2026-08-04
- [x] ~~🟡 **Integration test: discord-rich-presence client end-to-end**~~ ✅ 2026-08-04
