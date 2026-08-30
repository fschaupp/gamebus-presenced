---
date: 2026-08-22
type: concept
topic: every data source that can identify a running game, what it holds, what it accepts, and which ones carry artwork
tags: [concept, game-identity, detectable-json, umu-database, umu-protonfixes, lutris, artwork, data-sources, gamebus-presenced]
related-projects: [gamebus-presenced]
sources:
  - https://discord.com/api/v9/applications/detectable
  - https://github.com/Open-Wine-Components/umu-database
  - https://umu.openwinecomponents.org/umu_api.php
  - https://github.com/Open-Wine-Components/umu-protonfixes
  - https://lutris.net/api/games?format=json
  - https://catalog.gog.com/v1/catalog
  - https://api.egdata.app/multisearch/offers
  - https://www.gogdb.org
  - https://egdata.app
confidence: high
ai-first: true
---

## For future Claude

A survey of **every data source [[wiki/projects/gamebus-presenced]] can identify a
running game from** - what each one holds, what it accepts as a contribution, and
which ones carry artwork. Measured live on 2026-08-22 unless a claim says
otherwise. Two questions this note is built to answer fast: *"where do I find X?"*
(§ the lookup table, then the per-source sections) and *"can we put icons on the
bus?"* (yes - [[#1. Discord detectable.json]] already carries them; the daemon
needs no new data source). The conclusion the survey forces:
**there is no universal game-identity database, and no upstream accepts what this
project learns** - so the shape to build is a corrections *overlay*, never a fork.

## The lookup table

| I need… | Source | Bulk? | Network at runtime? |
|---|---|---|---|
| exe path → game name | Discord `detectable.json` | yes, one file | no (fetched once) |
| exe → Steam appid | Discord `detectable.json` (`third_party_skus`) | yes | no |
| **icon / cover art** | Discord `detectable.json` + CDN | yes (hashes), CDN per image | CDN fetch only |
| cover art fallback | Lutris `coverart` (IGDB) | yes, 347k rows | yes |
| title → Steam appid (2nd opinion) | Lutris detail endpoint | list is bulk, detail per-slug | yes |
| `(store, codename)` → umu id | umu-database | yes, full dump | yes |
| does this game need a Proton fix? | umu-protonfixes tree | yes, one request | yes |
| EGS codename | egdata.app Builds "App Name" | no, per-query | yes |
| GOG codename | gogdb.org numeric product id | no, per-query | yes |
| what the user actually owns | Heroic / Lutris local libraries | local files | **no** |
| exe → identity when the exe names no game | *nothing upstream* | - | - |

---

## 1. Discord detectable.json

**Endpoint:** https://discord.com/api/v9/applications/detectable - already wired as
`discord.detectable` in the project's `endpoints.toml`. `gamebus-presence
fetch-detectable` downloads it, and the setup tool runs that as the last install
step.

**Size (as of 2026-08-22, confidence: high - measured):** 23,900 entries. **18,358
of them (77%)** carry a `third_party_skus` entry with `distributor: "steam"`, i.e.
a Steam appid. That 77% is what the miss pipeline drafts umu ids from.

**Keying:** executable names, optionally with a path suffix. Control's entry lists
`control/control_dx11.exe`, `control_dx11.exe`, `control_dx12.exe`. The matching
rule is a path-*suffix* match, not a basename lookup - see
[[wiki/concepts/2026-08-04 - detectable-json-path-suffix-matching]] for the
algorithm and the collision cases; do not re-derive it here.

### Artwork - the important find

- **19,752 entries carry `icon_hash`; 16,758 carry `cover_image_hash`**
  (as of 2026-08-22, confidence: high - measured).
- The CDN URL is **constructible from the entry's own `id`** (the Discord
  application id) plus the hash. Verified live 2026-08-22:
  https://cdn.discordapp.com/app-icons/1402417032258916553/6ce4ec16083fc0b2a5c778dece82b3fc.png
  answered HTTP 200, `image/png`, 8,095 bytes (Control's icon). The
  `cover_image_hash` on the same path answered 200, 25,519 bytes.
- **Meaning:** icons and cover art over the D-Bus need **no new data source**. The
  file the daemon already resolves identities from carries the art reference for
  roughly **83%** of entries. Confidence: high (measured).

### Known data bug

Exactly **one** of the 23,900 entries names a UnityCrashHandler: Discord's own row
for **"Spellcraft"**, executable `spellcraft dev (staging)/unitycrashhandler64.exe`.
That single wrong upstream row is the whole cause of the Project Hospital mislabel
this project defended against with `shared-helpers.txt` - see
[[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]]
and [[wiki/concepts/Elected State Beats the Event Stream]]. One bad row in 23,900,
and there is no way to fix it upstream, which is the next point.

### Contribution path

**None public.** No repository, no pull requests. Corrections would have to go
through Discord's developer/support channels, which are aimed at a *game's own
developers*. Confidence: medium (not verified this session).

---

## 2. umu-database

**Repo:** https://github.com/Open-Wine-Components/umu-database ·
**API:** https://umu.openwinecomponents.org/umu_api.php (bare endpoint = full dump;
`?store=X&codename=Y`, `?title=T`, `?umu_id=X` also supported).

**Holds:** `(store, codename) -> umu_id + title`.

**Size (as of 2026-08-22, confidence: high - measured):** live full dump **1,200
entries**. The local checkout's CSV has **1,202 rows / 1,100 distinct umu ids** -
it carries the owner's own uncommitted Control rows.

### Scope rule - it is not a catalog and never will be

Verbatim from the README's opening (as of 2026-08-22, confidence: stated):

> This database is by no means a complete database of every game released on
> Windows. We focus on games that requires fixes in Proton. Games that run out of
> the box have no need be added to the database. If you want a more extensive
> database of games you can use the Lutris API.

The umu team confirmed this to the owner directly on 2026-08-22 (confidence:
stated). Consequence: **umu-database answers "which games need a Proton fix", not
"which games exist"**. See
[[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]] for the session that
built this gate into the export path.

### Codename rules

- **EGS** = the Builds **"App Name"** from https://egdata.app (e.g. `Calluna`,
  `Catnip`) - **not** the offer page's lowercase Namespace.
- **GOG** = the **numeric product id** from https://www.gogdb.org.
- A **numeric second part** of a umu id is passed to Proton as a `SteamAppId`, so
  **non-Steam ids must contain a letter**.

### Contribution path

GitHub pull requests against `umu-database.csv`. **Real but narrow** - only games
that need a fix.

---

## 3. umu-protonfixes

**Repo:** https://github.com/Open-Wine-Components/umu-protonfixes (default branch
`master`). The whole file list comes back in **one request** via
https://api.github.com/repos/Open-Wine-Components/umu-protonfixes/git/trees/master?recursive=1

**Size (as of 2026-08-22, confidence: high - measured):** **488 files** under
`gamefixes-*`, serving **377 distinct umu ids**.

- Naming: `gamefixes-steam/<appid>.py`, `gamefixes-<store>/umu-<id>.py`.
- **113 of the files are symlinks** (git mode `120000`) - the "point the other
  store's copy at the Steam fix" mechanism that a umu-database row unlocks.
- Per directory: steam 313, gog 79, umu 37, egs 29, ubisoft 9, zoomplatform 9,
  humble 3, itchio 3, amazon 2, battlenet 2, ea 2.

**Why it matters:** this list is what decides whether a game is **in scope for
umu-database at all**. The project now gates its exports on it.

---

## 4. Lutris API

**Endpoint:** https://lutris.net/api/games?format=json - **~347,657 games** as of
2026-08-22 (confidence: high - measured). The umu README points here for "a more
extensive database of games".

**List endpoint returns:** `aliases`, `banner_url`, `change_for`, `coverart`,
`discord_id`, `icon_url`, `id`, `name`, `platforms`, `provider_games`, `shaders`,
`slug`, `year`. `provider_games` was **empty** for Control.

**Detail endpoint** `https://lutris.net/api/games/<slug>?format=json`
additionally carries `steamid`, `gogslug`, `humblestoreid`, description, genres,
installers, `is_public`, `updated`, `user_count`. Verified 2026-08-22:

| Game | steamid | other |
|---|---|---|
| Control | 870780 | gogslug `control_ultimate_edition`, discord_id `""`, user_count 4,399, updated 2026-08-17 |
| Borderlands 3 | 397540 | - |
| Project Hospital | 868360 | - |
| Cyberpunk 2077 | 1091500 | gogslug `cyberpunk_2077` |

### Limits (all measured 2026-08-22, confidence: high)

- **No EGS field at all.**
- GOG is a **slug**, not the numeric gogdb product id the umu database wants.
- `discord_id` was **empty on all four games sampled**.
- **Cannot be queried by store id** - `?steamid=870780` is silently ignored and
  returns the full 347,657-row list.

So it resolves **title → Steam appid, and nothing else the project needs**.

### Artwork

`coverart` points at IGDB images, e.g. https://lutris.net/media/igdb/cover_big/co267h.jpg
answered HTTP 200, `image/jpeg`, 14,950 bytes (2026-08-22). A plausible **fallback
for games `detectable.json` has no art for**. Confidence: high (measured) for
availability; **licensing for redistribution is TBD**.

### Contribution path

The API is **read-only**. The public surfaces are a moderated **web submission** for
game entries and community-written **install scripts** (Control has 3 installers,
e.g. one by user `fifayReal` described "GOG + HDR + Ultrawide + DLSS + RT Patch",
created 2023-08-15). There is **no field for a store-codename identity**, so what
this project learns cannot be contributed here.

### Use it as a second opinion

Its `steamid` values matched the project's `detectable.json`-derived drafts
**exactly** on all three games checked (870780, 397540, 868360). Two independent
sources agreeing on an appid is a stronger claim than one - directly relevant to
the **firm-vs-guessed** distinction the scope check now draws (see
[[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]] § "The honesty split").

---

## 5. Store APIs (already wired in endpoints.toml)

- **GOG:** catalog search https://catalog.gog.com/v1/catalog and per-product
  `https://api.gog.com/products/<id>`. **gogdb.org product pages are the
  codename authority.**
- **egdata.app:** offer search https://api.egdata.app/multisearch/offers and
  per-namespace builds `https://api.egdata.app/sandboxes/<namespace>/builds` -
  the Builds **"App Name"** is the codename.

Both are **per-query, network, no bulk dump**. Used by the misses pane's one-shot
`o` lookup.

---

## 6. Local launcher libraries (zero network, per-machine)

Authoritative for **what the user actually owns**:

- **Heroic** `store_cache`: `legendary_library.json` and `gog_library.json`
  (flatpak path under `~/.var/app/com.heroicgameslauncher.hgl/`).
- **Lutris runtime:** `umu-games.json` under the Lutris data dir - matched
  **case-sensitively** on store + appid.

These are the **only** source that ties an installed copy to a store codename with
certainty, and they exist **only on the machine that owns the game**.

---

## The conclusion

**There is no universal game-identity database, and no upstream accepts the
knowledge this project produces.** Each source is scoped to its own purpose:

- Discord's is **executable-keyed and closed to contributions**.
- umu's is **deliberately limited to games needing fixes**.
- Lutris's is a **title/artwork catalog with no store-codename schema and a
  read-only API**.

The project's unique knowledge is a **delta** against those sources:

1. Executables that never name a game (wrappers, crash handlers, launchers).
2. Human-confirmed `(store, codename) -> identity` pairs.
3. Wrapper-layer **election** knowledge (see
   [[wiki/concepts/Elected State Beats the Event Stream]]).
4. Store mappings for games that **need no fix** - precisely what umu-database
   rejects.

None of it has anywhere upstream to live. **So the shape to build is a corrections
OVERLAY** - layered exactly like the existing `shared-helpers.txt`: bundled, then
installed data dir, then user config, **union-merged**. Never a fork of anyone's
catalog: a fork inherits a maintenance treadmill and rots, while a delta of
corrections stays small and stays true. **Every row in such an overlay should carry
its own provenance** (what confirmed it, on what evidence, when).

Confidence: **high** for the survey facts above (all measured 2026-08-22);
**medium** for the overlay recommendation - it is a design judgement, not a
measurement.

## Related

- [[wiki/projects/gamebus-presenced]]
- [[wiki/concepts/2026-08-04 - detectable-json-path-suffix-matching]] - how
  `detectable.json` is actually matched; do not duplicate that algorithm here.
- [[wiki/concepts/Elected State Beats the Event Stream]] - the mislabel this
  survey's known-bad Discord row caused.
- [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]] - the session that
  measured umu-protonfixes and built the scope gate.
- [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]] - what
  this survey's "no upstream accepts our delta" finding turned into: a PR-based
  repo of per-game TOML pages with built artifacts, decided 2026-08-22.
- [[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]]
  - `shared-helpers.txt`, the layering the overlay should copy.
- [[wiki/logs/2026-08-07 - gamebus-presenced S9b umu Contribution Pipeline]] - the
  export path these sources feed.
- [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]] - where
  human-confirmed identities are stored today.
