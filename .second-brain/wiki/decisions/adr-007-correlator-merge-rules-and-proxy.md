---
date: 2026-08-04
type: decision
title: Correlator merge rules, pid_<pid> absorption, and proxy degradation behavior
status: accepted
related-projects: [gamebus-presenced]
ai-first: true
tags: [decision, architecture, correlator, discord, ipc, proxy, cache]
---

## For future Claude

ADR-007: How S3 joins sources and degrades. The correlator keeps per-source partial records keyed by exact pid and derives the published Activity from them. A merged record keeps GameMode's `pid_<pid>` identity and absorbs `discord_<pid>`; Discord's human-facing fields win, GameMode owns executable; records degrade in place and die with the last source. The proxy closes the client connection on upstream loss. The restart cache re-adopts by pid + `/proc/<pid>/stat` start-time. The umu/Proton wrapper-tree join is a documented miss, deferred to S4.

## Context

S3 (proxy + correlator + restart cache) needed answers to: what happens when GameMode's `pid_X` and Discord's `discord_X` are the same process, which identity survives on the bus, what a running Discord upstream means for the standalone listener, and how a daemon restart avoids losing presence. Three decision points were confirmed by the owner during planning (2026-08-04): landing order proxy -> correlator -> cache, `pid_<pid>` absorbs (recommended), close client connection on upstream loss (recommended).

Prior decisions: [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]] (id charset), [[wiki/decisions/adr-006-rsrpc-crate-dependency]] (rsRPC payload model + transport boundary). The design doc's merge philosophy: "the record dies with the last source", "a property never regresses to unknown while a source still asserts it", "does not fabricate a join".

## Decision

1. **Per-source partial store keyed by exact pid** (`src/correlator.rs`): the published `Activity` is a derived view, not a stored flat record. `Manager` is now a plain id->Activity registry; its `remove_by_source` helper was deleted (source-loss handling lives in the correlator's `on_source_lost`).
2. **`pid_<pid>` absorbs `discord_<pid>` on a join**: GameMode is the more reliable source (kernel-tracked registration vs. self-reported presence) and a stable id spares consumers object-path churn. An absorbed solo object gets ActivityRemoved; the survivor updates in place.
3. **Field precedence**: Discord wins the human-facing fields (name, details/state, since/until, artwork, party) - those are the point of the Discord source. GameMode owns executable. `since` prefers Discord's game-reported timestamp, GameMode's registration time as fallback. `AppIds`/`Extra` are unions. `Kind` is game if any source says game. `Sources` is canonical order: [gamemode, discord].
4. **Split rules**: losing Discord degrades the merged record in place (PropertiesChanged); losing GameMode splits back to the solo `discord_<pid>` identity (Remove + PublishNew). The record dies only with the last partial.
5. **id_index keeps every id a pid has ever published under**: sources reference their own scoped id in removal events, and after an absorb the published id no longer matches the id the absorbed source will use.
6. **Proxy degradation**: upstream loss mid-connection closes the client connection (both streams dropped, client sees EOF). Reconnect behavior is the client's choice (design doc: `discord-rpc` has no internal retry timer); a reconnect is re-evaluated and lands in standalone mode if Discord stayed away. Frames are forwarded whole (`Frame::encode()` of a parsed frame reproduces the wire bytes exactly); the SET_ACTIVITY tap is passive - parse problems are logged, never propagated.
7. **Restart cache**: write-through JSON at `$XDG_RUNTIME_DIR/gamebus-presenced/cache.json` (tmpfs: survives daemon restart, dies at logout), keyed by pid + `/proc/<pid>/stat` field-22 start-time. Re-adopted records are published but NOT stored as source partials: re-deriving sources overwrite them naturally, fields whose source never returns stay at last-known values, and a record whose process later dies lingers until the next restart (accepted; best-effort per the design doc).
8. **Join is exact-pid only**. The umu/Proton wrapper tree - GameMode registers the `umu-run` wrapper pid, Discord connects from a wine child process - is a documented miss, not a fabricated join. A bounded ancestor-walk join (ppid chain, start-time validated) is an S4 candidate if measured miss rates justify it.

## Consequences

- `Activity` gained `PartialEq, Eq, Serialize, Deserialize` derives (correlator effect assertions + cache serialisation).
- Unit coverage: 8 correlator tests (merge/split both orders, solo, no-join, churn-free update, pid-0 passthrough) + 6 cache tests (stat parsing incl. comm-with-parens, round-trip, pid-reuse rejection, dead process, corrupt cache). Integration: proxy byte-identity vs fixture, same-pid join vs real client + `RegisterGameByPID`, SIGKILL/respawn re-adoption.
- umu research (2026-08-04, https://github.com/Open-Wine-Components/umu-launcher and /umu-database): `UMU_ID`/`STORE` in `/proc/<pid>/environ` becomes an S4 appid source - a numeric `umu-<N>` implies steam appid N per umu's own convention. umu-database is a cached secondary naming source only (protonfixes-scoped, "by no means a complete database" per its README); both repos are GPL-3.0, so query/interop only, no vendoring.

## Alternatives Considered

- **New `merged_<pid>` identity for joined records**: cleanest semantics, but consumers see id churn on every join/split; rejected by owner in favor of absorption.
- **Keep both objects and cross-link them**: pushes the join onto every consumer, which defeats the correlator's point; rejected.
- **Seamless proxy takeover on upstream loss** (start answering READY/echo mid-connection): no client-visible break, but we would fake responses for frames Discord already saw - state divergence risk; rejected by owner.
- **Keep `Manager::remove_by_source` as defense-in-depth**: testing a method no caller reaches asserts behavior of code that never runs; removed along with its test (correlator tests cover the semantics at the level where they are exercised). Flagged to owner, who confirmed the removal 2026-08-04; the exchange also produced [[wiki/concepts/test-deletion-visibility]].

## Related

- [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]]
- [[wiki/decisions/adr-006-rsrpc-crate-dependency]]
- [[wiki/projects/gamebus-presenced]]
- [[wiki/logs/2026-08-04 - gamebus-presenced S3]]
