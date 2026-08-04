---
date: 2026-08-04
type: learnings-review
tags: [learnings-review, thinking]
period-days: 30
ai-first: true
---

## For future Claude

Second learnings review covering 2026-08-04 (the vault's first day, now with S4 complete). All 25 vault notes were read: 8 ADRs, 5 dev logs (S0-S4), 5 concept notes, 1 project note, 1 daily note, 1 research note, 1 brainstorm, 1 entity stub, 1 board, 1 ops log. The S4 session added 2 new active learnings (one-record-per-key, detectable.json path-suffix), confirmed the real-client-testing promotion candidate (4th occurrence via live Amnesia/RE2 verification), and added 1 new supersession (pairwise ancestor-walk → appid_records HashMap). Vault is still one day old — the stale criterion (6+ months) cannot fire.

## Active Learnings (still applies)

### 1. Verify live state before trusting docs or assumptions
Two design-doc assumptions broke against the live system during S1 ([[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]]). Reinforced again in S4: the pairwise ancestor-walk merge was instrument-first (debug log before code) per the brainstorm's explicit decision, and detectable.json path-suffix matching was discovered empirically from the data (83% of entries are path-prefixed). The S4 brainstorm ([[wiki/concepts/2026-08-04 - Brainstorm - S4 Enrichment and Packaging]]) explicitly cited this principle as ADR-005/ADR-006 carry-forward. Already a standing rule (`_CLAUDE.md` §0.5) and the most-reinforced principle of the day (6+ occurrences).
- Sources: ADR-005, ADR-006, S1/S3/S4 dev logs, `_CLAUDE.md` §0.5, S4 brainstorm

### 2. Reuse battle-tested code; never hand-weave or vendor what exists
Owner enforced this twice during S2 (ADR-006). In S4, detectable.json was fetched from Discord's endpoint (not hand-built), and `ureq` was used for the HTTP client (not hand-rolled). Corollary: check licences before reuse — `pog5/rsrpc` is GPLv3 (learn from, not vendor), `SpikeHD/rsRPC` is MIT (depend on), umu-launcher/umu-database are GPL-3.0 (query-only).
- Sources: ADR-006, S2 dev log, S4 dev log (build.rs)

### 3. Integration-test against real clients and fixtures, not mocks
S1 used `gamemoderun sleep 30` + `busctl` acceptance. S2 used a genuine `discord-rich-presence` client. S3 used a fixture upstream for proxy byte-identity + `RegisterGameByPID` for same-pid join. S4 used live games (Brotato, Amnesia: The Bunker, Resident Evil 2) for end-to-end verification — catching 11 bugs that unit tests missed (merge direction, SteamAppId=default, path-suffix matching, appid explosion, etc.). Now at **4 occurrences** (S1, S2, S3, S4) — eligible for promotion.
- Sources: S1/S2/S3/S4 dev logs, [[wiki/concepts/2026-08-04 - Learnings Review]] (promotion candidate #1)

### 4. Let the test environment's reality shape assertions, never fight it
`libgamemodeauto` is preloaded globally — `HasActivity == false` is never assertable. `SO_PEERCRED` yields the test process's own pid (used deliberately for the same-pid correlator test). gamemoded's dead-client reaper takes 4-18s — tests use explicit `UnregisterGameByPID` for determinism. Discord's proxy rejects test client_ids — tests skip when a real Discord is running. In S4: daemon-already-running causes `Connection(NameTaken)` — tests must not assume a clean bus.
- Sources: S1/S2/S4 dev logs, S4 test isolation issues (#9-#12, #19)

### 5. Index identities at arrival time, not at publish time
Found via a failing correlator test in S3 (ADR-007 decision 5): after `pid_<pid>` absorbs `discord_<pid>`, the absorbed source still references its own scoped id in removal events. Indexing only published ids misses the gamemode-first order. In S4, this principle extended to `appid_records`: the merge key is indexed per-scan, and the scan skips already-indexed pids — no re-emission.
- Sources: ADR-007, S3 dev log, [[wiki/logs/2026-08-04 - gamebus-presenced S4]] (scan reconciliation)

### 6. Unify sources behind one event channel before you need the second source
S2's `SourceEvent` unification made S3's correlator a pure addition. S4's Enricher middleware (`process(SourceEvent) -> Vec<SourceEvent>`) extended this: Steam, naming, and ancestor-walk all flow through the same channel without touching the core loop. The brainstorm ([[wiki/concepts/2026-08-04 - Brainstorm - S4 Enrichment and Packaging]]) explicitly chose the Enricher pattern over probe-in-main for this reason — separation of concerns pays off long-term (owner's principle).
- Sources: S2/S3 dev logs, S4 brainstorm, [[wiki/logs/2026-08-04 - gamebus-presenced S4]]

### 7. Flag test deletions visibly with rationale and coverage mapping
Owner preference, stated after `test_remove_by_source` was silently deleted in S3. In S4, no tests were deleted — the pairwise ancestor-walk tests were replaced by `appid_records`-based tests, and the old `find_related_pid`/`find_all_related` tests were removed with visible rationale in the dev log.
- Sources: [[wiki/concepts/test-deletion-visibility]], ADR-007 Alternatives, S4 dev log

### 8. One record per key — prefer HashMap<key, representative> over pairwise merge
When deduplicating by a shared key across a hierarchy, a single HashMap mapping each key to exactly ONE representative (decided by `tree_depth`) is simpler and more robust than pairwise merging. The pairwise approach had convergence issues with siblings (same-depth processes), arbitrary merge direction, and re-emission on subsequent ticks. The one-record-per-key approach is O(1) per new item, deterministic, and order-independent. The user's simplification suggestion ("why not just a HashMap with appids as key?") was exactly right.
- Sources: [[wiki/concepts/2026-08-04 - one-record-per-key]], [[wiki/decisions/adr-008-appid-records-one-record-per-merge-key]], S4 dev log (Gap 5)

### 9. Discord's detectable.json is 83% path-prefixed — use path-suffix matching
83% of executable entries in Discord's `detectable.json` (9297 out of 11218) are path-prefixed (`amnesia the bunker/amnesiathebunker.exe`). A basename-only index misses most of the database. Discord's own scanner does path-suffix matching (case-insensitive, backslash-normalized for Wine paths). Fix: bucket by basename, try path-suffix match → plain entry → deterministic first-in-bucket.
- Sources: [[wiki/concepts/2026-08-04 - detectable-json-path-suffix-matching]], S4 dev log (Gap 1)

## Stale Learnings (consider archiving)

None. The vault is one day old (created 2026-08-04); the 6-months-without-reinforcement criterion cannot fire. First meaningful stale review is possible from 2027-02 onward.

**Note-drift observation (same as first review):** [[wiki/decisions/adr-004-manager-and-activity-interfaces]] documents `ListActivities -> Vec<Activity>` and an early property set; the S1 implementation returns `ao` object paths with the final property list. The decision itself (Manager + per-activity objects) stands — the note's details drifted. Suggest: annotate ADR-004 with a pointer to the S1 dev log rather than rewriting history.

## Superseded Learnings (already replaced)

| Old position | New position | Reference |
|---|---|---|
| GameMode hands over (pid, executable) directly | Signals/ListGames carry (pid, per-game object path); details from `com.feralinteractive.GameMode.Game` properties | [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]] |
| Hyphenated `pid-<pid>` activity IDs | `pid_<pid>` — hyphens illegal in D-Bus path elements | ADR-005 |
| Registry-level `remove_by_source` (S1 single-source assumption) | Correlator degrades multi-source records in place; method + test deleted | [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]] |
| Solo records per source (S2: `discord_<pid>` standalone) | Merged derived view per pid; `pid_<pid>` absorbs `discord_<pid>` | ADR-007 |
| **Pairwise ancestor-walk merge (S4d)** | **`appid_records: HashMap<String, u32>`** — one record per merge key, `tree_depth` decides | **[[wiki/decisions/adr-008-appid-records-one-record-per-merge-key]]** (new) |
| **Reactive-only Steam probing (S4a brainstorm decision)** | **Reactive piggyback + 15s bounded `/proc/*/environ` scan** — the reactive-only gap was proven real by Resident Evil 2 launched without gamemoderun | **[[wiki/logs/2026-08-04 - gamebus-presenced S4]]** (Gap 4) (new) |
| **Ancestor-walk merge keyed on Steam appid only (S4d)** | **Generalised merge key: `steam:<appid>` / `lutris:<uuid>` / `umu:<id>`** — Amnesia (Lutris/umu) had no Steam appid | **S4 dev log (Gap 6)** (new) |

## Promotion Candidates (appeared 3+ times)

### 1. Real-client/fixture integration testing — NOW ELIGIBLE (4 occurrences)
**Status:** Was a candidate in the first review (3 occurrences, awaiting S4). S4 used live games (Amnesia, RE2) for end-to-end verification → **4th occurrence confirmed**.

**Suggested wording for `_CLAUDE.md`:**

> Integration tests exercise the real wire: genuine client libraries, real daemons (gamemoded), or recording fixtures that capture bytes — never hand-written mocks of the protocol under test. Tests must skip gracefully when the session bus, gamemoded, or a real Discord client is unavailable. When a design can only be verified against a real game (wrapper trees, process identification), run the daemon against a live game and assert the bus state.

**Confidence:** High — 4 occurrences in one project on one day; the pattern caught 11 bugs that unit tests missed.

### 2. One record per key — NOT YET ELIGIBLE (2 occurrences)
Appeared in the S4 dev log and ADR-008. Strong but young — needs one more recurrence (different project or different domain) before promoting to a general operating rule.

## Top 5 Lessons of the Period

1. **Verify live state before acting** — frequency 6+, consequence highest. Prevented design-doc-driven bugs in S1 (D-Bus charset, GameMode API) and guided S4's instrument-first ancestor-walk and empirical detectable.json path-suffix discovery. Already a permanent rule (`_CLAUDE.md` §0.5).

2. **One record per key over pairwise merge** — frequency 2 (S4d failure + S4e simplification), consequence high (eliminated ~20-record-per-game explosion, replaced entire merge architecture). The user's insight was the breakthrough. Formalised in ADR-008.

3. **Integration-test against the real thing** — frequency 4 (S1-S4), consequence high (caught 11 bugs unit tests missed, including merge direction, SteamAppId=default, path-suffix gap). Now eligible for promotion to `_CLAUDE.md`.

4. **Let the test environment shape assertions** — frequency 4+ (libgamemodeauto preload, SO_PEERCRED, gamemoded reaper, Discord proxy). Consequence medium-high (test isolation is a recurring source of false failures). Each occurrence is environment-specific, not a code bug.

5. **Reuse battle-tested code; check the licence first** — frequency 3+ (rsrpc crate, ureq HTTP client, detectable.json database). Consequence medium (avoided hand-writing Discord payload parsing and HTTP fetching). Already enforced by owner.

## Comparison with First Review

| Metric | First review | This review | Delta |
|---|---|---|---|
| Active learnings | 7 | 9 | +2 (one-record-per-key, detectable-json path-suffix) |
| Stale | 0 | 0 | — (vault too young) |
| Superseded | 4 | 7 | +3 (pairwise merge, reactive-only Steam, appid-only merge key) |
| Promotion candidates | 1 (awaiting 4th) | 1 (now eligible) | Real-client testing confirmed |
| Unit tests | 43 | 61 | +18 |
| Integration tests | 5 | 6 | +1 (steam_enrichment) |
| ADRs | 7 | 8 | +1 (ADR-008 appid_records) |
| Dev logs | 4 (S0-S3) | 5 (S0-S4) | +1 |
| Concept notes | 2 | 5 | +3 (brainstorm, one-record-per-key, detectable-json) |

## Related

- [[wiki/projects/gamebus-presenced]] — project note (S0-S4 complete)
- [[wiki/concepts/2026-08-04 - Learnings Review]] — first learnings review (baseline)
- [[wiki/concepts/test-deletion-visibility]] — owner preference (active learning #7)
- [[wiki/concepts/2026-08-04 - one-record-per-key]] — design pattern (active learning #8)
- [[wiki/concepts/2026-08-04 - detectable-json-path-suffix-matching]] — technical insight (active learning #9)
- [[wiki/concepts/2026-08-04 - Brainstorm - S4 Enrichment and Packaging]] — S4 brainstorm
- [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]] — verify-live-state
- [[wiki/decisions/adr-006-rsrpc-crate-dependency]] — reuse battle-tested code
- [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]] — correlator merge rules (partially superseded by ADR-008)
- [[wiki/decisions/adr-008-appid-records-one-record-per-merge-key]] — one record per merge key
- [[wiki/logs/2026-08-04 - gamebus-presenced S4]] — S4 dev log (11 gaps, 27 issues)
