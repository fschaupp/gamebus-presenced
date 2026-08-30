---
date: 2026-08-07
type: concept
topic: persistence pattern - shared state files without locks
tags: [concept, persistence, concurrency, pattern, gamebus-presenced]
related-projects: [gamebus-presenced]
sources: [gamebus-presenced src/umu_report.rs (branch feat/s9-umu-miss-report, commit ca97250, 2026-08-07)]
confidence: high
ai-first: true
---

## For future Claude

A reusable persistence pattern extracted from
[[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]]
(gamebus-presenced, 2026-08-07): when two processes share a JSON state file
and each owns a **disjoint set of fields per record**, merge-on-persist turns
a session-long lost-update window into a milliseconds one without any locks.
Found by an adversarial review (vault rule §0.7 - the strategic-fit reviewer
traced the failure scenario the author and tests had missed).

## The pattern

Two long-lived writers, one file, each writer owning half of every record
(in the source case: a daemon owns the *resolution* half, a setup tool owns
the *annotation* half). Each writer loads once and keeps an in-memory map.

**On every persist, before flattening memory onto disk:**

1. **Re-read the file** and parse it.
2. **Adopt the other writer's half** of every shared record from disk; keep
   your own half from memory. (Each writer knows which side it is - a
   constructor flag suffices.)
3. **Union unknown records whole** - a record only the disk knows was
   written by the other side since you loaded; keep it. Nobody deletes.
4. Write atomically (temp file + rename).

The lost-update window shrinks from *the whole session* (load at startup,
clobber at first write) to *the gap between the re-read and the rename*.
When the writers are, say, a human-run CLI and a rare event in a daemon,
that residual race is acceptable by construction - and both sides are
retry-shaped, so a lost update is restored by the next event.

## Two corollaries you cannot skip

- **Deletion must become a flag owned by one side.** Removing a key does not
  survive: step 3's union resurrects it from the other writer's copy (and in
  the source case, the next game launch re-records it anyway). User
  "deletion" is therefore a `dismissed` date on the annotation half -
  filtered from views and exports, reversible, and immune to resurrection.
  The same holds for user *corrections*: they must live on the half owned by
  the side accepting the edit (`store_override`, read via an
  `effective_store()` accessor), or the other writer's next persist reverts
  them.
- **A parse failure must drop the write path.** If the file exists but does
  not parse, record a load error and refuse to write (in the source: `path`
  is set to `None`, so no write *can* happen). Otherwise the next persist
  flattens an empty map over the only surviving copy of the data. Surface
  the error - silently showing "empty" over a corrupt file hides data loss.

## Failure modes this prevents

- **Session-long mutual clobbering** - the originating bug: any daemon write
  after the setup tool's `--verify` erased all verification annotations, and
  any setup save erased misses recorded since it loaded.
- **Resurrection of deleted records** by the union step or by re-observation.
- **User edits silently reverted** by the other writer's stale copy.
- **Corruption amplification** - one bad parse turning into total data loss
  on the next write.

## When to reach for it (and when not)

Fits when: two (or few) cooperating processes, low write frequency, records
with a natural ownership split per field, and a strong preference for no
lock files / no daemons-coordinating-daemons. Does **not** fit when fields
are genuinely co-edited by both sides (no disjoint halves - you need real
merging or locking), or when writes are frequent enough that the
milliseconds window matters (use flock or a single-writer broker).

## Provenance

- Origin: [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]],
  [[wiki/projects/gamebus-presenced]] `src/umu_report.rs`
  (`UmuReport::merge_from_disk`), branch `feat/s9-umu-miss-report`, commit
  `ca97250`, 2026-08-07. Session log:
  [[wiki/logs/2026-08-07 - gamebus-presenced S9b umu Contribution Pipeline]].
- Found by the strategic-fit reviewer of the adversarial review workflow
  (verdict "revise"), per vault rule §0.7 - a concrete traced failure
  scenario, not a style nit.
- Verified by an interleaved two-writer test
  (`umu_report::tests::the_two_writers_never_clobber_each_others_half`) and
  a corrupt-file test (`a_corrupt_stash_is_reported_and_never_overwritten`),
  both at the branch tip as of 2026-08-07.
