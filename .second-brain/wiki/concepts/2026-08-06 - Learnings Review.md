---
date: 2026-08-06
type: learnings-review
tags: [learnings-review, thinking]
period-days: 30
ai-first: true
---

## For future Claude

Third learnings review, run 2026-08-06 after the S5 setup-tool session and the
S4f enricher-reliability fix. All 26 vault notes were scanned (8 ADRs, 5 dev
logs, 6 concept notes, project/daily/board/ops-log/entity/research), plus the
repo record for 2026-08-05/06 (PLAN.md S5 + S4f sections, git log 62dfd21 →
18cb92b) — because **the vault has no notes for S5 or S4f yet**; those events
are repo-recorded only (as of 2026-08-06). One prior classification is
overturned: ADR-008's deepest-pid rule died against two live failures. One new
promotion candidate reached the 4-occurrence bar.

## Corrections to the gather pass

Both gather agents claimed promoted rules were missing from `[[_CLAUDE.md]]`.
False — §0.5 (verify live state) and §0.6 (real-client testing) both exist,
verified by reading the file this session. Recorded here so a future review
does not "re-promote" them.

## Active Learnings (still applies)

Reviews 1 and 2 ([[wiki/concepts/2026-08-04 - Learnings Review]],
[[wiki/concepts/2026-08-04 - Learnings Review 2]]) classified nine active
learnings; none went stale. Reinforcements observed 2026-08-06:

### 1. Verify live state before trusting docs or assumptions (`_CLAUDE.md` §0.5)
Three new reinforcements (as of 2026-08-06, session + repo):
- A *fix* for umask-dependent directory modes used `DirBuilder::mode(0o755)` —
  which `mkdir(2)` masks with the umask exactly like the code it replaced.
  Caught only because a reviewer's claim was **measured** (0700 under
  `umask 077`) instead of argued. The wrong fix had passed review by reading.
- `/usr/local` as the system install prefix was adopted only after verifying
  live that `/usr/local/lib/systemd/user` is in `systemd-analyze --user
  unit-paths` and `/usr/local/share` in `XDG_DATA_DIRS` on this machine.
- Both S4f failure diagnoses started from the daemon's own journal, not from
  memory of the code.

### 2. Integration-test against the real thing (`_CLAUDE.md` §0.6)
Reinforced (as of 2026-08-06): S4f was accepted against a **real Brotato
relaunch** observed live on the bus (wrapper → launch process → game binary,
publish-first at every step; PLAN.md S4f). Inverse case in the same session:
the umask bug shipped precisely because no test ran under a non-default umask
— the environment variance you claim to handle must appear in a test.

### 3. One record per key — pattern holds, tiebreaker overturned
[[wiki/concepts/2026-08-04 - one-record-per-key]]'s abstract pattern
(`HashMap<key, representative>` over pairwise merge) survived its third
occurrence: S4f keeps exactly one `GameGroup` per merge key. Its stated
tiebreaker ("deepest pid, `tree_depth` decides") is **superseded** — see
below. Promotion still deferred: all three occurrences are in
[[wiki/projects/gamebus-presenced]]; Review 2's bar of a cross-domain
recurrence stands.

### 4. Let the test environment's reality shape assertions — sharpened
Strongest reinforcement of the period. A gamebus-presenced *installed and
enabled by the owner* took `org.gamebus.Presence.v1` on the live session bus,
so every integration test's daemon died with `NameTaken` while assertions ran
against somebody else's process. Two-step correction (repo, 2026-08-06):
first verify the bus-name *owner pid* is your own daemon; then remove the
shared resource entirely — `tests/common/mod.rs` gives every test a **private
`dbus-daemon --session`**, which also activates its own empty-state
`gamemoded`. Sharpened form: *isolate the identity (bus name), not just the
filesystem (`XDG_RUNTIME_DIR`)* — the S4 test-isolation fixes (Issues 4/5/12)
had isolated only paths.

### 5-9. No new evidence, still recent
Index-at-arrival, unified event channel, reuse + licence hygiene,
detectable.json path-suffix matching, and flag-test-deletions
([[wiki/concepts/test-deletion-visibility]]) — unchanged. Note the S4f
implementation spec *pre-authorized* exactly one test edit and demanded a stop
if any other existing test needed touching; the implementer complied. That is
OP1 working as a standing constraint rather than a correction (5th
occurrence, counting style).

## New Learnings of the period (not yet in any vault note — repo/session only)

### N1. Depth is not identity — elect representatives by evidence class
"Deepest pid wins" assumed deeper = closer to the game. On Steam/Proton/umu
launches the deepest pids are short-lived runtime helpers
(`i386-linux-gnu-inspect-library`, `pv-adverb`, `srt-bwrap`), and
`libgamemodeauto` registers *all* of them. Two user-visible failures
(2026-08-06 journal): Brotato's record migrated to a helper that exited 0.3ms
later and died with it while GameMode still held live ancestors; Amnesia's
**identified** record was replaced twice by unidentified helpers, ending with
an empty bus mid-game. Fix (PLAN.md S4f, `src/group.rs`): class ladder
Helper < Plain < IdentifiedWrapper < GameProcess; only a strictly greater
class dethrones a live representative; depth breaks ties only within a class.
Confidence: high (live-verified same day).

### N2. Records die with the last evidence for the game, not with a chosen pid
Deferred migration: a dead representative holds the published record while any
group member still carries evidence; the tick re-elects. Removal order emits
the Steam partial first so the record degrades in place — exactly one
`ActivityRemoved` per session. This restated design-doc rule 3 ("dies with the
last **evidence**", was "last source") — the invariant behind the owner's
"gamebus simply cannot break" requirement.

### N3. Adversarial verification catches the author's own wrong fixes → see Promotion
### N4. Two-layer change communication (owner preference, stated 2026-08-06)
Owner, verbatim intent: "the most important thing [is] to have the user
actually know what will be done: stating the clear change for powerusers,
keeping it simple for the regulars." Implemented in `gamebus-setup`: plain
summary + scope sentence by default, exact command list one keypress away,
both generated from the same `Plan` so they cannot drift. Candidate for an
owner-preference concept note in the style of
[[wiki/concepts/test-deletion-visibility]].

### N5. Squash-merges must keep a pointer to the feature branch (owner preference)
Stated 2026-08-06 after the S5 landing was redone: a plain `git merge
--squash` produced a single-parent commit with no graph connection to
`feat/s5-setup-tui`; the owner required the second parent. Rule: prefer
`--no-ff` (branch as real second parent); if a true squash is unavoidable,
the message names the branch + head SHA and the branch is kept. Recorded as
[[wiki/concepts/squash-merge-branch-pointer]] — same traceability-over-
tidiness shape as [[wiki/concepts/test-deletion-visibility]].

### N6. Hand-run installers write `/usr/local`, never `/usr`
`/usr` is package-manager territory: no ownership tracking, and read-only on
image-based distros (SteamOS/Bazzite class). Came out of the S5 strategic-fit
review; forced one daemon change (naming search walks `XDG_DATA_DIRS`).

## Stale Learnings

None possible — the vault is two days old; the 6-month criterion first bites
2027-02.

## Superseded Learnings (already replaced)

- **ADR-008 rule 2** ("the record stores one pid — the deepest; `tree_depth`
  decides") → **S4f class-based sticky election + deferred migration**
  (PLAN.md S4f, 2026-08-06; commit 18cb92b). Also superseded from the same
  ADR: "map rebuilt each scan, no iterative state" → group state is now
  persistent, with monotone identity and pinned `since`. **Everything else in
  [[wiki/decisions/adr-008-appid-records-one-record-per-merge-key]] holds**
  (merge-key derivation, one-record-per-key, no pairwise walks).
  Vault hygiene: ADR-008 carries **no annotation yet**; the dead tiebreaker is
  also asserted in Review 2 (active #8, Top-5 #2),
  [[wiki/concepts/2026-08-04 - Top 5 Lessons Export]] §2, and
  [[wiki/concepts/2026-08-04 - one-record-per-key]].
- The seven supersessions from Reviews 1-2 stand unchanged.

## Promotion Candidates (3+ occurrences)

### P1. Adversarial verification before handover — 4 occurrences, ELIGIBLE
Every review event of 2026-08-05/06 produced confirmed, consequential findings
the author and the test suite had both missed:
1. Fresh-context reviewer of the S5 branch → 4 blocking privilege-path bugs
   (escalation not target-gated, terminal-handover race, cancel-still-acts,
   doc self-contradiction).
2. Strategic-fit reviewer → verdict *revise*: `/usr` prefix wrong, STAGED
   honesty enforced, a false doc claim caught.
3. Adversarial find→refute workflow → the author's own umask fix proven wrong
   by measurement; a printed `sudo` hint that reintroduced a fixed bug; a test
   that would really install when run as root. Refutation killed
   plausible-but-wrong findings before they cost time.
4. S4f audit trio → wrapper-cmdline classification bug that would have blocked
   the anchor upgrade forever; a teardown flash contradicting the spec's own
   no-flapping promise.
Same 4-occurrence, same-project bar that promoted §0.6. Suggested wording for
`_CLAUDE.md` §0.7:

> **Adversarially verify before handing over.** Nontrivial changes get a
> fresh-context review whose job is to refute, not confirm: reviewers trace
> failure scenarios through the actual code, and skeptics try to kill each
> finding before it reaches the owner. A fix's *mechanism* must be verified —
> measured, traced, or reproduced — never just its intention; the reviewer
> panel caught a wrong fix for a confirmed bug (umask-masked `mkdir` modes)
> that reading alone had passed. Findings without a concrete failure scenario
> are noise.

## Top 5 Lessons of the Period

1. **Depth is not identity — elect by evidence class** (N1; fixed two live
   failures the same day they were reported).
2. **Adversarial verification catches your own wrong fixes** (P1; 4/4 review
   events found real bugs, incl. a wrong fix for a confirmed bug).
3. **Records die with evidence, not with a pid** (N2; the reliability
   invariant behind "cannot break").
4. **Isolate the identity in tests, not just the filesystem** (Active #4
   sharpened; the suite survived a real daemon owning the real bus).
5. **Verify live state — measure, don't argue** (§0.5, 3 new reinforcements;
   the umask measurement is the period's cleanest example).

## Delta vs Review 2

| | Review 2 (08-04) | This review (08-06) |
|---|---|---|
| Active | 9 | 9 (+5 new, pending vault capture) |
| Stale | 0 | 0 |
| Superseded | 7 | 8 (+ADR-008 rule 2) |
| Promotion candidates | 1 promoted (§0.6) | 1 eligible (P1) |
| Unit+integration tests | 61+6 | 134+16 (as of 2026-08-06, `cargo test`) |
| ADRs | 8 | 8 (ADR-008 needs annotation; S4f/S5 ADRs missing) |

## Open vault-hygiene actions (require owner confirmation)

1. Promote P1 to `_CLAUDE.md` §0.7 (wording above).
2. Annotate ADR-008 with a supersession note (ADR-004 style), pointing at
   PLAN.md S4f until an ADR-009 exists.
3. Write the missing records: S5 dev log + setup-tool ADR, S4f dev log +
   game-groups ADR, owner-preference note for N4. The vault currently has no
   record of either 2026-08-06 effort.
4. Correct the dead tiebreaker claim in
   [[wiki/concepts/2026-08-04 - one-record-per-key]] (annotate, don't rewrite).
