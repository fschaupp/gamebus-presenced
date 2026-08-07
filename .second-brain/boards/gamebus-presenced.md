---
date: 2026-08-04
type: board
tags: [board, kanban, gamebus-presenced]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

Kanban board for the gamebus-presenced project. Tracks all tasks across slices S0-S9b (as of 2026-08-07). S0-S8 complete on master: S0 (D-Bus surface), S1 (GameMode source), S2 (Discord IPC listener), S3 (proxy + correlator + restart cache), S4a (Steam enrichment), S4b (naming), S4c (packaging), S4d (ancestor-walk), S4e (game identification + Steam scan + appid_records simplification), S4f (game groups), S5 (setup TUI, STAGED), S6/S6b (MPRIS + Lutris naming hints), S7 (publication hygiene), S8 (Heroic). S9 (umu-miss stash) + S9b (umu-database contribution pipeline) landed on master 2026-08-07 via --no-ff merge 58afefd (branch feat/s9-umu-miss-report kept, 9 commits 4c8a16e→b638404); master pushed to gitea.

## gamebus-presenced Kanban Board

### 📥 Backlog

	9 commits ready (4c8a16e→b638404), gates green. [[wiki/projects/gamebus-presenced]]

- [ ] 🟢 **Owed vault records: S5 dev log + setup-tool ADR, S4f dev log + ADR**
- [ ] 🟡 **Live-verify S9b against a real umu-miss launch**
	Stash is empty until the next Heroic/Lutris umu game starts.

### ✅ Done
- [x] ~~🟡 **Merge feat/s9-umu-miss-report to master (--no-ff, branch kept) + push to gitea**~~ ✅ 2026-08-07 (merge 58afefd, by owner)
- [x] ~~🟡 **S9b: verify/draft/export pipeline + 2 adversarial review rounds (10 findings fixed) + interactive TUI pane + endpoints.toml**~~ ✅ 2026-08-07 (branch feat/s9-umu-miss-report through b638404)
- [x] ~~🟡 **S9: umu-miss stash written by the daemon**~~ ✅ 2026-08-07 (4c8a16e at 00:09, on feat/s9-umu-miss-report)
- [x] ~~🟡 **S8: Heroic Epic/GOG detection via HEROIC_APP_NAME + legendary install records**~~ ✅ 2026-08-06 (186c427, 27473ac)
- [x] ~~🟡 **S7: publish hygiene — wrapper pattern blacklist, withhold nameless, teardown name monotonicity**~~ ✅ 2026-08-06 (489e7fd, c1904e5)
- [x] ~~🟡 **S6/S6b: MPRIS naming hints + Lutris wrapper titles + any-key scan adoption**~~ ✅ 2026-08-06 (commits 5a620e9, 70d84e4)
- [x] ~~🟡 **S4e: detectable.json path-prefixed entries (83% of DB) — basename-bucketed index with path-suffix matching + backslash normalisation**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Three-layer game identification (wrapper cmdline / descendant walk / umu tmpdir sandbox-family scan)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Unresolved-wrapper retry tick (15s) for delayed game launches**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Bounded /proc Steam-appid scan for games without GameMode**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: identify_process filter for utility processes (wineserver, tabtip.exe, etc.)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: appid_records: HashMap<String, u32> — one record per merge key, tree_depth decides**~~ ✅ 2026-08-04 *(tiebreaker superseded 2026-08-06 by S4f class election)*
- [x] ~~🟡 **S4f: game groups — class-elected sticky representative, deferred migration, ListGames reseed**~~ ✅ 2026-08-06 (repo: PLAN.md §S4f, commit 18cb92b)
- [x] ~~🟡 **S5: gamebus-setup TUI — status dashboard + user/system install, STAGED**~~ ✅ 2026-08-06 (repo: PLAN.md §S5, merge e56bbb8)
- [x] ~~🟡 **S4e: Merge key generalisation — steam:<appid> / lutris:<uuid> / umu:<id>**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: SteamAppId=0 rejected (Heroic/GOG Proton compat)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Tracing filter fix — RUST_LOG=debug was ignored by add_directive**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Cache excludes Steam-only records (scan re-finds on restart)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: is_wrapper_executable clears wrapper names (env, bash, etc.)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Discord integration tests skip when real Discord is running**~~ ✅ 2026-08-04
- [x] ~~🟢 **S4d: Bounded ancestor-walk join (ppid chain, descendant absorbs ancestor)**~~ ✅ 2026-08-04
- [x] ~~🟢 **S4c: gamebus-presence monitor CLI (pretty-prints bus state)**~~ ✅ 2026-08-04
- [x] ~~🟢 **S4c: gamebus-presence fetch-detectable CLI subcommand (refresh to $XDG_CACHE_HOME)**~~ ✅ 2026-08-04
- [x] ~~🟢 **S4c: systemd user unit + D-Bus activation file**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4b: build.rs fetch detectable.json from Discord applications/detectable endpoint**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4b: Naming lookup — precedence: Discord name > detectable.json > executable stem**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: Create `src/enricher.rs` — Enricher middleware between sources and correlator**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: Steam probe — `/proc/<pid>/environ` for SteamAppId/SteamGameId/UMU_ID/STORE**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: Correlator `steam` slot in Partials + merge rule**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: Steam partial removal tied to last non-Steam source**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: tracing::debug! on Discord-pid join-miss**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: Integration test — Steam appid enrichment via /proc environ**~~ ✅ 2026-08-04
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
- [x] ~~🟡 **S3a: upstream discovery at connect time (walk discord-ipc-1..=9)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3a: pump mode - whole-frame forwarding + SET_ACTIVITY tap**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3a: upstream loss closes client connection; standalone fallback**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3a: proxy fidelity test (fixture upstream, byte-identity)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3b: correlator.rs per-source partial store keyed by pid**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3b: merge rules both orders + field precedence; pid_<pid> absorbs**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3b: split/degrade - die with last source, no regression**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3b: same-pid join integration test (RPC client + RegisterGameByPID)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3c: write-through cache keyed pid+starttime; startup re-adoption**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3c: restart re-adoption test (SIGKILL + respawn)**~~ ✅ 2026-08-04
