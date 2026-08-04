---
date: 2026-08-04
type: learnings-review
tags: [learnings-review, thinking]
period-days: 30
ai-first: true
---

## For future Claude

Learnings review covering 2026-08-04 (the vault's first day; scope `recent` = last 30 days, which is everything). All 16 vault notes were read exhaustively: 7 ADRs, 4 dev logs, 1 project note, 1 daily note, 1 concept note, 1 entity stub, 1 research note. Because the vault is one day old, the Stale classification cannot fire (6+ month criterion) and every Active learning is same-day - treat "reinforcement" counts as early signal, not established pattern.

## Active Learnings (still applies)

### 1. Verify live state before trusting docs or assumptions
Two design-doc assumptions broke against the live system during S1 ([[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]]): hyphens are illegal in D-Bus object path elements (zbus runtime rejection), and GameMode signals carry per-game object paths, not executables. Reinforced in S2/S3: the pinned rsrpc crate was assessed as-found before depending on it ([[wiki/decisions/adr-006-rsrpc-crate-dependency]]), and `/proc/<pid>/stat` parsing was verified against the real file. This is already a standing rule (`_CLAUDE.md` Section 0.5) and was the most-reinforced principle of the day.
- Sources: ADR-005, ADR-006, [[wiki/logs/2026-08-04 - gamebus-presenced S1]], `_CLAUDE.md` 0.5

### 2. Reuse battle-tested code; never hand-weave or vendor what exists
Owner enforced this twice during S2 planning: depend on the pinned MIT rsrpc crate for the Discord payload model rather than vendoring or hand-writing structs ([[wiki/decisions/adr-006-rsrpc-crate-dependency]]). Corollary: check licences before reuse - `pog5/rsrpc` is GPLv3 (learn from, never vendor), `SpikeHD/rsRPC` is MIT (depend on), umu-launcher/umu-database are GPL-3.0 (query/interop only).
- Sources: ADR-006, ADR-007 Consequences

### 3. Integration-test against real clients and fixtures, not mocks
S1 used `gamemoderun sleep 30` + `busctl` acceptance on the real bus; S2 used a genuine `discord-rich-presence` client; S3 used a recording fixture upstream (byte-identity proxy proof), `RegisterGameByPID` for the same-pid join, and SIGKILL/respawn for the cache. All integration tests skip gracefully without a session bus. Mocks would have missed the GameMode API shape and the id_index bug.
- Sources: [[wiki/logs/2026-08-04 - gamebus-presenced S1]], [[wiki/logs/2026-08-04 - gamebus-presenced S2]], [[wiki/logs/2026-08-04 - gamebus-presenced S3]]

### 4. Let the test environment's reality shape assertions, never fight it
This machine preloads `libgamemodeauto` globally, so `HasActivity == false` is never asserted (unrelated processes legitimately hold activities), and `SO_PEERCRED` yields the test process's own pid (used deliberately for the same-pid correlator test). gamemoded's dead-client reaper interval was observed at 4-18s - tests use explicit `UnregisterGameByPID` for determinism instead.
- Sources: S1 dev log Verification, S2 dev log Verification, `tests/correlator_integration.rs`

### 5. Index identities at arrival time, not at publish time
Found via a failing correlator test: after `pid_<pid>` absorbs `discord_<pid>`, the absorbed source still references its own scoped id in removal events. Indexing only published ids misses the gamemode-first order entirely; the fix indexes every partial's source-scoped id on arrival ([[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]] decision 5).
- Sources: ADR-007, S3 dev log

### 6. Unify sources behind one event channel before you need the second source
S2's `SourceEvent` unification made the S3 correlator a pure addition (`src/correlator.rs` + effect execution) instead of a core rewrite. The core consumed source-agnostic effects; sources never learned about each other.
- Sources: S2 dev log, S3 dev log, ADR-007

### 7. Flag test deletions visibly with rationale and coverage mapping
Owner preference, stated after `test_remove_by_source` was silently deleted: name the removed test, show where the coverage moved, allow a veto. Captured as standing preference in [[wiki/concepts/test-deletion-visibility]].
- Sources: [[wiki/concepts/test-deletion-visibility]], ADR-007 Alternatives

## Stale Learnings (consider archiving)

None. The vault is one day old (created 2026-08-04); the 6-months-without-reinforcement criterion cannot fire. First meaningful stale review is possible from 2027-02 onward.

**Note-drift observation (not a stale learning):** [[wiki/decisions/adr-004-manager-and-activity-interfaces]] documents `ListActivities -> Vec<Activity>` and an early property set; the S1 implementation returns `ao` object paths with the final property list. The decision itself (Manager + per-activity objects) stands - the note's details drifted. Suggest: annotate ADR-004 with a pointer to the S1 dev log rather than rewriting history.

## Superseded Learnings (already replaced)

| Old position | New position | Reference |
|---|---|---|
| GameMode hands over (pid, executable) directly | Signals/ListGames carry (pid, per-game object path); details from `com.feralinteractive.GameMode.Game` properties | [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]] |
| Hyphenated `pid-<pid>` activity IDs | `pid_<pid>` - hyphens illegal in D-Bus path elements | ADR-005 |
| Registry-level `remove_by_source`, records removed on source loss (S1 single-source assumption) | Correlator degrades multi-source records in place; dies with last source; method + test deleted | [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]] |
| Solo records per source (S2: `discord_<pid>` standalone) | Merged derived view per pid; `pid_<pid>` absorbs `discord_<pid>` | ADR-007 |

## Promotion Candidates (appeared 3+ times)

### Already promoted: verify live state before acting
In `_CLAUDE.md` Section 0.5 since before today and reinforced 4+ times today (ADR-005, ADR-006, S1, S3). No action needed - the rule works.

### Candidate: real-client/fixture integration testing, mocks last resort
Appeared 3 times today (S1 gamemoderun, S2 genuine RPC client, S3 fixture upstream + RegisterGameByPID + SIGKILL). Suggested wording for `_CLAUDE.md` (project-scoped, or general):

> Integration tests exercise the real wire: genuine client libraries, real daemons (gamemoded), or recording fixtures that capture bytes - never hand-written mocks of the protocol under test. Tests must skip gracefully when the session bus or the daemon is unavailable.

Confidence: medium - three occurrences, but all within one project on one day. Recommend one more recurrence (S4) before promoting.

## Top 5 Lessons of the Period

1. **Verify live state before acting** - prevented two design-doc-driven bugs on day one (ADR-005) and shaped the rsrpc dependency (ADR-006). Frequency x consequence: highest.
2. **Reuse battle-tested code, check the licence first** - owner-enforced; the rsrpc dependency replaced a hand-written payload model (ADR-006).
3. **Real clients and fixtures in integration tests** - caught what mocks could not: GameMode's actual API shape, byte-level proxy fidelity, the id_index ordering bug.
4. **Index identities at arrival, not publish** - removal events reference source-scoped ids; publish-time indexing silently loses them (S3 failing test).
5. **Flag test deletions visibly** - the silent delete was corrected by the owner; now a standing preference ([[wiki/concepts/test-deletion-visibility]]).

## Related

- [[wiki/projects/gamebus-presenced]]
- [[wiki/concepts/test-deletion-visibility]]
- [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]]
- [[wiki/decisions/adr-006-rsrpc-crate-dependency]]
- [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]]
