---
type: adr
date: 2026-08-07
tags: [decision, adr, gamebus-presenced, umu, persistence]
ai-first: true
status: Accepted, implemented (feat/s9-umu-miss-report, merge landed in ca97250, 2026-08-07)
---

# ADR-009: umu-miss Stash - Two Writers, Owned Halves, Merge on Every Persist

## For future Claude

The umu-miss stash file (`$XDG_DATA_HOME/gamebus-presenced/umu-misses.json`)
is written by TWO independent processes: the network-free daemon and the
`gamebus-setup` tool. Each owns a disjoint half of every entry, and every
persist re-reads the file and adopts the *other* writer's half before
flattening - otherwise either writer's stale in-memory map silently erases
the other's work. Deletion is a flag (`dismissed`) on the annotation half,
never a key removal, because the merge (and the next launch) would resurrect
a removed key. A file that fails to parse drops the write path entirely.
Source of truth: `src/umu_report.rs` (`UmuReport`, `merge_from_disk`) on
branch `feat/s9-umu-miss-report`.

## Context

S9 turns umu-database misses (`GAMEID=umu-0`, `UMU_ID=umu-default`) into a
contribution pipeline: the daemon stashes each miss with whatever it resolved
(title, store guess, codename, confidence, executable), and `gamebus-setup`
lets the user verify entries against the database, draft collision-checked
umu ids, correct store guesses, and export a submission-shaped CSV. Both
programs write the same stash file.

The original implementation loaded the file once at startup and persisted the
whole in-memory map on change. The **strategic-fit reviewer** of the
adversarial review workflow (vault rule §0.7; verdict "revise") traced the
failure scenario: any umu-missed launch after `gamebus-setup umu-misses
--verify` made the daemon persist its pre-verify copy, **silently erasing all
verification annotations** - and the reverse (a `--verify` save during a
session) erased fresh misses. A session-long lost-update window, invisible to
both the author and the test suite.

An earlier PLAN.md claim that the stash is "written only by the daemon" was
retired by this decision; PLAN.md §S9/§S9b now describes both writers (as of
2026-08-07, branch tip).

## Decision

The stash has two writers, and **each writer owns a disjoint half of every
entry** (`Miss` in `src/umu_report.rs`):

- **Daemon - resolution half:** `title`, `store`, `codename`, `umu_id`,
  `title_source`, `confidence`, `executable`, `first_seen`, `last_seen`.
- **gamebus-setup - annotation half:** `verification`, `drafted_id`,
  `possible_pr`, `store_override`, `dismissed`.
  (`store_override` and `dismissed` joined this half in follow-up commits
  `30b9b42` and `6589e18` on the same branch.)

**Rules** (all implemented in `UmuReport`):

1. **Merge on every persist.** `persist()` calls `merge_from_disk()` before
   writing: re-read the file, adopt the OTHER writer's half of each shared
   entry from disk, keep your own half from memory, then flatten. The
   `annotator` flag (set by `load_for_annotations()` in the setup tool)
   decides which half is "yours".
2. **Union unknown entries.** Entries only the disk knows are inserted
   whole - nobody ever deletes a miss.
3. **Deletion is a flag.** User "deletion" is `dismissed` (a date) on the
   annotation half - not wanted, dropped from exports, parked at the bottom,
   reversible. A removed key would be resurrected by rule 2 *and* re-recorded
   by the next launch anyway; a flag survives both.
4. **A parse failure drops the write path.** A file that exists but fails to
   parse sets `load_error` and leaves `path: None` - no write may ever
   flatten a file that could not be read; accumulated knowledge beats a
   working session. Callers surface `load_error()` instead of showing
   "no misses" over a corrupt file.
5. Writes stay atomic (temp file + rename), and the user's store correction
   lives in `store_override` (annotation half) while `store` stays the
   daemon's guess - downstream reads `Miss::effective_store()` - so a daemon
   write can never revert a user correction.

## Consequences

**Positive:**

- The lost-update window shrinks from **session-long to the read-write gap**
  (milliseconds), with no locks. The writers are a human-run CLI and a rare
  launch event; they do not race in practice.
- Verification annotations survive daemon launches; fresh misses survive
  setup saves. Covered by the interleaved two-writer test
  `umu_report::tests::the_two_writers_never_clobber_each_others_half`
  (plus `a_corrupt_stash_is_reported_and_never_overwritten` for rule 4).
- The daemon stays network-free; the whole verification/database half lives
  in `gamebus-setup`.

**Negative / obligations:**

- **User corrections and judgments must always be annotation-half fields**
  (`store_override`, `dismissed` are the precedents). Putting a user edit on
  the resolution half means the next daemon persist reverts it. Every future
  user-editable field inherits this constraint.
- A true race inside the read-write gap can still lose one half's update -
  accepted: the window is milliseconds and both writers are retry-shaped
  (the next launch or the next `--verify` restores the data).
- Every persist pays a re-read and re-parse of the file - negligible at
  stash scale.

## Alternatives

1. **Setup-owned sidecar file** (annotations in a second file keyed by entry
   key) - rejected: two files to keep consistent for one logical record, and
   every reader must join them.
2. **File locking** (flock around load-modify-write) - rejected: shrinking
   the window via merge suffices for a human-run CLI vs. a rare launch
   event; locking adds a blocking failure mode for no practical gain.
3. **Keep single-writer semantics** (last writer wins) - rejected: this is
   the bug the review found; either writer silently destroys the other's
   session of work.

## Amendments (2026-08-08)

Two changes from S9c (branch `fix/umu-export-conformance`, merged to master
via `fb8a702`, 2026-08-08):

1. **The annotation half's user-correction family is now three fields:**
   `store_override` (the original precedent) has been joined by
   `codename_override` and `title_override`. Each is read through an
   `effective_*()` accessor (`effective_store()`, `effective_codename()`,
   `effective_title()`), threaded through verify, exports, and matching -
   the "user corrections are annotation-half fields, read via accessors"
   obligation from the Consequences section held exactly as written.

2. **The resolution half's write hook changed.** The daemon used to stash
   *any member identity reaching a umu-missed group* (refresh on same
   confidence); it now stashes only **the group's ELECTED identity after
   routing**. Member claims that lose the class election never reach the
   stash, so the stash inherits the election's monotonicity instead of
   event-arrival order. Motivated by the Project Hospital/"Spellcraft"
   mislabel (2026-08-08): a Unity crash handler's same-confidence,
   different-title member claim flip-flopped the stash while the published
   record - which reads the election - stayed correct. The generalized
   lesson lives in
   [[wiki/concepts/Elected State Beats the Event Stream]]; session detail in
   [[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]].

## References

- [[wiki/projects/gamebus-presenced]]
- [[wiki/logs/2026-08-07 - gamebus-presenced S9b umu Contribution Pipeline]]
- [[wiki/concepts/Two-Writer State Files with Owned Halves]] - the
  generalized pattern
- Repo: `src/umu_report.rs` (`UmuReport` docs, `merge_from_disk`), branch
  `feat/s9-umu-miss-report`, commit `ca97250` (2026-08-07); PLAN.md §S9/§S9b
- Vault rule §0.7 (Adversarially Verify Before Handing Over) - the
  strategic-fit reviewer found this

## Amendment (2026-08-30, identity misses)

The stash widened from umu misses to identity misses (design:
[[wiki/concepts/2026-08-24 - Identity Misses Design]], branch
feat/identity-misses). Half assignments that landed:

- Resolution half grew `launcher`, `launcher_name`, `launcher_dir`,
  `codename_source` (`lutris-config` | `heroic-env`), `runner`; `umu_id`
  stays always-serialised, empty meaning "not a umu miss".
- Annotation half grew `umu_promoted` (the owner's opt-in umu promotion) and
  `codename_override_source` (`lutris-library`, cleared by a hand edit).
- **A structural consequence found during integration**: the setup tool
  cannot durably write ANY resolution-half field - its own
  `merge_from_disk` re-adopts that whole half from disk before every
  persist, so the write is flattened by its own save, before the daemon is
  even involved. Every tool-supplied fact must land on the annotation half;
  `codename_override` + `codename_override_source` is the pattern.

