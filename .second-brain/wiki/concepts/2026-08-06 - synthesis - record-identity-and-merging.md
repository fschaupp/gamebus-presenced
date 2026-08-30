---
date: 2026-08-06
type: synthesis
topic: record identity and merging in gamebus-presenced
tags: [research, thinking, vault-deep-synthesis]
ai-first: true
sources-read:
  - wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects.md
  - wiki/decisions/adr-006-rsrpc-crate-dependency.md
  - wiki/decisions/adr-007-correlator-merge-rules-and-proxy.md
  - wiki/decisions/adr-008-appid-records-one-record-per-merge-key.md
  - wiki/logs/2026-08-04 - gamebus-presenced S1.md
  - wiki/logs/2026-08-04 - gamebus-presenced S2.md
  - wiki/logs/2026-08-04 - gamebus-presenced S3.md
  - wiki/logs/2026-08-04 - gamebus-presenced S4.md
  - wiki/concepts/2026-08-04 - one-record-per-key.md
  - wiki/concepts/2026-08-04 - Brainstorm - S4 Enrichment and Packaging.md
  - wiki/concepts/2026-08-04 - Learnings Review.md
  - wiki/concepts/2026-08-04 - Learnings Review 2.md
  - wiki/concepts/2026-08-04 - Top 5 Lessons Export.md
  - wiki/concepts/2026-08-06 - Learnings Review.md
  - wiki/concepts/2026-08-06 - Top 5 Lessons Export.md
  - wiki/concepts/test-deletion-visibility.md
  - wiki/projects/gamebus-presenced.md
  - boards/gamebus-presenced.md
  - wiki/daily/2026-08-04.md
  - wiki/daily/2026-08-06.md
  - Logs/2026-08-04.md
  - Logs/2026-08-06.md
---

## For future Claude

Deep synthesis (2026-08-06) of everything the vault says about how
[[wiki/projects/gamebus-presenced]] gives a game ONE identity on the bus and
merges multi-source evidence into it. The identity story evolved through four
regimes in three days - exact-pid join (S3) → pairwise ancestor-walk (S4d) →
deepest-pid-per-key (S4e/ADR-008) → class-elected game groups (S4f) - and the
vault documents the first three well but the current regime only through
[[wiki/concepts/2026-08-06 - Learnings Review]]; **the authoritative S4f
record is the repo (PLAN.md S4f, src/group.rs), not the vault** (as of
2026-08-06). Read the Contradictions section before trusting any single note.

## What the vault agrees on

1. **Published identity is `pid_<pid>`, underscore, stable across merges.**
   Corroborated by [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]]
   (charset forced live: zbus rejects hyphens, D-Bus has no escaping),
   [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]] (absorption
   keeps it; `merged_<pid>` rejected by owner as id churn),
   [[wiki/logs/2026-08-04 - gamebus-presenced S3]], and
   [[wiki/concepts/2026-08-06 - Learnings Review]] (S4f explicitly preserves
   it - "consumers need no changes"). Confidence: high, 4+ notes.
2. **GameMode is the most trustworthy source, so its identity survives a
   join.** Kernel-tracked registration beats self-reported presence -
   [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]] decision 2,
   [[wiki/logs/2026-08-04 - gamebus-presenced S3]]; the same trust ordering
   appears in [[wiki/decisions/adr-006-rsrpc-crate-dependency]] (ignore
   client-supplied `args.pid`, trust `SO_PEERCRED`) and survives into S4f's
   evidence classes. Field precedence is the complement: Discord wins
   human-facing fields, GameMode owns executable (ADR-007 decision 3;
   [[wiki/concepts/2026-08-04 - Brainstorm - S4 Enrichment and Packaging]]
   anti-goal: naming never overrides a more authoritative source).
3. **One record per merge key.** The single most corroborated claim:
   [[wiki/decisions/adr-008-appid-records-one-record-per-merge-key]],
   [[wiki/concepts/2026-08-04 - one-record-per-key]],
   [[wiki/logs/2026-08-04 - gamebus-presenced S4]] (Gap 5, ~20-record
   explosion), both 2026-08-04 reviews, both Top-5 exports, and S4f keeps it
   (one `GameGroup` per key). Keys are namespaced `steam:<appid>` /
   `lutris:<uuid>` / `umu:<id>` (ADR-008 rule 1; forced by Amnesia having no
   Steam appid, S4 Gap 6). Pairwise merging is rejected everywhere it is
   mentioned; no note defends it.
4. **Never fabricate a join; a documented miss beats a guess.** ADR-007
   decision 8 (exact-pid only, wrapper-tree miss documented), the design-doc
   philosophy quoted there, the brainstorm's instrument-first decision
   (measure join misses before building the walk). Confidence: high.
5. **Records degrade in place and die only at the end.** ADR-007 decision 4
   (lose Discord → PropertiesChanged; lose GameMode → split back; die with
   last partial) is the S3 form; [[wiki/concepts/2026-08-06 - Learnings
   Review]] N2 records the S4f strengthening - die with the last *evidence
   for the game*, exactly one ActivityRemoved per session. The direction of
   travel is consistent: every regime change made removal *harder*, never
   easier.
6. **Identity bookkeeping happens at arrival, not publish** - ADR-007
   decision 5 (`id_index`), S3 dev log (found via a failing test), both
   reviews (active learning #5), extended to per-scan key indexing in S4.

## Contradictions (surfaced, not resolved - see /obsidian-reconcile)

1. **The tiebreaker.**
   [[wiki/decisions/adr-008-appid-records-one-record-per-merge-key]] rule 2
   ("the record stores **one pid - the deepest**; `tree_depth` decides",
   status annotated by the 2026-08-06 reconcile) vs
   [[wiki/concepts/2026-08-06 - Learnings Review]] N1/superseded section
   (class ladder Helper < Plain < IdentifiedWrapper < GameProcess; depth
   never dethrones a live representative; two live failures killed the depth
   rule, 2026-08-06 journal). Same conflict inside
   [[wiki/concepts/2026-08-04 - one-record-per-key]] - its "For future
   Claude" *instructs* the deepest-pid tiebreaker - and in
   [[wiki/concepts/2026-08-04 - Learnings Review 2]] (active #8) and
   [[wiki/concepts/2026-08-04 - Top 5 Lessons Export]] §2. Eight notes carry
   the dead rule (also [[boards/gamebus-presenced]],
   [[wiki/projects/gamebus-presenced]], [[Logs/2026-08-04]] 14:30, and the S4
   dev log Gap 5 - count corrected by the 2026-08-06 reconcile sweep); one
   note records its death. Resolved by the 2026-08-06 reconcile: ADR-008 and all
   carrier notes now carry dated supersession markers.
2. **Does group state persist?** ADR-008 lists "the map is rebuilt each scan
   pass from scratch; there is no iterative state" as a *positive*;
   [[wiki/concepts/2026-08-06 - Learnings Review]] records S4f group state as
   deliberately persistent and sticky (monotone identity, pinned `since`,
   sticky representative). Both are presented as virtues in their respective
   notes.
3. **May the id scheme change?**
   [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]]
   leaves the door open ("the correlator may revisit the scheme when merging
   multi-source records") while ADR-007 slams it (owner rejected `merged_<pid>`
   for id churn) and S4f re-affirms `pid_<pid>`. Mild tension, resolved in
   practice by "revisit ≠ change", but the notes point different directions.

## Stale claims (dated facts to re-verify before trusting)

- ADR-007 consequence: "a record whose process later dies lingers until the
  next restart (accepted)" - stale since S4d/S4e (as of 2026-08-04 evening):
  `drop_partial` was fixed so cache-adopted records CAN be removed
  ([[wiki/logs/2026-08-04 - gamebus-presenced S4]] S4d bug 2), the cache now
  excludes Steam-only records (Gap 9), and the S4f sweep prunes dead members.
- ADR-007's umu research (GPL-3.0, "by no means a complete database",
  https://github.com/Open-Wine-Components/umu-launcher - as of 2026-08-04):
  external facts, re-verify licence and DB scope before relying on them.
- Every merge-behavior claim in notes dated 2026-08-04 describes a superseded
  regime unless the 2026-08-06 review confirms it - the vault's newest
  decision record (ADR-008, 18:21) predates the current implementation by two
  days.

## Coverage gaps (the vault does not answer)

1. **No ADR for S4f.** The decision chain ends at ADR-008; the current
   regime's authority is repo-only (PLAN.md S4f; the game-groups spec's
   decision rules; `src/group.rs`). An ADR-009/010 is an open action in
   [[wiki/concepts/2026-08-06 - Learnings Review]], deferred by owner.
2. **Discord × groups.** ADR-007's absorption is exact-pid; the vault says
   nothing about what happens when a group's representative migrates while a
   `discord_<pid>` partial is joined to it (the repo spec pins the rep while
   Discord is attached, and explicitly defers Discord merge-key stamping -
   the wrapper-tree Discord join-miss REMAINS a miss post-S4f; repo-only).
3. **Keyless records.** Pids with no merge key follow the untouched ungrouped
   path - including the sub-second publish/remove flapping of keyless helpers
   observed live on 2026-08-06 (session evidence, recorded nowhere in the
   vault). No note states this residual behavior or its acceptability.
4. **Key fragmentation.** One process carrying both `SteamAppId` and
   `LUTRIS_GAME_UUID` produces two possible keys; S4f added telemetry only
   (a `warn!`), decision deferred pending live data - repo-only, no vault
   record.
5. **The v2 identity direction.** Key-derived stable ids (`game_<key>`) are
   named as the documented v2 direction in the repo spec, gated on verifying
   how epaper-hubd reads `ProcessId` - the vault's ADR-003 versioning
   strategy would govern this, but no note connects them.

## Related

- [[wiki/decisions/adr-003-d-bus-service-naming]] (versioning strategy the v2
  direction would invoke)
- [[wiki/concepts/test-deletion-visibility]] (born from ADR-007's
  `remove_by_source` removal)
- [[wiki/concepts/squash-merge-branch-pointer]] (how S5/S4f landed in git)
