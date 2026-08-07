---
date: 2026-08-04
type: board
tags: [board, kanban, gamebus-presenced]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

Kanban board for the gamebus-presenced project. Tracks all tasks across slices S0-S9b (as of 2026-08-07). S0-S8 complete on master: S0 (D-Bus surface), S1 (GameMode source), S2 (Discord IPC listener), S3 (proxy + correlator + restart cache), S4a (Steam enrichment), S4b (naming), S4c (packaging), S4d (ancestor-walk), S4e (game identification + Steam scan + appid_records simplification), S4f (game groups), S5 (setup TUI, STAGED), S6/S6b (MPRIS + Lutris naming hints), S7 (publication hygiene), S8 (Heroic). S9 (umu-miss stash) + S9b (umu-database contribution pipeline) landed on master 2026-08-07 via --no-ff merge 58afefd (branch feat/s9-umu-miss-report kept, 9 commits 4c8a16e→b638404); master pushed to gitea. Public branch re-composed 2026-08-07 evening (17 cherry-picked commits → 13 clean publishable commits on branch `public`, backup ref `public-original`). **v0.1.0 RELEASED the same night** at https://github.com/fschaupp/gamebus-presenced via S10 (network-free builds + release workflow + `.scripts/release.sh`); the disclosed (`Assisted-by:` + README AI note) history still needs a one-time force-push + re-tag.

## gamebus-presenced Kanban Board

### 📥 Backlog

	9 commits ready (4c8a16e→b638404), gates green. [[wiki/projects/gamebus-presenced]]

- [ ] 🟢 **Owed vault records: S5 dev log + setup-tool ADR, S4f dev log + ADR**
- [ ] 🟡 **Live-verify S9b against a real umu-miss launch**
	Stash is empty until the next Heroic/Lutris umu game starts.

### ⏳ Waiting On

- [ ] 🟡 **Republish the disclosed history (owner's call)**
	Delete the GitHub release + remote tag v0.1.0, `git push --force github public`, re-push the tag; the workflow republishes from the Assisted-by history. Then delete the `public-original` backup ref, and optionally add the alias email spritzwine.absently488@passinbox.com to the GitHub account so commits attribute. [[wiki/logs/2026-08-07 - gamebus-presenced v0.1.0 Release]]
### ✅ Done
- [x] ~~🟡 **Merge `feat/release-pipeline` to master (--no-ff, branch kept)**~~ ✅ 2026-08-07 (merge d6adb69, parents f0454e6 + a9dfa0e; trailers switched to Assisted-by first; gate green on merged master; master now 3 commits ahead of gitea, push is the owner's call)
- [x] ~~🟡 **v0.1.0 released: S10 network-free builds + GitHub release workflow + .scripts/release.sh**~~ ✅ 2026-08-07 (night; pushed by owner, workflow published the release)
	build.rs gone, install fetches detectable.json best-effort via installed CLI; release.sh gates, sets version, commits, tags; exec-bit fixed via update-index (core.filemode=false here). Mastodon posts drafted. All 17 public commits then rewritten with Assisted-by trailers + README AI note. [[wiki/logs/2026-08-07 - gamebus-presenced v0.1.0 Release]]
- [x] ~~🟡 **Publish/push the `public` branch (owner's call)**~~ ✅ 2026-08-07 (pushed to GitHub, first release live)
- [x] ~~🟡 **Public branch re-composition: 17 cherry-picked commits → 13 clean publishable commits on `public`**~~ ✅ 2026-08-07 (worktree /media/Data/Projekte/gamebus-presenced.worktrees/public, backup ref `public-original`)
	All em-dashes and internal references (S-phase tags, spec §, ADR numbers, diary dates) removed from every commit's tree via git filter-branch + deterministic cleanup script; fresh tweet-size messages (max 200 chars, no Co-Authored-By); every commit passes cargo check --all-targets; full licensing set threaded through history (Apache-2.0 LICENSE + NOTICE + "Copyright 2026 Florian Schaupp" + Discord disclaimer + umu prior-art in README). [[wiki/projects/gamebus-presenced]] [[wiki/logs/2026-08-07 - gamebus-presenced Public Branch Re-Composition]]
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
