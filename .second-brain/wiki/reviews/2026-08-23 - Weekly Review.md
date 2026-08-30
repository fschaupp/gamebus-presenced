---
type: review
period: weekly
date: 2026-08-23
week: 2026-08-17 to 2026-08-23
tags: [review, weekly, gamebus-presenced, gamebus-gamedb]
related-projects: [gamebus-presenced]
ai-first: true
---

# Weekly Review - 2026-08-17 to 2026-08-23

## For future Claude

The week gamebus-presenced's identity knowledge became its own public
project. Quiet first half (only game launches, recorded in the umu-miss
stash: Black Ops Cold War and Control on 08-15, Project Hospital through
08-16 - no vault notes exist for those days). Then two dense days: 08-22
reframed the umu contribution pipeline around the scope rule the README had
stated all along, and 08-23 published **gamebus-gamedb** end to end and
taught the daemon's tooling to feed it. First weekly review in this vault;
there is no earlier one to compare against.

## What I accomplished

- **The umu scope gate** (08-22, [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]]):
  umu-database only wants games that need a Proton fix - the umu team said
  so, and their README's opening paragraph always had. Built the protonfix
  check into `gamebus-setup` (`e41d657` on master): exports now carry only
  qualifying games, with honest wording split between firm and guessed ids.
  Live consequence: none of the 15 stashed misses qualifies, and the open
  upstream PR #151 (Control) should probably be withdrawn.
- **gamebus-gamedb designed, built, split out, and PUBLISHED**
  (08-22 design, 08-23 everything else;
  [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]],
  [[wiki/logs/2026-08-23 - gamebus-gamedb Published and the Export Loop]]):
  per-game TOML pages under ODbL at https://github.com/fschaupp/gamebus-gamedb,
  PR-gated by a lint fetched by tag+digest from gamebus-presenced's
  gamedb-tools-v0.1.0 release, artifacts (JSON/Parquet/SQLite/helpers/manifest)
  as release assets on date tags, first data release v2026.08.23. CI-built
  binaries byte-identical to local builds.
- **The export loop** (branch `feat/gamedb-export`, unmerged): the stash
  folds one-page-per-game, checks the published index, writes new pages, and
  **enhances already-published ones in place** (Control gained its exe as a
  one-line diff). Fourth TUI tab with the matchup keys.
- Daemon v0.2.0 released (owner-run pipeline).

## Key decisions made

All recorded in [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]]
and the project note's Key Decisions blocks (08-22 and 08-23):

- Corrections repo over fork or upstream submission; one TOML page per game;
  canonical `gamedb` id derived-then-frozen; one game one page, enforced by
  the merge build; ODbL for the data; date tags for data releases; tools
  pinned by digest; enhancement never regenerates a page (toml_edit
  round-trip); a page exe must end `.exe` and not be a shared helper.
- Reversed within the same sessions, reasons written down: numeric-means-
  Steam ids, mandatory `year`, year directories, filename-derived keys, GPL,
  DuckDB as a build dependency.
- New standing rule, now `_CLAUDE.md` Section 0.10: **only the owner pushes.**

## People I worked with

- **The umu team** (via the owner): their scope rule triggered the whole arc.
  No new person notes; no individual named.

## What I learned

- **Read the opening paragraph.** The umu scope rule sat in plain sight for
  weeks while sessions anchored on the ID-format rules further down. The
  gamebus-gamedb README states its scope in paragraph one because of this.
- **Run it against the real data before believing it.** The week's worst
  defects were invisible to green test suites: wrapper processes
  (`python3.13`, `env`) becoming page exes, the release tag lost to GitHub's
  second redirect, `core.filemode=false` swallowing exec bits, a brief's
  example GOG id belonging to a different game (an agent caught that one).
- **Store the claim, check it, never trust it**: ids stored for visibility
  and lint-recomputed; tool pins proven against the data before written;
  determinism proven byte-for-byte across machines.
- **The review-format/lookup-format split pays rent**: every artifact choice
  stayed reversible because the TOML pages are the source of truth.

## What to carry forward (owner's queue, in dependency order)

1. Merge `feat/gamedb-export` (5f2d219 + 769cdf5, reviewed and TUI-tested
   08-23) and reinstall gamebus-setup - not reinstalled since the scope gate.
2. Cut gamedb-tools-v0.1.1 (the `page` column), re-pin, new data release.
3. Control's GOG codename via the gamedb tab's `o` lookup.
4. First real gamebus-gamedb PR: two exe-only pages + the Control exe
   enhancement.
5. Older, still open: withdraw-or-keep PR #151, restore Lutris
   `umu-games.json`, republish the Assisted-by history, push master to gitea.

## Suggested questions for future-Claude

1. **Could the gamedb alias table fix the group-split wart?** The daemon
   splits a wrapper (`lutris:<uuid>`) from its game (`steam:<appid>`) during
   play - the oldest open item on [[boards/gamebus-presenced]] - and as of
   this week the published `identities.sqlite` resolves *any* identifier,
   exe included, to one game id
   ([[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]]).
   These two facts co-appeared all week without being connected.
2. **What does the daemon actually gain from consuming the index?** ADR-010
   parks "daemon reads Parquet" as a later feature, and
   [[wiki/concepts/Game Identity Data Sources]] shows per-source resolution
   already works. The unstated trade: the index is this machine's own
   knowledge round-tripped - its value arrives only with outside
   contributors. When is that worth a read path?
3. **Should icons ride on gamedb pages?**
   [[wiki/concepts/Richer Presence Over the Bus]] proved art is already in
   hand (detectable.json hashes, verified CDN URLs), and ADR-010 lists art
   references on pages as open. The licensing question recorded there is the
   real blocker, not the data.
4. **Does withdrawing PR #151 follow from the week automatically?**
   [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]] proves Control
   does not qualify upstream, and since 08-23 the knowledge it carried lives
   on Control's published gamedb page - the PR's only remaining value.
   The board still lists withdraw-or-keep as undecided.
5. **Is the Spellcraft report to Discord stronger now?** The bad detectable
   row is documented with incident provenance in gamebus-gamedb's public
   `helpers.toml` ([[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]]) -
   a citable public artifact where before there was only a local file. The
   backlog item predates that and does not mention it.

## Related

- [[wiki/daily/2026-08-22]] · [[wiki/daily/2026-08-23]]
- [[wiki/projects/gamebus-presenced]] · [[boards/gamebus-presenced]]
