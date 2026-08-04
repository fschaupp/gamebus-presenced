---
date: 2026-08-04
type: devlog
tags: [devlog, gamebus-presenced, s4, steam, enrichment, naming, cli]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

Dev log for S4 of [[wiki/projects/gamebus-presenced]] — enrichment and packaging, completed 2026-08-04. Three sub-slices landed in one session: S4a (Steam enrichment via Enricher middleware), S4b (naming via detectable.json), S4c (packaging: CLI + systemd). 52 unit tests + 6 integration tests pass; clippy fully clean. The Enricher middleware pattern (chosen over probe-in-main for separation of concerns) accommodates future launcher enrichment and tracing/profiling. S4d (ancestor-walk join) is a candidate, trigger-only — instrumentation is in place (tracing::debug! on Discord-pid join-miss).

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

## Test results

- 52 unit tests pass (7 enricher, 5 naming, 4 correlator steam, 1 types from_steam, plus all existing)
- 6 integration tests pass: gamemode, discord, discord_proxy, correlator, restart_cache, steam_enrichment
- `cargo clippy --all-targets` fully clean, zero warnings
- `cargo fmt` applied

## Issues encountered

1. **Naming DB load blocks startup** — the 12MB JSON parse in `Enricher::new()` delayed the Discord listener by ~100ms, causing the correlator integration test to fail (RPC client connects before the socket is bound). Fix: `Enricher::new()` starts with `naming: None`; `load_naming()` is called after sources spawn.

2. **`Source` needs `Hash` derive** — the Enricher uses `HashSet<Source>` for tracking active sources per pid. Added `Hash` to the derive list.

3. **`unused_mut` warnings in tests** — two test functions had unnecessary `mut` bindings. Fixed.

## Related

- [[wiki/projects/gamebus-presenced]] — project note (S0-S4c complete)
- [[wiki/concepts/2026-08-04 - Brainstorm - S4 Enrichment and Packaging]] — S4 brainstorm (7 decisions, 4 open questions)
- [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]] — correlator merge rules (extended for Steam)
- [[wiki/logs/2026-08-04 - gamebus-presenced S3]] — S3 dev log
- `PLAN.md` — roadmap (S0-S4c done, S4d candidate)
- `docs/design/gamebus-presence.md` — design doc
