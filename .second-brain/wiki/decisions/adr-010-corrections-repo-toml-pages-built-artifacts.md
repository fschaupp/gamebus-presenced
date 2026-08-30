---
type: adr
date: 2026-08-22
tags: [decision, adr, gamebus-presenced, umu, data, toml, parquet, sqlite, duckdb, contribution]
ai-first: true
status: Accepted, implemented, PUBLISHED 2026-08-23 at https://github.com/fschaupp/gamebus-gamedb (tools release gamedb-tools-v0.1.0 pinned by digest, first data release v2026.08.23). Tools live in gamebus-presenced.
---

# ADR-010: gamebus-gamedb - Per-Game TOML Pages, With Built Artifacts

## For future Claude

The knowledge this project accumulates about game identity - store codename
to game, helper executables that never name a game, wrong rows in closed
datasets - has **no upstream that accepts it** (see
[[wiki/concepts/Game Identity Data Sources]]). The decision: publish it as
its own public git repo, in the shape umu-database *should* have had. One
**TOML page per game**, reviewed as a pull request, validated by a CI lint
that is our existing Rust validator, with **generated artifacts published as
release assets** - never committed. The repo optimizes for *review*; the
artifacts optimize for *lookup*. That split is the whole design, and it is
what makes every artifact-format choice reversible.

Status: **split into its own repository, 2026-08-23.** `/media/Data/Projekte/gamebus-gamedb`
holds the data set at its root (two seeded pages, `helpers.toml`, both schemas,
the ODbL text, CONTRIBUTING, a project page) with history carried over by
`git subtree split`, plus its own workflows: `lint.yml` gates pull requests and
`release.yml` builds the artifacts on a `v*` tag. **Neither workflow vendors a
tool** - both download `gamedb-lint` / `gamedb-build` from a pinned
gamebus-presenced release and verify the digest, and until the first such
release exists the pins are empty and the jobs fail saying so, rather than
passing on nothing.

gamebus-presenced keeps the tooling (`gamedb-build` crate, the Python lint,
the shared fixtures) and takes the data set back as a **submodule at `gamedb/`**
(`.gitmodules` records `https://github.com/fschaupp/gamebus-gamedb.git`; the
local checkout overrides that URL to the sibling directory until the remote
exists). `gamedb-tools.yml` tests the tooling; `gamedb-tools-release.yml`
publishes the two binaries plus `SHA256SUMS` on a `gamedb-tools-v*` tag - the
release the data repository pins. Verified: lint, self-test and all 37 builder
tests pass through the submodule.

**Published 2026-08-23** (all verified live; only the owner pushes, ever):
`fschaupp/gamebus-gamedb` is public with Lint green on `main`;
gamebus-presenced released **gamedb-tools-v0.1.0** (`gamedb-lint`
sha256 `1fbc8daf...`, `gamedb-build` `22440df0...`) and the data repo pins it
by tag and digest; the first data release is **v2026.08.23** - date tags, the
owner's choice, since a data set has no API surface to be semantic about. The
CI-built binaries produced artifacts **byte-identical** to local builds.
`.scripts/release.sh` (date tag, gates with the pinned tools) and
`.scripts/update-tools.sh` (moves the pin, proving the new tools on the data
first) live in the data repo. Consumption side: `gamebus-setup gamedb`
(branch `feat/gamedb-export`, commits 5f2d219 + 769cdf5, unmerged) folds the
umu-miss stash into pages, checks the published index, exports new games and
**enhances already-published pages in place** via toml_edit so everything the
index does not carry survives byte for byte. See
[[wiki/logs/2026-08-23 - gamebus-gamedb Published and the Export Loop]].

## Context

Three findings converged on 2026-08-22:

1. **No upstream wants our delta.** umu-database is deliberately scoped to
   games that require a Proton fix (their README's opening paragraph, and
   confirmed to the owner by the umu team). Discord's `detectable.json` has
   no public contribution path at all. Lutris's API is read-only, has no
   store-codename schema, and its README pointer is where umu sends people
   who want a broader catalog. Full survey:
   [[wiki/concepts/Game Identity Data Sources]].
2. **A fork of a catalog is the wrong answer.** Our unique knowledge is a
   *delta*, not a catalog: a helper-exe blacklist, confirmed store-codename
   identities, wrapper-layer election results. Forking 23,900 Discord rows or
   347,657 Lutris rows to carry a few hundred corrections inherits a
   maintenance burden for data we did not learn.
3. **umu-database's single-CSV shape is the failure to avoid.** Every
   contribution touches one file, so every PR conflicts with the one merged
   before it. Measured drift from having no validation gate (2026-08-22):
   only **240 of 1,202 rows** have a protonfix behind them, and **182 fix ids
   are absent from the database entirely**. By contrast umu-protonfixes is
   one file per game across **488 files** and never conflicts.

The owner's framing (2026-08-22, verbatim): *"i hated how umu-database has it
all in a file everyone writes to. contributing a page for the game would make
it clean to review - toml is easy to write and format both by hand or by
code, widely understood and trivially safe to parse even on a webpage or
retroemulation chip."*

## Decision

### 1. A separate public repo, PR-based, data only

Contributions arrive as pull requests. No API key, no auth service, no
moderation backend - the forge already provides identity, spam control, and
CI. The repo contains **only**: `README.md`, `LICENSE` (data license, TBD),
the per-game TOML pages, `helpers.toml`, one JSON Schema, and the CI
workflow. The **validator is not vendored** - the workflow downloads a
pinned, checksummed release of our Rust linter, so the data repo stays
genuinely data-only.

The README states the scope rule in its opening paragraph: facts a launcher
cannot derive on its own and that no upstream will accept. **Not a game
catalog.** This is the direct lesson of reading past umu's own opening
paragraph for weeks (see
[[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]]).

### 2. One TOML page per *game*, stores nested inside

Keyed by path (`games/control.toml`); the key is **never repeated inside the
file**, so it cannot drift.

```toml
title = "Control"

[ids]
steam = 870780
source = "steam-sku"
seen = "2026-08-22"

[[stores.egs]]
codename = "Calluna"
seen = "2026-08-15"
source = "heroic-config"
confidence = "high"
```

(That is `gamedb/games/control.toml` verbatim, minus its `note`. Control's GOG
codename is not recorded because this project has never observed it.)

- **Every page carries a canonical `gamedb` id, derived where an authority
  already names the game and minted only where none does** (owner's design,
  2026-08-22). Precedence: `steam-<appid>`, then the umu id, then a store
  codename containing a letter (`egs-Calluna`), then a minted
  `gamedb-<uid>`. Store order: gog, egs, ubisoft, ea, battlenet, amazon, humble,
  itchio, zoomplatform. A minted id is eight `[a-z0-9]` with at least one
  letter, which keeps it visibly distinct from an authority's number, and it is
  reachable only for a page with **no store identity at all** - identified by
  executable, which the schema now allows (`exe = [...]`, and a page must name
  either a store entry or an executable). The field may be
  omitted when derivable; the lint recomputes it from `[ids]` and the store
  entries and rejects a written id that disagrees, so it is **stored for
  visibility and checked for truth**, never trusted.
- **The prefix names the namespace; the shape of what follows is never
  parsed.** `gog-1660194629` is a valid canonical id. An earlier rule in this
  same session reserved the numeric form for Steam app ids, on the theory that
  a consumer would read a trailing number as a SteamAppId and hand it to
  Proton. The owner challenged it directly (2026-08-22) and it does not hold:
  umu's numeric rule applies to *umu ids* reached through `GAMEID`/`UMU_ID`,
  nothing consumes gamedb ids at all, and the misparse it guards against is a
  bug that `steam-870780` invites equally. The cost was real, though - **GOG
  product ids are always numeric**, so the rule silently forced every GOG-only
  game (most of the DRM-free back catalogue) into an opaque minted uid instead
  of a stable, derivable `gog-<id>`. Replaced by documentation: a gamedb id is
  not a umu id, and consumers cross over through `[ids]`.
- **One game, one page, enforced by the merge build** (owner's requirement,
  2026-08-22). Every identifier a page carries is a lookup key resolving to that
  page - its canonical id, each `<store>-<codename>`, the Steam app id, the umu
  id, every executable it names, and everything absorbed through `merged_from`
  - and **no two pages may claim the same one**. This catches the contribution
  mistake that actually happens: someone adds a game's GOG copy as a new page,
  not knowing the Epic copy is already there. Both pages then claim
  `egs-Calluna` (or the same app id, or the same exe) and CI fails, naming both
  files. Uniqueness on the canonical id alone would have missed it, since the
  two pages can perfectly well carry different `gamedb` values while describing
  one game. Consolidation stays manual and small (move the store entry onto the
  existing page); the build's job is only to stop the duplicate landing, because
  after it lands a lookup has two answers and neither is wrong.
- **Ids are frozen on assignment, and merges are recorded, not silent** (owner
  raised merging, 2026-08-22). Precedence decides an id once, at page creation;
  it is never recomputed. Without that split the lint fought the very fixup it
  was meant to survive: an exe-only page that later learns its Steam app id
  would be rejected until its id changed, retiring an identifier. Now the lint
  checks only that a stored id is one the page's own data justifies (or a
  well-formed minted id) and *warns* when a better identifier has since
  appeared. When two pages turn out to be one game, the survivor lists
  `merged_from = [...]`; absorbed ids stay reserved forever (never a live id
  elsewhere, never absorbed twice) and become alias rows in the artifacts, so an
  old lookup still resolves. Deciding two pages are the same game stays a human
  judgement - the mechanical part is only that no id is lost.
- **An id is never re-pointed**, but the **file name is free**: it is a human
  label (lowercase title slug, plus a qualifier when two games share a title),
  not the key. This replaced an earlier scheme where the key was derived from
  the file name - that made renaming a page retire an identifier, and made a
  title collision a structural problem instead of a naming one. Two earlier
  sketches died on the way: keying by the owning authority without the letter
  rule (the `gog-` hazard above), and shelving pages under `games/<year>/`,
  which would have put a **correctable fact inside an immutable id** and made
  contributors index by a fact they do not have to hand.
- **Title collisions, editions and DLC** (owner's question, 2026-08-22): two
  different games sharing a title (Prey 2006 / Prey 2017) do NOT rename the
  incumbent - that would retire a name, and names are never re-pointed. The
  newcomer simply takes a distinguishing file name (`prey-2017.toml`), which is
  all that is needed: different file names give different keys, and `[ids]`
  separates the games exactly for any consumer. **No `year` is required.** A
  first draft demanded one from every colliding page, and a second considered
  shelving pages under `games/<year>/`; both were dropped 2026-08-22. Year
  directories would have forced the key to become the whole path
  (`gamedb-2017-prey`), putting a **correctable fact inside an immutable id** -
  the same objection that killed `steam-870780` - and they index by a fact
  contributors do not have to hand: the common question is "does a page for this
  game exist?", which a flat directory answers at a glance and a year hierarchy
  turns into a grep. `year` survives as an optional field.
- **Editions are store entries, not pages** (`edition = "Deluxe"`): one game,
  several SKUs, and presence should say the game's name. **A remaster sold as
  its own product does get a page**, pointing home with `variant_of`
  (lint-checked to exist), because it is separately installed and separately
  named on screen. **DLC gets nothing** - it never launches as its own process,
  so no resolver has to name it. The dividing line throughout: if a launcher
  ever has to name the thing, it earns a page.
- **`[[stores.<store>]]`, nested one level** rather than top-level
  `[[egs]]`: top level stays identity, everything under `stores` is a store,
  and a consumer iterates one map instead of guessing which top-level keys
  are stores.
- **Always array-of-tables**, even for a single entry - regional variants and
  re-releases exist, and uniform shape means no parser special-casing.
- **Store vocabulary matches umu's exactly** (measured off their
  `gamefixes-*` directories, 2026-08-22): `steam`, `gog`, `egs`, `ubisoft`,
  `zoomplatform`, `humble`, `itchio`, `amazon`, `battlenet`, `ea`, `umu`.
  Free today; saves a mapping table later.
- **`taplo fmt --check` in CI** so nobody argues about spacing in review.
- TOML carries no schema of its own, so the **lint carries it** - the id
  letter rule, the collision check, store/codename conformance, and the scope
  gate, all of which already exist and are under test in
  `src/setup/umu_misses/`.

### 3. `helpers.toml` in, `shared-helpers.txt` out

The shared-helper list follows the same rule. Today `shared-helpers.txt`
carries its evidence in a **comment** (the Project Hospital /
UnityCrashHandler64 mislabel incident) where nothing can validate or query
it. In the repo each entry is TOML with its reason as data; the runner
flattens it to the `shared-helpers.txt` the daemon already union-merges
(`src/naming.rs`, bundled → config → data dir → system dirs, union never
override).

### 4. Built artifacts, published as release assets

Generated by a GitHub runner on merge, **never committed to the main branch**
(a binary blob per merge bloats the repo permanently):

| Artifact | For |
|---|---|
| `identities.json` | web pages, scripts, the general case |
| `identities.parquet` (zstd) | bulk and analysis; DuckDB can query it over HTTP with range requests without downloading it |
| `identities.sqlite` | in-process point lookup, indexed on `(store, codename)` and exe basename |
| `shared-helpers.txt` | the daemon's existing format |
| `manifest.toml` | source commit SHA + checksums per artifact |

A `identities.csv` in umu-database's exact column shape was in the first draft
of this list and is **deferred**: the subset umu would accept is defined by the
protonfix scope gate, which lives in `gamebus-setup` (`e41d657`), not in the
data set. Emitting it from the builder would either duplicate that gate or ship
a CSV whose "umu would take these" claim nothing checked. `gamedb/README.md`
lists the five artifacts above and is authoritative.

**Determinism is a requirement**: sorted keys, fixed field order, no build
timestamp inside the data. Parquet has no timestamp; SQLite's page layout
depends on insert order, so build with a fixed `ORDER BY`, `VACUUM` at the
end, and stamp `user_version` with the schema version. Then the manifest
checksum is meaningful and `--fetch` can do a cheap ETag check.

### 5. DuckDB in the runner; `rusqlite` in the daemon

One tool produces both binary artifacts - DuckDB's SQLite core extension
writes as well as reads (docs, 2026-08-22, verbatim: *"the extension also
allows you to create new SQLite database files, create tables, ingest data
into SQLite and make other modifications"*):

```sql
CREATE TABLE games AS SELECT * FROM read_json('build/identities.json');
COPY games TO 'identities.parquet' (FORMAT PARQUET, COMPRESSION ZSTD);
ATTACH 'identities.sqlite' AS out (TYPE sqlite);
CREATE TABLE out.games AS SELECT * FROM games;
```

**Licenses verified 2026-08-22 (confidence: high, read from the LICENSE
files):** DuckDB is MIT, `Copyright 2018-2026 Stichting DuckDB Foundation`;
`duckdb/duckdb-sqlite` is MIT, same foundation. And the runner only
*executes* the CLI, which is not distribution - the license would only start
mattering if DuckDB were linked into a shipped binary, which this design
never does.

**Client-side format: SQLite first, Parquet the intended destination.** The
daemon starts by reading the SQLite artifact via `rusqlite` (bundled, one C
file). This is an **interim**, recorded as such on the owner's correction of
2026-08-22: their intent from the start was Parquet on the client - structure
carried inside the file, densely compressed - and Claude drifted the design to
SQLite twice on a point-lookup argument that does not bite at this scale. At a
few thousand rows the daemon loads the whole file into a `HashMap` at startup
whichever format it is, so the real trade is the `parquet` crate's weight in the
daemon's dependency tree against Parquet's size and self-description. **Parquet
in the daemon is parked as a later feature**, not rejected.

The runner writes both regardless, so switching the daemon is a read-path change
with no data migration - the point of keeping the TOML pages as the source of
truth.

**DuckDB is not required anywhere.** The runner is a pure-Rust workspace member
(`parquet` crate, `rusqlite` bundled), so a clean machine builds every artifact
with `cargo run` and no system package. The owner has the DuckDB CLI at
`/home/florian/.duckdb/cli/latest/duckdb` (v1.5.5, verified 2026-08-22), which
earns a different job: **reference reader**, checking that Parquet written by
the Rust crate round-trips through the ecosystem's own implementation. Querying
the published Parquet over HTTP stays a consumer convenience they install for
themselves.

## Consequences

- **Artifact formats are never load-bearing.** The TOML pages are the source
  of truth, so adding Parquet, dropping it, or adding a format later is a
  change to the runner, never a data migration. This is the main dividend of
  splitting review format from consumption format.
- **The repo starts where umu-database's scope ends** - their database holds
  fix-needing games, their README sends everyone else to Lutris, and nothing
  covers the launcher-codename gap in between. A complement, not a
  competitor. It is also where drafted ids that cannot be submitted upstream
  (Control `umu-870780`, Project Hospital `umu-868360`) finally have a home
  instead of sitting in a local stash.
- **A public repo carries an obligation** to triage issues and PRs. Our
  current delta is small: 20 lines of `shared-helpers.txt`, one known-bad
  Discord row (Spellcraft / UnityCrashHandler64), a handful of confirmed
  codename identities.
- **Sequencing, as it happened:** the data layout and the lint were built
  *inside* gamebus-presenced first, then split out with `git subtree split`
  on 2026-08-23 once the shape, the license and the tooling were settled -
  earlier than the "second consumer" trigger first proposed, at the owner's
  call. The in-tree phase did what it was for: every design reversal (numeric
  ids, `year`, filename keys, the client format) happened before there was a
  public repo to carry the churn.

### 6. License: ODbL for the data, code licensed separately

**Decided 2026-08-22.** The data (`gamedb/games/*.toml`, `helpers.toml`) is
under the **Open Database License v1.0**; the lint, schemas and build scripts
stay under the project's code license. Full text vendored at `gamedb/LICENSE`
from https://opendatacommons.org/licenses/odbl/1-0/ (retrieved 2026-08-22, ten
sections, verified complete; should be diffed against canonical before the repo
goes public).

The owner's intent, verbatim (2026-08-22): *"use it, extend it, enrich it, but
share it if you are a project vendor"* and *"i don't care about attribution -
they dont even need to pr it back to me. since all of that is nonetheless public
knowlege, everyone should at least be on the same page."*

ODbL encodes exactly that, and the two halves are not in tension: **it never
routes anything to the licensor.** Its condition is only that a publicly used
adapted database is itself offered under ODbL - no pull request, no contact, no
permission. Share-alike attaches to the **database, not the application**, so a
launcher can ship proprietary or GPL code embedding this data and comply by
publishing its extended dataset.

- **GPL was rejected**: written around source code, object code and linking;
  "the corresponding source" of a TOML data set has no good answer, and the
  ambiguity costs adoption without buying reciprocity.
- **Attribution kept rather than engineered around**: no mainstream
  share-alike-without-attribution data license exists (CC has no SA without BY),
  and a hand-rolled "ODbL minus section 4.3" turns a license reviewers know into
  one they must read. The clause is a notice, not a credit screen, and the
  licensor is free never to enforce it.
- **PDDL/ODC-BY remain the escape hatch** - both strictly more permissive, and a
  license can always be relaxed later, never tightened.
- **Enforceability is partial and that is fine**: facts are not copyrightable;
  the EU sui generis database right plausibly covers curation of this kind, the
  US (Feist) does not, so protection rests on selection, arrangement and the
  notes. ODbL functions here as a strong norm with partial legal teeth.
- **The license is not the lever for structural convergence.** What gets other
  projects onto this shape is the schema, stable ids, and artifacts in formats
  they already read.

## Alternatives rejected

- **Fork a catalog** (Discord's or Lutris's): inherits maintenance of data we
  did not learn, to carry a delta. See Context.
- **A single CSV like umu-database**: write contention on every PR; measured
  drift shows what it looks like without a validation gate.
- **Delta Lake instead of plain Parquet**: exists to give ACID and time
  travel over object storage with concurrent writers. Git already provides
  both, more legibly. Adding it means a `_delta_log/` growing a JSON
  transaction file per write, in a repo whose transaction log *is* the commit
  history.
- **One file per store entry**: hides the cross-store identity claim, which
  is the thing worth reviewing.
- **Parquet in the daemon**: arrow/parquet crates are a heavy dependency for
  a few thousand rows that load into a `HashMap` in milliseconds.
- **An API with a moderation queue**: a project of its own. The data is
  reviewable as a diff, which is exactly why the PR model transfers.

## Open (TBD)

- Repo host: GitHub for contributor reach (that is where umu, Heroic and Lutris
  people are), gitea mirror per the owner's usual setup. **Name settled
  2026-08-22: gamebus-gamedb.**
- Whether the daemon eventually reads identities from the published artifact at
  all, or keeps its per-source resolution and uses this data only for
  corrections. Format for that path is already decided: Parquet eventually,
  SQLite as the interim.
- Whether icons and cover art references belong in the same pages - see
  [[wiki/concepts/Richer Presence Over the Bus]].

## Related

- [[wiki/projects/gamebus-presenced]]
- [[wiki/concepts/Game Identity Data Sources]] (why no upstream takes this)
- [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]] (the scope rule
  and the gate committed as `e41d657`)
- [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]] (the
  annotation half these corrections would eventually feed)
- [[wiki/concepts/Richer Presence Over the Bus]]

## Sources

- umu-database (single-CSV shape, scope paragraph):
  https://github.com/Open-Wine-Components/umu-database
- umu-protonfixes (488 files, one per game; store directory vocabulary):
  https://github.com/Open-Wine-Components/umu-protonfixes
- DuckDB LICENSE (MIT, 2018-2026 Stichting DuckDB Foundation):
  https://raw.githubusercontent.com/duckdb/duckdb/main/LICENSE
- DuckDB SQLite extension LICENSE (MIT):
  https://raw.githubusercontent.com/duckdb/duckdb-sqlite/main/LICENSE
- DuckDB SQLite extension write support:
  https://duckdb.org/docs/current/core_extensions/sqlite.html
