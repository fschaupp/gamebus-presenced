---
date: 2026-08-08
type: concept
topic: derived views must read elected state, never the raw event stream
tags: [concept, consistency, state-machines, pattern, gamebus-presenced]
related-projects: [gamebus-presenced]
sources: [gamebus-presenced umu-miss stash write hook (branch fix/umu-export-conformance, merge fb8a702, 2026-08-08)]
confidence: high
ai-first: true
---

## For future Claude

A consistency pattern extracted from the gamebus-presenced
Project Hospital/"Spellcraft" mislabel incident (2026-08-08): when a system
holds both an **event stream of raw claims** and an **elected/settled state**
derived from those claims, every derived view must read the elected state -
a view fed directly by events inherits event-ORDER semantics and can silently
disagree with the settled state it is supposed to reflect. Companion to
[[Two-Writer State Files with Owned Halves]] (same stash file, different
failure axis: that one is about *who writes*, this one is about *what a
writer listens to*).

## The pattern

Many systems carry two representations of the same truth:

- An **event stream** - raw claims as they arrive (identity guesses, votes,
  sensor readings), each with some rank (confidence, class, precedence).
- An **elected state** - what the system has settled on, derived from the
  claims via a monotone election, consensus, or precedence rule. Once
  settled at a rank, only a strictly better claim can move it.

The trap: a derived view (a persisted stash, a cache, an export, a metrics
row) that subscribes to the *event stream* instead of reading the *elected
state*. Such a view inherits **event-order semantics** - last-writer-wins
among equal-rank claims - while the elected state stays put by construction.
The two then disagree, and nothing detects it: each side is locally
"correct" under its own semantics. The disagreement is an ordering accident,
invisible to tests that never replay an adversarial arrival order.

## The concrete instance (gamebus-presenced, 2026-08-08)

The umu-miss stash's daemon write hook fired on **per-member identity
claims** reaching a umu-missed group - the rule was "any identity reaching
the group is worth recording", with refresh-on-same-confidence. The
published D-Bus record, meanwhile, read the group's **monotone class
election** (S4f: class-elected sticky representative). During a Project
Hospital session, a Unity crash handler process raised a same-rank claim
with the wrong title: the election correctly ignored it (same rank, no
upgrade), so the published record stayed "Project Hospital" - but the stash
flip-flopped to "Spellcraft", because at equal confidence the last event
won. Two views of one group, disagreeing, both "working as coded".

## Fix shape

Derive the view **from the elected state, at every point the election can
change** - not from the claims feeding the election. In the source case the
stash write hook now fires on the group's ELECTED identity after routing;
member claims that lose the election never reach the stash at all (S9c,
branch `fix/umu-export-conformance`, merge `fb8a702`). The view then
inherits the election's monotonicity *structurally* - there is no
same-rank-refresh path left to misuse - and agreement between the views
stops being an ordering accident and becomes a consequence of shared
derivation.

## Test shape

Replay the incident's exact arrival order (right claim, then wrong same-rank
claim, in that sequence) and assert the derived view matches the election
afterwards. A test that feeds only well-ordered or single-claim streams
cannot see this bug; the arrival order IS the test input.

## When to reach for it

Any time you add a persisted or exported view to a system that already has
an election/consensus/precedence rule: ask "does this view read the settled
state, or the claims?" If the answer is claims, either justify why
event-order semantics are wanted (an audit log of raw claims genuinely wants
them) or move the tap after the election. Views meant to *reflect* settled
truth must never sit before the settling.

## Provenance

- Origin incident: Project Hospital mislabeled as "Spellcraft" in the
  umu-miss stash, 2026-08-08, [[wiki/projects/gamebus-presenced]]. Fixed in
  S9c, branch `fix/umu-export-conformance`, merged `fb8a702` (2026-08-08).
- Session log:
  [[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]]
- The stash's ownership rules (who writes which half):
  [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]] - its
  2026-08-08 amendment records this write-hook change.
- Sibling pattern from the same file:
  [[Two-Writer State Files with Owned Halves]].
