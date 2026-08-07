---
date: 2026-08-07
type: concept
topic: git history rewriting - publish curated history with per-commit invariants
tags: [concept, git, history-rewriting, publishing, pattern, gamebus-presenced]
related-projects: [gamebus-presenced]
sources: [gamebus-presenced public-branch re-composition (2026-08-07)]
confidence: high
ai-first: true
---

## For future Claude

A technique generalized from the 2026-08-07 [[wiki/projects/gamebus-presenced]]
public-branch work (dev log:
[[wiki/logs/2026-08-07 - gamebus-presenced Public Branch Re-Composition]]):
to publish a curated git history from an internal branch, run a deterministic,
idempotent cleanup script over **every commit's tree** via
`git filter-branch --tree-filter`, instead of hand-editing per commit. Trees,
not diffs, means zero merge conflicts by construction; the invariant ("no
internal vocabulary anywhere in history") is then verified per commit, not
just at the tip.

## The problem

Publishing a curated history from an internal branch requires removing
internal vocabulary — phase tags, private-spec references, diary dates — from
**every** commit, not just the tip. Otherwise early commits introduce text
that later "cleanup" commits delete, which both leaks the internals (they are
still in history) and adds noise (the deletions show up as diffs).
Hand-editing each commit invites conflicts and drift between commits.

## The technique (three separable passes)

1. **Deterministic cleanup script.** Exact-pair replacements — file-scoped,
   replace-or-die so a silent no-op cannot hide — for prose that needs
   judgment, plus generic context-free regexes for patterned references
   (parenthetical tags, leading comment tags, section refs). Tune it against
   the final tree until `pristine + script == hand-validated tree`, verified
   byte-for-byte (run twice, compare diffs). The script must be idempotent
   and safe on **any** historical tree: pairs no-op when their text is
   absent; regexes are context-free.
2. **Structure rebuild.** A cherry-pick / `git commit-tree` chain for
   squashes, message rewrites, and author-date preservation.
   `git commit-tree` builds commits from explicit snapshot trees, so there
   are zero merge conflicts by construction.
3. **`git filter-branch --tree-filter <script>`.** Runs the cleanup on every
   commit's tree independently (trees, not diffs, so no conflicts ever).
   Diffs between consecutive cleaned trees come out cleaned automatically.

## Key insights

- **Exact pairs written against the final tree miss intermediate-only text**
  — text that was edited between its introduction and the tip.
  Countermeasure: after filtering, grep every historical tree for the banned
  patterns (`for c in $(git rev-list branch); do git grep -E '<patterns>'
  $c; done`), add pairs for whatever surfaces, re-filter. Converges in one
  or two rounds.
- **Per-commit compilability is a separate concern.** Cherry-picked
  intermediates often carry conflict resolutions that pulled later file
  versions: premature module declarations, build-manifest entries before
  their files exist, test helpers arriving after their users. Fix each
  commit's tree minimally — the next commit's diff simply re-adds the
  stripped lines, which reads as the honest introduction. When a file's
  introduction must move earlier (e.g. `tests/common`), inject the **final
  (cleaned)** version at first use in every tree that lacks it. Sometimes
  the right fix is a squash: two commits whose contents are inseparable.
- **Do not assume the original history compiled either.** In
  gamebus-presenced, the original S0/S3 commits already referenced modules
  that did not exist yet (verified 2026-08-07), so there was no oracle to
  restore from — fixes had to be hand-written and compile-verified per
  commit.
- **Verification battery:** per-commit `cargo check --all-targets` (or the
  language's equivalent), per-commit banned-pattern grep, programmatic
  message constraints, and tip-tree byte-identity against the tree that
  passed the full test gate. The chain of equality:
  `pristine + script == validated`, `filtered-tip == pristine + script`,
  therefore `filtered-tip == validated` — no need to re-run the suite on
  the filtered branch.
- **Follow-up edits that must appear "from the start"** (license files,
  README sections) are cheap one-line tree-filters appended later; verify
  per-commit presence with a checksum loop.
- **Keep the pre-rewrite branch as a backup ref** until published.

## When to reach for it

Any change that must hold at **every commit**, not just the tip: publishing
internal work as a public branch, license/header retrofits across history,
scrubbing accidental internal references. The tree-filter approach trades
diff-level surgery (conflict-prone, per-commit judgment) for tree-level
determinism (one script, run everywhere, verified everywhere).

## Provenance

- Origin: [[wiki/projects/gamebus-presenced]] public-branch re-composition,
  2026-08-07. Session log:
  [[wiki/logs/2026-08-07 - gamebus-presenced Public Branch Re-Composition]].
- Verified 2026-08-07 by the full battery above: per-commit
  `cargo check --all-targets`, per-commit banned-pattern grep, and tip-tree
  byte-identity against the hand-validated tree that passed the test gate.
- Related pattern note from the same project:
  [[wiki/concepts/Two-Writer State Files with Owned Halves]] (same vault
  rule of extracting reusable patterns from project work).
