---
type: concept
date: 2026-08-04
tags: [concept, design-pattern, gamebus-presenced]
ai-first: true
---

## For future Claude

When deduplicating records across a process tree, a single `HashMap<key, pid>` where each key maps to exactly ONE record — the deepest pid, decided by `tree_depth` — is simpler and more robust than pairwise ancestor-walk merging. The pairwise approach had convergence issues with siblings (same-depth processes), arbitrary merge direction, and re-emission on subsequent ticks. The one-record-per-key approach eliminates all of these. Apply this pattern whenever you face a similar deduplication-by-key across a hierarchy — do not reach for pairwise merging.

## Context

[[wiki/projects/gamebus-presenced]]'s Enricher tried pairwise ancestor-walk merging for ~20 Steam processes in a Wine wrapper chain. Siblings at the same depth merged arbitrarily, utility processes absorbed the actual game, and records were re-created on subsequent ticks. The user's suggestion — "why not just a HashMap with appids as key?" — was exactly right: one HashMap, one record per key, `tree_depth` decides, no convergence issues. See [[wiki/logs/2026-08-04 - gamebus-presenced S4]] for the full session and [[wiki/decisions/adr-008-appid-records-one-record-per-merge-key]] for the formal decision.

## The General Pattern

When deduplicating by a shared key across a hierarchy, prefer **one-record-per-key** (`HashMap<key, representative>`) over **pairwise merge** (iterate all pairs, decide direction per pair).

| Property | One-record-per-key | Pairwise merge |
|---|---|---|
| Complexity per new item | O(1) | O(n²) |
| Determinism | Yes — deepest/representative wins | No — direction decided per pair |
| Order-independent | Yes | No — merge result depends on iteration order |
| Same-level siblings | Handled naturally (tree_depth tie-break) | Fragile — arbitrary merge direction |
| Re-emission on subsequent ticks | Eliminated — key already holds the representative | Possible — merged records can be re-created |

## When to apply

- Deduplicating process records by appid across a process tree (the gamebus-presenced case)
- Any hierarchy where items share a natural key and you need one canonical representative
- When you catch yourself writing pairwise iteration with "decide merge direction" logic — stop and use a HashMap instead

## When NOT to apply

- When the merge operation is not idempotent (you genuinely need both records' data combined, not just one representative)
- When there is no natural single key to deduplicate on

## Related

- [[wiki/projects/gamebus-presenced]]
- [[wiki/logs/2026-08-04 - gamebus-presenced S4]]
- [[wiki/decisions/adr-008-appid-records-one-record-per-merge-key]]
