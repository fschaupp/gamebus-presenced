---
date: 2026-08-04
type: devlog
tags: [devlog, gamebus-presenced, s4, steam, enrichment, naming, cli]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

Dev log for S4 of [[wiki/projects/gamebus-presenced]] — enrichment and packaging, completed 2026-08-04. Four sub-slices landed in one session: S4a (Steam enrichment via Enricher middleware), S4b (naming via detectable.json), S4c (packaging: CLI + systemd), S4d (ancestor-walk join for wrapper-tree duplicates). 58 unit tests + 6 integration tests pass; clippy fully clean. The Enricher middleware pattern (chosen over probe-in-main for separation of concerns) accommodates future launcher enrichment and tracing/profiling.

## What was done

### S4a — Steam source (reactive via Enricher middleware)

**New files:**
- `src/enricher.rs` — Enricher middleware between sources and correlator. `Enricher::process(SourceEvent) -> Vec<SourceEvent>`: on `Updated` with a pid, probes `/proc/<pid>/environ` for `SteamAppId`/`SteamGameId`/`UMU_ID`/`STORE`; if found, emits `SourceEvent::Updated(Source::Steam)`. Steam partial removal tied to last non-Steam source (Enricher tracks `{pid: Set<Source>}` — pid-reuse safe). `tracing::debug!` on Discord-pid join-miss for ancestor-walk instrumentation.
- `tests/steam_enrichment.rs` — integration test: `sleep` spawned with `SteamAppId=480`, registered via `RegisterGameByPID`, merged record carries `app_ids["steam"]` and `sources` includes "steam".

**Modified files:**
- `src/main.rs` — Enricher wired into the event loop: `sources → Enricher → Vec<SourceEvent> → Correlator`.
- `src/correlator.rs` — `Partials` gains `steam: Option<Activity>` slot; `merge()` accepts steam parameter; Steam owns `app_ids["steam"]`; name fallback chain includes Steam.
- `src/dbus/types.rs` — `Activity::from_steam(pid, appid)` maps to `id = steam_<pid>`, `sources = [Source::Steam]`, `app_ids["steam"] = appid`, `kind = Kind::Game`. `Source` gains `Hash` derive (needed for `HashSet<Source>` in the Enricher).

**Key decisions:**
- Reactive piggyback (no polling): the Enricher probes `/proc/<pid>/environ` only when another source reports a pid. No independent Steam watcher.
- Steam partial removal tied to last non-Steam source: prevents stale Steam data surviving a pid reuse.
- `UMU_ID=umu-<N>` takes precedence over `SteamAppId`/`SteamGameId` (umu-launcher convention; numeric N implies steam appid N).

### S4b — Naming enrichment (enrichment-only fallback)

**New files:**
- `build.rs` — fetches `detectable.json` from Discord's `applications/detectable` endpoint (23858 entries, 12.3MB) at build time, writes to `OUT_DIR/detectable.json`. Ships as an installation data file (not binary-embedded).
- `src/naming.rs` — `NamingDb` with two pre-built indexes: `by_executable` (filename lowercase → name) and `by_steam_appid` (appid → name). Runtime lookup order: `$XDG_CACHE_HOME` > `$XDG_DATA_HOME` > `/usr/share/` > `OUT_DIR` (dev). `NamingDb::load()` returns `None` if no database found — naming enrichment is silently disabled.

**Modified files:**
- `src/enricher.rs` — `apply_naming()` method: modifies the activity's name from detectable.json before forwarding to the correlator. Enrichment-only: never overrides a non-empty name from Discord. Tries appid lookup first (stronger signal), then executable. Also applies naming to the Steam partial (by appid).
- `src/correlator.rs` — `merge()` name fallback chain: Discord name > GameMode name (possibly enriched by detectable.json) > Steam name (possibly enriched by detectable.json).
- `src/main.rs` — `enricher.load_naming()` called after sources spawn so the 12MB JSON parse doesn't delay the Discord listener.
- `Cargo.toml` — `ureq` added to `[build-dependencies]` (for build.rs) and `[dependencies]` (for the CLI's fetch-detectable subcommand).

**Key decisions:**
- `detectable.json` ships as an installation data file, not embedded in the binary. The file can be updated without recompiling.
- Naming DB loaded after sources spawn — the 12MB JSON parse would otherwise delay the Discord listener by ~100ms, causing the correlator integration test to fail (RPC client connects before the socket is bound).
- Naming precedence: Discord name > detectable.json lookup > executable stem. Anti-goal: never overrides a more authoritative source.

### S4c — Packaging

**New files:**
- `src/bin/gamebus-presence.rs` — CLI binary with two subcommands:
  - `monitor` — connects to the session bus, pretty-prints all activities (name, kind, sources, pid, executable, details, state, appids)
  - `fetch-detectable` — downloads Discord's detectable.json to `$XDG_CACHE_HOME/gamebus-presenced/detectable.json`
- `data/gamebus-presenced.service` — systemd user unit (Type=dbus, BusName=org.gamebus.Presence.v1, Restart=on-failure)
- `data/org.gamebus.Presence.v1.service` — D-Bus activation file (SystemdService=gamebus-presenced.service)

**Modified files:**
- `Cargo.toml` — `[[bin]]` entries for both `gamebus-presenced` and `gamebus-presence`.

### S4d — Ancestor-walk join (wrapper-tree dedup)

**Modified files:**
- `src/enricher.rs` — added `steam_appids: HashMap<u32, String>` tracking, `read_ppid()` (parses `/proc/<pid>/stat` field 4), `is_ancestor()` (bounded ppid-chain walk, `MAX_ANCESTOR_DEPTH = 10`), `find_related_pid()` (same appid + same tree). Two merge cases: (1) new pid is descendant → remove ancestor's record, forward descendant; (2) new pid is ancestor → suppress entirely, remove any cache-adopted record for the ancestor.
- `src/correlator.rs` — `drop_partial` now handles cache-adopted records (published but no partials). Previously, `adopt()` inserted into `id_index` and `published` but created no `Partials` entry, so `on_removed` found the pid but `drop_partial` returned empty.
- `src/enricher.rs` — `find_steam_appid` now requires numeric values. Steam sets `SteamAppId=default` for its own client processes, which caused false-positive merges between unrelated Steam client processes.

**Bugs found during live testing with Brotato (native, via Steam Runtime):**
1. **Merge direction reversed** — `find_related_pid` returned the already-tracked pid regardless of ancestor/descendant direction. The merge code assumed it was the ancestor and removed it. With the GameMode seed arriving in arbitrary order, the descendant could be tracked first, causing the wrong record to be removed. Fixed by explicitly checking `is_ancestor(other_pid, pid)` vs `is_ancestor(pid, other_pid)` to determine direction.
2. **Cache-adopted records couldn't be removed** — `adopt()` creates no correlator partials, so `drop_partial` couldn't remove them. The Enricher's ancestor-walk emitted `Removed` for the ancestor, but the correlator couldn't execute it. Fixed by adding a fast path in `drop_partial` for published-but-no-partials records.
3. **`SteamAppId=default` false positive** — Steam client processes have `SteamAppId=default` in their environment. The Enricher treated "default" as a valid appid and merged unrelated Steam client processes. Fixed by requiring numeric-only appids.

**Process tree observed (Brotato, native via Steam Runtime):**
```
steam-runtime-supervisor(991977)
  └─ reaper(991978)          ← GameMode registered, SteamAppId=1942280
      └─ srt-bwrap(991987)   ← GameMode registered, SteamAppId=1942280 (survives)
          └─ pv-adverb(992227)
              └─ Brotato.x86_64(992251)  ← actual game, NOT GameMode registered
```

GameMode registers the wrapper processes (reaper, srt-bwrap) because `libgamemodeauto.so` is LD_PRELOADed into them. The actual game binary (Brotato.x86_64) is not GameMode-registered. The ancestor-walk merges the two GameMode-registered wrappers into one record (the deepest one, srt-bwrap).

## Test results

- 61 unit tests pass (7 enricher, 5 naming, 6 correlator steam/ancestor-walk, 1 types from_steam, plus all existing)
- 6 integration tests pass: gamemode, discord, discord_proxy, correlator, restart_cache, steam_enrichment
- `cargo clippy --all-targets` fully clean, zero warnings
- `cargo fmt` applied

## Final verification (2026-08-04)

| Game | Record | Sources | AppId |
|---|---|---|---|
| Amnesia: The Bunker | `pid_1215096` | gamemode | — |
| Resident Evil 2 | `steam_1116381` | steam | steam=883710 |

Amnesia's umu-run wrapper (1215062) suppressed by `lutris:113b1bfa-...` merge key (deeper pid kept). RE2 converged on steam.exe (depth=9). Exactly 2 records. 61 unit + 6 integration tests pass; clippy clean.

## Issues encountered

1. **Naming DB load blocks startup** — the 12MB JSON parse in `Enricher::new()` delayed the Discord listener by ~100ms, causing the correlator integration test to fail (RPC client connects before the socket is bound). Fix: `Enricher::new()` starts with `naming: None`; `load_naming()` is called after sources spawn.

2. **`Source` needs `Hash` derive** — the Enricher uses `HashSet<Source>` for tracking active sources per pid. Added `Hash` to the derive list.

3. **`unused_mut` warnings in tests** — two test functions had unnecessary `mut` bindings. Fixed.

4. **Test isolation: daemon already running** — the proxy test failed with `Connection(NameTaken)` because the user's daemon was still running. Not a code bug; test assumes no other daemon. Resolved by killing the daemon before running tests.

5. **Test isolation: cache from previous run** — the gamemode test failed because the daemon re-adopted cached records from the previous run (the user's game). Fixed: gamemode test now uses an isolated `XDG_RUNTIME_DIR`.

6. **`HasActivity == false` assertion too broad** — the gamemode test asserted no activities exist, but the machine has other GameMode registrations (libgamemodeauto preload, running game). Fixed: removed the overly broad assertion.

7. **Ancestor-walk merge direction reversed** — see S4d section above.

8. **Cache-adopted records couldn't be removed** — see S4d section above.

9. **`SteamAppId=default` false positive** — see S4d section above.

10. **`ppid > 1` excluded init** — `is_ancestor` used `ppid > 1` as the chain guard, which excluded pid 1 (init/systemd) from the walk. Fixed to `ppid > 0`.

11. **Wrapper executable names shown as game names** — `libgamemodeauto` preload registers every process with GameMode, including shells and wrappers (`env`, `bash`, `python3`, etc.). The executable stem became the record's name, which is never useful. Fixed: `is_wrapper_executable()` clears the name for known wrappers (env, bash, sh, zsh, fish, python, perl, ruby, node, steam-runtime-*, reaper, srt-bwrap, pv-adverb, bwrap, umu-*, gamemoderun, lutris-wrapper). The name shows as "(unknown)" unless detectable.json or Discord provides a real name.

12. **Discord integration tests fail when real Discord is running** — the proxy forwards the test's invalid client_id to the real Discord upstream, which rejects it. Fixed: `correlator_integration` and `discord_integration` tests now skip when a connectable `discord-ipc-1..=9` socket exists.

### S4e — Game identification for wrapper processes + Steam process scan

Live testing with Brotato (native Steam), Call of Duty: Black Ops Cold War (Lutris-Flatpak + umu + Battle.net), Amnesia: The Bunker (Lutris-Flatpak + umu), and Resident Evil 2 (native Steam) surfaced four gaps the S4a-S4d design didn't cover. All fixed in one session.

**Gap 1: detectable.json path-prefixed entries missed.** 83% of executable entries in Discord's detectable.json are path-prefixed (`amnesia the bunker/amnesiathebunker.exe`), but the S4b index only matched bare filenames. Fixed: `NamingDb` now buckets entries by basename with a three-tier lookup — (1) path-suffix match (mirrors Discord's own scanner, disambiguates basename collisions like `amnesia/amnesia.exe` vs `amnesia the dark descent/amnesia.exe`), (2) plain entry, (3) deterministic first-in-bucket. Also normalises Windows backslashes in lookup paths (Wine cmdlines use `S:\Spiele\...`).

**Gap 2: wrapper processes show as "(unknown)" — the actual game is a descendant.** GameMode registers wrappers (umu-run, steam-runtime-l), not the game. Fixed: three-layer identification in the Enricher (`identify_wrapper`):
- **Layer 1 — wrapper cmdline**: the wrapper's own `/proc/<pid>/cmdline` ends with the game path (`... proton waitforexitandrun /path/Game.exe`). Cheapest, no tree walk.
- **Layer 2 — descendant walk**: for connected trees (native Steam, non-portal spawns), walk `/proc/*/task/*/children` bounded (depth 12, breadth 64), matching exe links AND cmdline tokens (Wine games: exe link is `wine64-preloader`, the real exe is a cmdline token).
- **Layer 3 — sandbox-family scan**: Lutris-Flatpak spawns via `org.freedesktop.portal.Flatpak`, which severs the process tree (sandbox's parent is the portal, not the wrapper — `/proc/<pid>/task/<pid>/children` is empty at the boundary). All sandbox members share the umu `var/tmp-XXXXXX` token in their cmdlines; scan `/proc` for it, then identify among family members and their descendants.

**Gap 3: games launched minutes after the wrapper aren't retried.** Battle.net launcher → actual game starts ~2 min later; identification at registration time finds nothing. Fixed: `unresolved_wrappers` set in the Enricher, retried every 15s via a `tick()` in the main loop's `tokio::select!`. Also covers the startup race: the naming DB loads after sources spawn, so seed-time wrappers are queued for retry.

**Gap 4: games launched without GameMode are invisible.** The S4a "reactive piggyback, no polling" decision (brainstorm) meant Steam enrichment only triggers when another source reports a pid. A Steam game launched without `gamemoderun` produces no source event. Found live: Resident Evil 2 launched without gamemoderun was completely invisible despite `SteamAppId=883710` in the whole chain's environ. Fixed: the 15s `tick()` also runs a bounded `/proc/*/environ` scan (~500 processes, ~5ms) for numeric Steam appids not yet tracked; new pids get a Steam partial (ancestor-walk merges the family), vanished pids are reconciled. This extends the S4a decision with a measured justification — the reactive-only gap was proven real by a user's game, not speculation.

**Gap 5: Steam scan exploded into ~20 records per game.** The scan emitted Steam partials for every process in the wrapper chain (wineserver, tabtip.exe, explorer.exe, rpcss.exe, etc. — all inherit the appid). The pairwise ancestor-walk merge couldn't converge: siblings at the same depth merged arbitrarily, utility processes absorbed the actual game, and the merge only worked for direct ancestor/descendant pairs. Fixed in two steps: (1) `identify_process` filter — only emit Steam partials for processes whose exe or cmdline matches detectable.json (skips utility processes); (2) **replaced the entire pairwise merge with `appid_records: HashMap<String, u32>`** (merge key → deepest pid) — one record per appid, `tree_depth` decides, no convergence issues. The user's simplification suggestion ("why not just a HashMap with appids as key?") was exactly right.

**Gap 6: merge key generalisation beyond Steam.** Amnesia (Lutris/umu, no SteamAppId) showed as duplicates because the ancestor-walk only triggered on Steam appids. Fixed: `probe_merge_key` generalises the merge key — `steam:<appid>` (Steam), `lutris:<uuid>` (Lutris `LUTRIS_GAME_UUID`), `umu:<id>` (umu `UMU_ID`, when not `umu-default`). Amnesia merges into one record via `lutris:37b63bec-...`.

**Gap 7: SteamAppId=0 treated as valid appid.** Heroic/GOG games set `SteamAppId=0` for Proton compatibility. The scan treated "0" as a valid appid, creating useless `steam=0` records. Fixed: `find_steam_appid` rejects "0".

**Gap 8: tracing filter overrode RUST_LOG.** `add_directive("gamebus_presenced=info")` always forced info level, ignoring `RUST_LOG=debug`. Fixed: `try_from_default_env()` with info fallback.

**Gap 9: cache re-adopted stale Steam-only records.** The restart cache stored scan-found Steam-only records; on restart, they were re-adopted even though the scan would re-find them (or they were stale). Fixed: `sync_cache` excludes records whose only source is Steam.

**Gap 10: wrapper executables shown as game names.** `libgamemodeauto` preload registers every process with GameMode, including shells and wrappers (`env`, `bash`, `python3`, etc.). The executable stem became the record's name. Fixed: `is_wrapper_executable()` clears the name for known wrappers.

**Gap 11: Discord integration tests fail when real Discord is running.** The proxy forwards the test's invalid client_id to the real Discord upstream, which rejects it. Fixed: `correlator_integration` and `discord_integration` tests now skip when a connectable `discord-ipc-1..=9` socket exists.

**Test additions**: 3 naming tests (path-suffix, basename collision, plain+prefixed coexist), is_ancestor/read_ppid tests, ancestor-walk merge tests. 61 unit + 6 integration tests pass; clippy clean.

**Reality-check on existing crates** (user question): no crate does runtime game detection/correlation. `rgd` (720 dl) is *installed*-game detection, not runtime. gamescope has no detection API (compositor, no metadata). umu-launcher assigns identity via env vars at launch, no runtime detection (confirmed in S3 research). All Rust Discord crates (`discord-rich-presence` 415k dl, `discord-presence` 90k dl, `bevy-discord-presence`) are *presence publishers* (client side), not detectors. arRPC does process detection via detectable.json but is JS, GPL-v3. The daemon's niche is real.

## Related

- [[wiki/projects/gamebus-presenced]] — project note (S0-S4 complete)
- [[wiki/concepts/2026-08-04 - Brainstorm - S4 Enrichment and Packaging]] — S4 brainstorm (7 decisions, 4 open questions)
- [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]] — correlator merge rules (extended for Steam)
- [[wiki/logs/2026-08-04 - gamebus-presenced S3]] — S3 dev log
- `PLAN.md` — roadmap (S0-S4 done)
- `docs/design/gamebus-presence.md` — design doc
