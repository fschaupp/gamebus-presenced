---
type: learnings-review
date: 2026-08-23
scope: recent (last 30 days = the vault's whole life, 2026-08-03..2026-08-23)
tags: [learnings-review, gamebus-presenced, gamebus-gamedb]
related-projects: [gamebus-presenced]
ai-first: true
counts:
  active: 27
  stale: 0
  superseded: 16
  promotion-candidates: 2
---

# Learnings Review - 2026-08-23

## For future Claude

Fourth learnings review (prior: two on 2026-08-04, one on 2026-08-06 - read
those for the S0-S4f record; this one covers 2026-08-07..2026-08-23 fully and
re-verifies the carried-forward actives). The vault's promotion bar is
**4 occurrences** - both `_CLAUDE.md` §0.6 and §0.7 were promoted at exactly
that count, and "one record per key" sits deferred at 3 pending a cross-domain
recurrence. Sections 0.8-0.10 came from direct owner rules, not reviews.
Two candidates meet the bar this time; one of them has met it since
2026-08-04 and was never proposed - the process itself missed it.

## Active Learnings (27 recurring; all reinforced within 90 days)

The full occurrence-by-occurrence record is condensed here; every count was
gathered file-by-file from the 6 dailies and 11 dev logs on 2026-08-23.

| Lesson | Count | Latest reinforcement |
|---|---|---|
| Only a live run against real data exposes the real failure mode | 12 | [[wiki/logs/2026-08-23 - gamebus-gamedb Published and the Export Loop]] (wrapper exes, redirect tag, filemode) |
| Wrapper/helper executables are never the game; filter every leak path | 6 | same (page-exe poisoning) |
| Full gate (fmt, clippy -D warnings, all tests) before every landing | 6 | same |
| Real-client / captured-wire testing, never protocol mocks (§0.6) | 5 | [[wiki/logs/2026-08-07 - gamebus-presenced S9b umu Contribution Pipeline]] |
| `--no-ff` merge, branch kept ([[squash-merge-branch-pointer]]) | 5 | 2026-08-08 |
| Owner merges and pushes; agent never does (§0.10 since 2026-08-23) | 5 | [[wiki/daily/2026-08-23]] |
| Test isolation: own daemon, prior caches, real clients break tests | 5 | tracked since Review 1 as "environment reality shapes assertions" - see Promotion |
| Daemon stays network-free by construction | 5 | [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]] |
| Sentinel values (`0`, `default`, `umu-0`) are never identity | 4 | 2026-08-06 (S7/S8) |
| Adversarial review finds the worst bug (§0.7) | 4+ | uncounted reinforcements in [[Two-Writer State Files with Owned Halves]] and [[Deterministic History Re-Composition with Tree Filters]] |
| Two-writer state needs owned halves + merge-on-persist (ADR-009) | 4 | fix annotation joined the half 2026-08-22 |
| Negative results claim only what the input supports | 3 | firm-vs-guessed split, 2026-08-22 |
| Elected identity, not the event stream, feeds downstream state | 3 | [[Elected State Beats the Event Stream]] |
| Loose/substring matching is not identity - demote to advisory | 3 | 2026-08-08 (case-sensitivity differs per consumer) |
| Checklists state only what a human verifies | 3 | 2026-08-08 |
| Owner's own framing is often the right design - take it | 3 | file-per-game shape, 2026-08-22 |
| Never commit while exploring (§0.9) / docs-free feature commits / per-commit standalone compile | 3 each | 2026-08-22 |
| Dash + internal-ref scrubbing before finishing (.txt escapes filters) | 3 | 2026-08-08 |
| Stale checkpoint preambles: refresh headers when later work lands in the same note | 3 | project-note preamble un-staled 2026-08-23 |
| Vault capture debt carries forward for days (S5/S4f logs still owed) | 3 | still open on [[boards/gamebus-presenced]] |
| One record per key (promotion still deferred: all occurrences one project) | 3 | 2026-08-06 |
| `core.filemode=false` swallows exec bits (`git update-index --chmod=+x`) | 2 | 2026-08-23 |
| Read the upstream's opening paragraph - the gate may be there | 2 | cost: PR #151 |
| Upstream's existing content is not precedent - measure the corpus | 2 | 240/1,202 rows, 2026-08-22 |
| Kernel-verified identity (`SO_PEERCRED`) over client claims | 2 | 2026-08-06 |
| Union-semantics overlay layering (add, never remove) | 2 | proposed again for gamedb 2026-08-22 |

New this period, single-occurrence, watch for recurrence: pin tools by
digest and prove byte-determinism across machines (2026-08-23); enhancement
over hold-back with format-preserving edits (toml_edit, 2026-08-23); date
tags for data releases (2026-08-23); prove a new pin on real data before
moving it (2026-08-23); agent briefs need the same fact-checking as code -
a briefed agent caught my wrong GOG id (2026-08-23); scratch-HOME copies for
tooling runs against real state (2026-08-22); GitHub double-redirect loses
the release tag (2026-08-23); TOML key order is semantic - a root key after
a table header changes meaning (2026-08-23).

## Stale Learnings

**None possible.** The vault is 20 days old; the 6-months-without-
reinforcement criterion cannot fire before 2027-02 (all three prior reviews
said the same). Distinct from learning-staleness, these **stale claims**
flagged by [[2026-08-06 - synthesis - record-identity-and-merging]] remain
un-refreshed: ADR-007's "a record whose process later dies lingers until
restart" (dead since S4d/S4e) and its umu licence/scope research (now
superseded by ADR-010's survey - effectively resolved, worth an annotation).

## Superseded Learnings (16 positions, each with both sides on record)

- **ADR-008 rule 2** (deepest pid, rebuilt each scan) → S4f evidence-class
  election with sticky persistent groups. Killed by Brotato's 0.3 ms helper
  and Amnesia's `i386-linux-gnu-inspect-library`.
- **PLAN.md "stash written only by the daemon"** → ADR-009 two writers.
- **Stash any member identity** → stash only the group's elected identity
  (S9c, the Project Hospital / "Spellcraft" mislabel).
- **ADR-007's umu-database-as-secondary-source disposition** → ADR-010's own
  repo for the delta.
- **Review 2's claim that a test deletion carried visible rationale** →
  retracted by `git log -S` (its History block, 2026-08-06). A review's own
  claims are subject to the same verification as code.
- **Ten reversals inside ADR-010 itself**, all reasons recorded: numeric-ids-
  reserved-for-Steam; filename-derived keys; authority-keying without the
  letter rule; year directories / mandatory `year`; recompute-on-change ids
  (→ frozen on assignment); id-only uniqueness (→ every identifier a page
  carries); SQLite as the settled client format (→ owner's Parquet intent,
  SQLite interim); `identities.csv` as a sixth artifact (→ deferred, the
  scope gate lives elsewhere); split-on-second-consumer (→ owner split at
  settle-time); GPL / hand-rolled ODbL variant (→ ODbL unmodified).

The one-day reversal cycle inside ADR-010 is itself the period's strongest
argument for the review-format/lookup-format split: every reversal was cheap
because the TOML pages stayed the only source of truth.

## Promotion Candidates

Bar: 4 occurrences (the §0.6/§0.7 precedent). Two meet it; suggested wording
included; **both need the owner's confirmation before touching `_CLAUDE.md`**.

**P1 - Run it against the real thing before calling it done. 12 occurrences,
2026-08-04..2026-08-23.** Distinct from §0.5 (read reality before acting) and
§0.6 (tests use real wire): this is about executing the *built artifact*
against real data/games before declaring it works. Every worst defect of the
period was invisible to a green suite: the wrapper-exe alias poisoning, the
lost release tag, the S9b substring-match bug, the honesty split, S4f's two
live kills. Suggested `_CLAUDE.md` §0.11:

> ## Section 0.11 - Run It Against the Real Thing Before Calling It Done
> A green gate earns "the tests pass", never "it works". Before work is
> presented as done, run the built thing against real data on this machine -
> the real stash (a scratch-HOME copy), a live game, the published release -
> and read what actually comes out. Twelve recorded occurrences 2026-08-04
> through 2026-08-23 of the live run finding what the suite could not,
> including every worst defect of the gamedb period.

**P2 - Amend §0.6 with identity isolation. 5 occurrences, met the bar since
2026-08-04 and never proposed** (Reviews 1-3 tracked it as "let the test
environment's reality shape assertions", Review 3 called it the period's
strongest reinforcement, nobody promoted it - the review process itself
missed one). Not a new section; one paragraph appended to §0.6:

> Integration tests isolate the *identity*, not just the filesystem: own bus
> name per test where possible, isolated `XDG_RUNTIME_DIR`/`HOME`, and skip
> honestly when the developer's real daemon, Discord, or a preloaded
> libgamemodeauto owns the environment. Five recorded failures 2026-08-04
> onward came from the developer's own machine leaking into tests.

**Routed to concept notes instead of `_CLAUDE.md`** (domain knowledge, not
process): *Wrappers are never the game* (6 occurrences - the single most
recurrent domain lesson, spanning naming, grouping, stashing, and now gamedb
pages; deserves its own concept note tying `is_wrapper_executable`,
shared-helpers, and the `.exe` filter together) and *identity must be exact:
no sentinels, no substring matches* (4 + 3 occurrences, two sides of one
lesson). **Still deferred:** one-record-per-key (3, cross-domain bar stands).

## Top 5 Lessons of the Period (2026-08-07..2026-08-23)

1. **Run it against the real thing** - 12 occurrences, and the period's
   entire defect record. (→ P1)
2. **Read the upstream's opening paragraph** - 2 occurrences, highest
   consequence-per-occurrence: a built, submitted PR that does not qualify,
   and a week of rework that ended in gamebus-gamedb existing.
3. **Separate the review format from the lookup format** - made ten design
   reversals cost nothing ([[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]]).
4. **A negative result claims only what its input supports** - the honesty
   family (firm-vs-guessed ids, human-only checkboxes, advisory-only fuzzy
   matches) is becoming this project's signature.
5. **Isolate the test environment's identity** - five failures from the
   developer's own machine, promotion-eligible since day one. (→ P2)

## Related

- Prior reviews: [[2026-08-04 - Learnings Review]], [[2026-08-04 - Learnings Review 2]], [[2026-08-06 - Learnings Review]]
- [[wiki/reviews/2026-08-23 - Weekly Review]] (same day, narrative view)
- [[wiki/projects/gamebus-presenced]] · [[boards/gamebus-presenced]]
