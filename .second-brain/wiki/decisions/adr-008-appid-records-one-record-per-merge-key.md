---
type: adr
date: 2026-08-04
tags: [decision, adr, gamebus-presenced]
ai-first: true
status: Accepted
---

# ADR-008: appid_records — One Record Per Merge Key

## For future Claude

This ADR records why we abandoned pairwise ancestor-walk merging in favor of a
simple `HashMap<String, u32>` keyed by merge key. The old approach tried to
deduplicate wrapper-tree processes by walking ancestor relationships between
pairs; it could not handle siblings and had convergence issues. The new
approach keeps exactly one record per merge key, with `tree_depth` as the
tiebreaker — no pairwise logic, no convergence.

## Context

S4d implemented a pairwise ancestor-walk merge that deduplicated
wrapper-tree processes sharing the same Steam appid by checking
`is_ancestor` between pairs of pids. This handled direct
ancestor/descendant pairs (e.g. a wrapper launcher vs. the actual game
process) but **failed for siblings**: same-depth processes like
`wineserver`, `explorer.exe`, and `re2.exe` — all children of `pv-adverb`
— were never collapsed because none is an ancestor of another.

The scan emitted Steam partials for **every process in the wrapper chain**,
creating roughly 20 records per game. Multiple fixes were attempted:

- **same_tree** — group processes by shared tree root; still left siblings
  as separate records.
- **find_all_related** — gather all processes related by ancestry; no
  deterministic way to pick which pid should hold the merged record.
- **tree_depth comparison** — pick the deepest pid; worked in isolation but
  the pairwise walk still had convergence issues when the tree shape
  changed between scan passes.

All approaches shared a fundamental problem: **pairwise merging has
inherent convergence issues** because the result depends on the order of
pair comparisons and the tree can shift between scans.

## Decision

Replace the pairwise ancestor-walk merge with a single
`appid_records: HashMap<String, u32>` — a map from **merge key** to the
**deepest pid** holding that key.

**Rules:**

1. Every scanned process produces a merge key:
   - Steam → `steam:<appid>`
   - Lutris → `lutris:<uuid>` (from `LUTRIS_GAME_UUID` env var)
   - umu → `umu:<id>` (from `UMU_ID`, only when not `umu-default`)
2. For each merge key, the record stores **one pid** — the deepest one.
   When a new pid with the same key is encountered, `tree_depth` decides:
   deeper pid replaces shallower.
3. No pairwise `is_ancestor` checks. No convergence. One record per key,
   always.

This generalizes beyond Steam: any source that can produce a stable merge
key gets the same deduplication for free.

## Consequences

**Positive:**

- **No convergence issues** — the map is rebuilt each scan pass from
  scratch; there is no iterative pairwise state to converge.
- **O(n) per scan** instead of O(n²) pairwise comparisons.
- **Sibling-safe** — siblings sharing a merge key collapse to the deepest
  one regardless of tree shape.
- **Generalizable** — adding a new source (e.g. Heroic, Bottles) only
  requires defining how to derive its merge key.
- **Simple to reason about** — one key, one record, deepest pid wins.

**Negative:**

- **Merge key must be derivable** — a process without a recognizable merge
  key is invisible to this system (same limitation as before, now
  explicit).
- **tree_depth as sole tiebreaker** — two processes at the same depth
  with the same key: last-seen wins. In practice this is rare and the
  choice is arbitrary anyway.
- **No partial merging** — if two genuinely distinct games share a key by
  collision, they'd be merged. Keys are namespaced (`steam:`, `lutris:`,
  `umu:`) to make this unlikely.

## Alternatives

1. **Keep pairwise ancestor-walk** (`adr-007` approach) — rejected due to
   sibling failure and convergence issues across scan passes.
2. **same_tree grouping** — group by tree root, emit one record per tree.
   Rejected: a tree can contain multiple unrelated games (e.g. a Steam
   launcher spawning a non-Steam helper), so tree root is too coarse a key.
3. **find_all_related + deterministic pid selection** — gather all
   related pids, then pick by depth. Rejected: the "gather all related"
   step still requires pairwise ancestry walks, reintroducing the
   convergence problem.
4. **Per-source deduplication** — handle Steam, Lutris, umu separately
   with source-specific logic. Rejected: more code, more edge cases, and
   the merge-key abstraction is strictly more general.

## References

- [[wiki/projects/gamebus-presenced]]
- [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]]
- [[wiki/logs/2026-08-04 - gamebus-presenced S4]]
- [[wiki/concepts/2026-08-04 - Brainstorm - S4 Enrichment and Packaging]]
