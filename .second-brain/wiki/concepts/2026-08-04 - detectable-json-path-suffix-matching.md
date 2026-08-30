---
type: concept
date: 2026-08-04
tags: [concept, detectable-json, discord, gamebus-presenced]
ai-first: true
---

## For future Claude

Discord's `detectable.json` database is mostly path-prefixed, not bare filenames. 83% of executable entries (9297 out of 11218) are path-prefixed (e.g. `amnesia the bunker/amnesiathebunker.exe`). A basename-only index misses most of the database. Discord's own scanner does **path-suffix matching**: the entry name is matched as a suffix of the full exe path (case-insensitive, backslash-normalized for Wine paths). Always index entries by basename but do path-suffix matching at lookup time - never assume a bare-filename index is sufficient.

## The Data

- **11218** total executable entries in `detectable.json`
- **9297** (83%) are path-prefixed - `amnesia the bunker/amnesiathebunker.exe`
- **1921** (17%) are bare filenames - `hl2.exe`
- A basename-only index finds at most 17% of the database directly and relies on collision-prone fallback for the rest

## The Fix

Bucket entries by basename, then at lookup time try in order:

1. **Path-suffix match** (most specific) - disambiguates basename collisions like `amnesia/amnesia.exe` vs `amnesia the dark descent/amnesia.exe`. The entry's full path is matched as a suffix of the exe's full path, case-insensitive, with backslashes normalized for Wine (forward-slash → backslash).
2. **Plain entry** (no path component) - the entry is a bare filename, match directly.
3. **Deterministic first-in-bucket fallback** - if multiple path-prefixed entries share the same basename and no suffix match is unambiguous, pick the first deterministically (sorted by entry name).

## Why it matters for gamebus-presenced

[[wiki/projects/gamebus-presenced]] reads `detectable.json` to enrich Steam appids with Discord game names (S4b naming enrichment). Without path-suffix matching, the enrichment would miss 83% of Discord's database - the naming fallback would fire far more often than necessary, degrading to executable stems. See [[wiki/logs/2026-08-04 - gamebus-presenced S4]] for the session where this was discovered.

## Related

- [[wiki/projects/gamebus-presenced]]
- [[wiki/logs/2026-08-04 - gamebus-presenced S4]]
- [[wiki/decisions/adr-008-appid-records-one-record-per-merge-key]]
