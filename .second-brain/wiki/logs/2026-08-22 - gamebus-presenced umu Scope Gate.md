---
date: 2026-08-22
type: devlog
tags: [devlog, rust, umu, protonfixes, umu-database, scope, tui]
related-projects: [gamebus-presenced]
confidence: high
ai-first: true
---

## For future Claude

[[wiki/projects/gamebus-presenced|gamebus-presenced]] session of 2026-08-22: the owner talked to
the umu team and learned that umu-database only wants entries for games that REQUIRE a
Proton/umu fix - a rule stated in the README's own opening paragraph that earlier sessions had
read past. Consequence: the already-submitted PR #151 (Control) does NOT qualify, because no
protonfix for it exists upstream. The session built the gate into gamebus-setup: a new
`ProtonFixes` index of Open-Wine-Components/umu-protonfixes, a `fix` annotation on every miss,
and `--export` refusing to submit anything without a fix behind it. Everything is UNCOMMITTED in
the working tree (the owner has not closed the exploration, see `_CLAUDE.md` §0.9). All facts
below verified live 2026-08-22.

## 1. The scope rule we had missed

The umu-database README opens with this, verbatim (as of 2026-08-22, confidence: stated):

> This database is by no means a complete database of every game released on Windows. We focus on
> games that requires fixes in Proton. Games that run out of the box have no need be added to the
> database. If you want a more extensive database of games you can use the Lutris API.

Source: https://github.com/Open-Wine-Components/umu-database (README, opening paragraph).

The owner heard the same thing from the umu team directly (confidence: stated). Earlier sessions
had anchored on the README's ID-format rules further down the page and never took the opening
paragraph as a **gate**. It is one.

**Consequence for PR #151** (Control, `gog`+`egs`, still the owner's open submission - see
[[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]]): it does
not qualify. Verified 2026-08-22 on umu-protonfixes master - there is no
`gamefixes-steam/870780.py` - and Proton's own `proton_9.0` script has no `870780` branch either.
Control runs out of the box. Nothing in the database wants it.

## 2. What upstream actually looks like (measured 2026-08-22, confidence: high)

All numbers measured directly against Open-Wine-Components/umu-protonfixes master, not estimated:

- **488 files** under `gamefixes-*`, serving **377 distinct umu ids**.
- Layout: `gamefixes-steam/<appid>.py` and `gamefixes-<store>/umu-<id>.py`.
- **113 of those files are symlinks** (git mode `120000`) - this is the "link the other store's
  copy to the Steam fix" mechanism, and it is exactly what a database row unlocks.
- Per directory: steam 313, gog 79, umu 37, egs 29, ubisoft 9, zoomplatform 9, humble 3, itchio 3,
  amazon 2, battlenet 2, ea 2.

Source: https://github.com/Open-Wine-Components/umu-protonfixes

**Enforcement has been loose historically**, which is worth remembering before someone argues from
precedent: only **240 of 1,202 rows** in the local `umu-database.csv` (196 of 1,100 distinct ids)
have a fix behind them, and **182 fix ids are not in the database at all**. The existing content
is therefore not precedent for submitting fix-less games - it is drift.

## 3. The gate, built (all uncommitted)

**New module `src/setup/umu_misses/fixes.rs`** (304 lines). A `ProtonFixes` index parsed from
GitHub's recursive tree listing of umu-protonfixes, cached as a plain path list at
`~/.cache/gamebus-presenced/umu-protonfixes.txt` and refreshed when older than 7 days.
`GAMEBUS_UMU_PROTONFIXES` - the fix-list twin of the existing `GAMEBUS_UMU_DB` - points the check
at a local umu-protonfixes checkout (directories read straight off disk) or at an exported list.
That override is also what keeps the test suite off the network.

**`Miss` gained `fix: Option<FixCheck>`** (`umu_id`, `fixes: Vec<String>`, `checked`) in the
**annotation half**, adopted through the same two-writer merge as every other annotation - see
[[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]].

**CLI and TUI:**

- `--verify` now records, per entry, whether a protonfix exists for the id that entry would carry.
- `--fetch` refreshes the database **and** the fix list.
- `--export` / `--export-md` submit only entries that have a fix.
- The merge-request text states the scope rule and links the fix file per row.
- The TUI gained a "Needs umu" detail line and a dark `○` glyph for out-of-scope entries.
- `usage()` documents all of it.

**Endpoints:** `umu.protonfixes_tree` and `umu.protonfixes_file` added to `endpoints.toml` (the
daemon still reads none of it).

## 4. The honesty split - found by running it live

"No fix found" does not mean the same thing for every id, and the first live run made that
obvious. The distinction now runs through the review list, the TUI, and the held-back reasons:

- **Firm id** (the database's own id, a `detectable.json` Steam sku, or a manual assignment):
  no fix **proves** the game runs out of the box. Say so.
- **Guessed id** (a store-codename or title-slug draft): no fix proves nothing - a fix could sit
  under a Steam appid we never learned. Those entries say instead: *that id is our own guess,
  nothing here shows the game needs umu.*

This is the same class of honesty rule as S9c's "the checklist is for what a human must verify,
not what code guarantees" - do not let a negative result claim more than the input supports.

## 5. Live run against a copy of the real stash

Run against a **COPY** of the real stash under a scratch `HOME`; the real stash was untouched.

**None of the 15 misses qualifies.**

- Control resolves to `umu-870780` - no fix.
- Project Hospital (`umu-868360`), Call of Duty: Black Ops Cold War (`umu-1985810`) and Star Wars
  Outlaws (`umu-2842040`) all draft clean Steam-appid ids and have no fix either.

`--export-md` now answers:

> Nothing to export: 6 of 15 entries need no protonfix, so the database does not want them.

## 6. Gate

Green: **310 tests**, `clippy -D warnings`, fmt. New coverage for the tree/cache parsers, the
scope gate itself, the firm-vs-guessed distinction, and the CLI end to end.

## 7. Open afterwards

- The owner still has to decide whether to **withdraw PR #151**.
- The feature set was **committed the same day** as `e41d657` on master (11 files, +813/-57) after
  the owner closed the exploration; the gate was re-run green immediately before. Still owed: the
  **PLAN.md S9 docs commit** (feature commits stay docs-free) and the **gamebus-setup reinstall**.
- The daemon is untouched by all of this - it stays network-free.
- The same evening produced the follow-on design decision:
  [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]] - since the scope rule
  means most of what this project learns can never go upstream, that knowledge gets its own
  PR-based repo of per-game TOML pages.

## Related

- [[wiki/projects/gamebus-presenced]]
- [[wiki/concepts/Game Identity Data Sources]] (the map of which source answers which identity
  question; umu-protonfixes is now one of them)
- [[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]] (the
  session that submitted PR #151 and built the workbench this gate now filters)
- [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]] (the annotation half the new
  `fix` field lives in)
- [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]] (where the knowledge this
  gate keeps out of umu-database is meant to go instead)

## Sources

- umu-database README (scope paragraph, verbatim above):
  https://github.com/Open-Wine-Components/umu-database
- umu-protonfixes (all counts measured against master, 2026-08-22):
  https://github.com/Open-Wine-Components/umu-protonfixes
- Upstream submission: umu-database PR #151 (Control, branch `fschaupp/mr/control`)
