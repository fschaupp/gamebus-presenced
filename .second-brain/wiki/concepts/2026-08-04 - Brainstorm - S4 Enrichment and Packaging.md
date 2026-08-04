---
date: 2026-08-04
type: brainstorm
tags: [brainstorm, gamebus-presenced, s4, steam, enrichment]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

Brainstorm for S4 of [[wiki/projects/gamebus-presenced]] — enrichment and packaging. Conducted 2026-08-04 via Socratic interview (6 questions, all converged). Decided: S4 splits into four sub-slices (S4a Steam source, S4b naming enrichment, S4c packaging, S4d ancestor-walk candidate). Steam is reactive via an Enricher middleware layer between sources and correlator. `detectable.json` ships as an installation data file (not embedded in the binary). Naming is strictly enrichment-only — never overrides a more authoritative source. Ancestor-walk is instrument-first (debug log on join-miss, decide based on real miss rate). Use this note when implementing S4; the open questions section has design details the dev session should resolve.

## Problem

S4 enriches gamebus-presenced with Steam appid data, human-readable game naming, and deployment packaging — moving from "functional daemon" to "deployable, enriched product."

## Constraints and anti-goals

### Constraints
- **Daemon stays network-free** — no HTTP client in the daemon process. `detectable.json` fetch is a CLI subcommand, not a runtime call.
- **Correlator stays pure** (ADR-007) — no I/O in the correlator. The Enricher handles all enrichment I/O.
- **Naming is enrichment-only** — `detectable.json` never overrides a non-empty name from a more authoritative source.
- **Steam source is reactive** — no independent watcher or polling loop. Probes `/proc/<pid>/environ` only when another source reports a pid.
- **`detectable.json` ships as an installation data file** — not embedded in the binary. `build.rs` fetches at build time; packaging installs it to `$PREFIX/share/gamebus-presenced/detectable.json`.

### Anti-goals
- **Naming overrides a more authoritative source** — the single failure condition. If `detectable.json` ever wins over Discord's `name` or GameMode's executable stem when those are non-empty, S4b is broken by definition. Precedence: Discord name > detectable.json lookup (by appid or executable) > executable stem.

## Approaches considered

### Approach 1: "Probe-in-main"
`main.rs` intercepts `SourceEvent::Updated` events and calls `steam::probe(pid)` directly. If found, emits a second `SourceEvent::Updated` with `Source::Steam` into the correlator. No new abstraction layer.

| Pro | Con |
|---|---|
| Correlator stays pure | main.rs grows with enrichment logic |
| Steam is a normal source partial | Future enrichment also flows through main.rs |
| Matches S3 pattern | Two event emissions per pid arrival |

### Approach 2: "Enricher middleware" (CHOSEN)
A new `Enricher` struct sits between sources and correlator: `sources → SourceEvent → Enricher → Vec<SourceEvent> → Correlator`. The Enricher probes `/proc/<pid>/environ` for Steam data, and later (S4b) applies `detectable.json` naming. The correlator receives already-enriched events.

| Pro | Con |
|---|---|
| Separation of concerns — pays off long-term | New abstraction for one current use case |
| Extensible for future "misbehaving" launchers | Over-engineered for S4a's scope alone |
| Keeps the door open for tracing/profiling | Correlator no longer sees raw source events |
| main.rs stays thin | |

**Owner chose Approach 2** for long-run separation of concerns and future launcher extensibility.

## Decisions made

### 1. S4 splits into four sub-slices
- **S4a — Steam source**: reactive probe via Enricher middleware
- **S4b — Naming enrichment**: `detectable.json` lookup, enrichment-only fallback
- **S4c — Packaging**: `gamebus-presence` CLI + systemd user unit + D-Bus activation file
- **S4d (candidate, trigger-only)**: bounded ancestor-walk join, only if S4a's measured misses justify it

Rationale: small verifiable slices match the S0→S3 pattern. Each sub-slice is independently testable.

### 2. Steam source is reactive piggyback (no polling)
The Steam source has no independent watcher. The Enricher probes `/proc/<pid>/environ` for `SteamAppId`/`SteamGameId`/`UMU_ID`/`STORE` only when another source (GameMode, Discord) reports a pid. A numeric `umu-<N>` in `UMU_ID` implies steam appid N per umu's convention (researched 2026-08-04, [[wiki/projects/gamebus-presenced]] S3 dev log).

Rationale: Steam has no push mechanism. Polling `/proc/*/environ` adds overhead for marginal coverage (Steam-only launches with no GameMode or Discord). The reactive path covers the common case (game registered with GameMode + launched from Steam).

### 3. Enricher middleware architecture
```
sources → SourceEvent → Enricher → Vec<SourceEvent> → Correlator
                          │
                          ├─ steam::probe(pid) on new pid arrival
                          ├─ (S4b) detectable.json naming lookup
                          └─ (future) other launcher enrichment
```

The Enricher is a stateful struct in a new `src/enricher.rs` module. It tracks `{pid: Set<Source>}` to manage Steam partial lifecycle. The correlator stays pure (ADR-007 principle preserved). See [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]].

### 4. Steam partial removal: tie to last non-Steam source
When the last non-Steam source removes its record for a pid, the Enricher emits `SourceEvent::Removed` for Steam. This prevents stale Steam data surviving a pid reuse — the pid could be reused before a re-probe would catch it. The Enricher tracks which non-Steam sources are active per pid.

Rationale: pid reuse is a real risk on Linux. Tying Steam removal to the triggering sources' removal is conservative and correct — no Steam data outlives the sources that justified probing for it.

### 5. `detectable.json` layered strategy
- **Build time**: `build.rs` fetches `detectable.json` from Discord's `applications/detectable` endpoint, writes to output directory
- **Installation**: ships as a data file at `$PREFIX/share/gamebus-presenced/detectable.json` (not embedded in the binary)
- **CLI refresh**: `gamebus-presence fetch-detectable` downloads to `$XDG_CACHE_HOME/gamebus-presenced/detectable.json` (overrides installed copy)
- **Runtime lookup order**: `$XDG_CACHE_HOME` (CLI-refreshed) > installed data path > absent (degrade to executable stem)
- **Later**: runtime auto-fetch as a config option (not in S4 scope)

Rationale: daemon stays network-free. The installed data file can be updated without recompiling. The CLI gives the user control over refresh. Standard Linux packaging conventions.

### 6. Ancestor-walk: instrument first
S4a adds `tracing::debug!` when a Discord connect's pid doesn't match any GameMode registration (a "join-miss"). After running S4a with real games, the logs determine whether S4d (bounded ancestor-walk) is worth building. No speculative walk code.

Rationale: follows the "verify live state before acting" principle ([[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]], `_CLAUDE.md` §0.5). We don't know the miss rate yet. A debug log is cheap; the data decides.

### 7. Anti-goal: naming overrides a more authoritative source
`detectable.json` naming is strictly a fallback. Precedence: Discord `name` (human-curated, most reliable) > detectable.json lookup (by appid or executable, cached) > executable stem from GameMode (last resort). If this precedence is ever violated, S4b is broken by definition.

## Open questions

### Enricher state management
The Enricher tracks `{pid: Set<Source>}` for Steam partial lifecycle. When a `SourceEvent::Removed` arrives for a non-Steam source, the Enricher removes that source from the pid's set. If the set is now empty, emit `SourceEvent::Removed` for Steam. Implementation detail for the S4a dev session:
- Does the Enricher use the `id` from `SourceEvent::Removed` to infer the pid, or does it track pid separately?
- The `id` is source-scoped (`pid_<pid>`, `discord_<pid>`) — parsing the pid from the id is fragile. Better: the Enricher maintains its own `{pid: Set<Source>}` map updated on every `SourceEvent::Updated`.

### Enricher in the event loop vs separate task
The Enricher processes events synchronously in the main event loop (between `rx.recv()` and `correlator.on_updated()`). The `/proc/<pid>/environ` read is fast (single file read). If blocking becomes a concern, the Enricher could run as a separate task with its own channel. Defer to profiling if needed — the owner noted tracing/profiling as a future concern the Enricher pattern accommodates.

### Steam `Activity::from_steam()` mapping
`Activity::from_steam(pid, appid) -> Activity` — maps to `id = steam_<pid>`, `sources = [Source::Steam]`, `app_ids["steam"] = appid`, `kind = Kind::Game`. Other fields stay empty (Steam knows only the appid). The correlator merge rule: Steam owns `app_ids["steam"]`, contributes nothing to human-facing fields (name, details, state, artwork).

### umu `UMU_ID`/`STORE` parsing
`UMU_ID=umu-<N>` where N is numeric implies steam appid N. `STORE` is the store front (e.g. "steam"). The Enricher should check `UMU_ID` first (umu-launcher convention), then fall back to `SteamAppId`/`SteamGameId` (native Steam). Both map to `app_ids["steam"]`.

## Related

- [[wiki/projects/gamebus-presenced]] — project note (S0-S3 complete, S4 next)
- [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]] — correlator merge rules, purity principle
- [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]] — verify-live-state principle
- [[wiki/decisions/adr-006-rsrpc-crate-dependency]] — reuse battle-tested code
- [[wiki/concepts/test-deletion-visibility]] — flag test deletions visibly
- [[wiki/concepts/2026-08-04 - Learnings Review]] — learnings review (verify-live-state most reinforced)
- `docs/design/gamebus-presence.md` — design doc (S4 slice description)
- `PLAN.md` — roadmap (S4 planned)
