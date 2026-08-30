---
date: 2026-08-08
type: devlog
tags: [devlog, rust, tui, umu, gog, epic, naming]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

[[wiki/projects/gamebus-presenced|gamebus-presenced]] session of 2026-08-08 (the day after
[[wiki/logs/2026-08-07 - gamebus-presenced v0.1.0 Release|the v0.1.0 release]]): branch
`fix/umu-export-conformance` (9 commits) fixed umu-database export drift found by the first
real submission (Control, upstream PR #151), built the S9c manual-matchup workbench into the
setup TUI's misses pane, and - triggered by a live Project Hospital mislabel - made the stash
mirror the group's ELECTED identity plus a shipped shared-helpers list and a user title
override. Merged to master as --no-ff `fb8a702` (branch kept), rolled out (reinstall + daemon
restart), plus one pre-existing en-dash fixed directly on master (`075121b`). All facts
verified live 2026-08-08.

## 1. Conformance fixes (commit 8475823)

The first real submission - Control, upstream umu-database PR #151 - showed the export
drifting from the database's own rules:

- CSV NOTE column now **always empty**: per the upstream README's Genshin example the column
  is for game-related remarks only; our provenance line moved into the MR evidence text.
- `gog` rows are held back unless the codename is a **numeric gogdb.org product id**.
- Evidence links are per-store authorities (not one generic link).

Upstream facts verified live (as of 2026-08-08):

- PR #151 is the owner's own commit on branch `fschaupp/mr/control`, **still open**.
- The public umu API does **NOT serve unmerged entries** - Control queries returned `[]`
  while the owner's local checkout had the rows.
- The API is **case-insensitive** (Catnip/catnip/CATNIP all matched).
- `--check-prs` caught PR #151 in production and annotated all 9 Control stash entries
  `possible_pr`.

## 2. Local umu-db swap test (owner's request) - the loop closes

Discovered where the ids actually resolve from: Lutris (flatpak) reads
`~/.var/app/net.lutris.Lutris/data/lutris/runtime/umu-games/umu-games.json` - matching is
**case-SENSITIVE `==` on store+appid** (314 entries, no Control). protonfixes'
`umu-database.csv` is title-display only.

Patched the json (+3 Control rows, backup kept beside it) → live launch resolved
`GAMEID=umu-870780`, bus record "Control" with steam appid 870780, **no new stash miss** -
the full contribution loop (miss → stash → draft → PR → local verify) closed for the first
time.

- **Wart found (open item):** the resolved umu id SPLITS the group - wrapper keys
  `lutris:<uuid>`, game keys `steam:870780` → two records during play. Unfixed, future slice.
- **Finding for PR #151:** egdata's "Namespace" field (lowercase `calluna`) is NOT the
  README's egs codename - the Builds "App Name" (`Calluna`) is. Recommended capitalizing in
  the PR: Lutris matches case-sensitively, and the DB precedent is capitalized
  (Catnip/Heather/Grunion).

## 3. S9c matchup workbench (commits 31c3596, 4ae2091, eeef879)

The misses pane became a manual-matchup workbench:

- `p` pick - two zero-network candidate sections: umu db → verdict; Heroic `store_cache`
  libraries → identity via store/codename overrides.
- `o` one-request store lookups - GOG catalog by title; a gog **numeric** codename flips to
  an `api.gog.com` by-id product lookup that sets the TITLE; egdata search → sandbox builds
  App Name. The namespace is structurally never offered (see §2 finding).
- Stale-cache (>7d) warning with `v` as the exit; dismiss selection-hop fix.
- `ui/` + `umu_misses/` split into per-concern modules - mechanical, byte-identical moves.
- `endpoints.toml` gained `[gog]` catalog/product/gogdb_product and `[egs]`
  search/sandboxes.
- GOG crate research (as of 2026-08-08): crates.io `gog` 0.5.0 (Feb 2023) wraps the
  authenticated store API, not the catalog - hand-rolled call kept.

## 4. Project Hospital mislabel + defense (commits 06e603d, 8bad5c6)

Live incident: a Heroic-GOG launch (codename 1660194629, verified = Project Hospital via
api.gog.com) was stashed as "Spellcraft" at high confidence. Root cause: the daemon resolved
the shared helper exe `UnityCrashHandler64.exe`, and detectable.json's only entry listing it
is Spellcraft (path-prefixed, suffix miss → first-in-bucket fallback). The monitor was right
(group election is monotone); the stash was wrong (event-stream hook with a
same-confidence-different-title refresh). See
[[wiki/concepts/Elected State Beats the Event Stream]] (written in parallel).

Fixes:

- (a) The stash now mirrors the group's **ELECTED identity** - `note_group_identity` after
  every `set_identity` site; a regression test replays the incident's exact event order.
- (b) `shared-helpers.txt` shipped file (bundled → installed → user config, **UNION
  semantics** - local files add, never remove; installed beside `endpoints.toml`).
- (c) `t` title override in the TUI + `o`'s by-id title fix - an annotation-half
  `title_override` threaded through everything, with honest provenance:
  "title set by you (resolver said 'Spellcraft')".

The owner fixed their live entry in a few clicks.

## 5. Checklist honesty + dash hygiene (88ac13c, a993c5a docs, master 075121b)

- The export checklist's "Store ids are lowercase" box was misread by the owner as claiming
  codenames are lowercase → dropped entirely, along with the NOTE-column box.
  **Principle: the checklist is for what a human must verify, not what code guarantees** -
  encoded as a code comment plus inverted test asserts. The owner dropped the
  lowercase-pin test and its commit (9dcf825) themselves.
- Em-dash scan before finishing: two leak paths fixed - `shared-helpers.txt` (a `.txt` file
  escapes the public cherry-pick filter) and all export/stash-persisted strings. One
  pre-existing EN-dash found in the TUI seen-range (also shipped in v0.1.0), fixed directly
  on master (`075121b`). Repo now 0 en-dashes.

## 6. Process

- Owner's new standing rules this session: **never commit while exploring** (subagents
  included); **feature-branch commits stay docs-free** - PLAN.md updates go in one closing
  docs commit (`a993c5a`) for public cherry-pickability.
- Per-commit compile checks in throwaway worktrees.
- Gate at merge: 302 tests, clippy `-D warnings`, fmt - all green.
- Merge: --no-ff `fb8a702` to master, branch `fix/umu-export-conformance` kept.
- Rollout: install ran S10's best-effort fetch-detectable (12.3MB), daemon restarted,
  active now.

## Related

- [[wiki/projects/gamebus-presenced]]
- [[wiki/logs/2026-08-07 - gamebus-presenced v0.1.0 Release]] (previous session)
- [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]] (the stash's two-writer
  split that this session's annotation-half `title_override` builds on)
- [[wiki/concepts/Elected State Beats the Event Stream]]
