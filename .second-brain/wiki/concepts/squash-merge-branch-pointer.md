---
date: 2026-08-06
type: concept
topic: git workflow - owner preference
tags: [concept, git, owner-preference, workflow]
related-projects: [gamebus-presenced]
confidence: stated
ai-first: true
---

## For future Claude

Owner preference stated 2026-08-06, in the style of
[[wiki/concepts/test-deletion-visibility]]: when landing a feature branch as
one unit, a plain `git merge --squash` is not acceptable - the resulting
commit must carry a pointer to the feature branch. Apply this to every merge
performed for Florian in any repo.

## The rule

A feature branch lands as **one unit with its ancestry intact**:

1. **Prefer `git merge --no-ff`** with the custom message on the merge commit.
   The feature branch becomes a real second parent, shows as merged in the
   graph, and stays traversable. When the branch holds a single commit this
   *is* the squash - one unit on the mainline, history preserved.
2. **If a true `--squash` is unavoidable** (branch has messy multi-commit
   history that must collapse), the squash commit's message must name the
   feature branch and its head SHA, and the branch is **kept**, not deleted.
3. Never land a plain `--squash` with no pointer: it produces a single-parent
   commit with no graph connection to the branch - the branch never appears
   as merged, and its history becomes unreachable context.

## The incident (why this exists)

Landing S5 (2026-08-06), the assistant ran `git merge --squash
feat/s5-setup-tui` → commit `36404f9`, single parent, branch invisible in the
graph. Owner: *"the squash-merge has not second parent of the feature
branch."* Redone as `--no-ff` → merge commit `e56bbb8` with parents `62dfd21`
+ `18ace6c` (the branch head), byte-identical tree, branch kept. The content
was never the problem - **the missing ancestry pointer was.**

## Relation to other owner preferences

Same shape as [[wiki/concepts/test-deletion-visibility]]: the owner's default
is *traceability over tidiness*. Deleting a test silently and severing a
branch from the graph are the same failure - history made invisible without
consent.
