---
type: design
date: 2026-08-24
tags: [design, gamebus-presenced, gamebus-gamedb, identity, lutris, stash]
related-projects: [gamebus-presenced]
confidence: high
ai-first: true
status: implemented on feat/identity-misses (through a8d3358, 2026-08-30); one further finding recorded in ADR-009's amendments
---

# Identity Misses: widening the stash from umu misses to every non-obvious identity

## For future Claude

Design for the owner's direction of 2026-08-23/24: *"gamedb should not just
record umu-misses - it should also make the gamebus data more reliable:
therefore mapping it out when it is not obvious would make sense too."*
Produced by an ultracode workflow (4 subsystem readers, 3 competing designs,
3 judges, 1 synthesis; scores 23.5 / 18 / 15 for minimal-widen /
identity-first / launcher-truth). The synthesis below is the winner with the
judges' grafts. **One amendment by Claude after measuring the live machine**
(section 0) supersedes the design's reliance on `.lutrisgame.json`. Triggering
case: [[wiki/logs/2026-08-23 - gamebus-gamedb Published and the Export Loop]]
(Danger Scavenger, native itch.io via Lutris, unrecorded because the stash
trigger was the umu marker only). Contract: [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]].

## 0. Amendment (measured 2026-08-24, confidence high)

The design assumes Lutris writes `<GAME_DIRECTORY>/.lutrisgame.json` with
`service`/`appid` beside each game. **Measured: only itch.io installs carry
it** - 2 files on the whole machine, 0 of 29 GOG, 0 of 18 EGS, 0 of 6
Battle.net rows. Lutris's own database, however, knows the store codename for
most of them: `~/.var/app/net.lutris.Lutris/data/lutris/pga.db` table `games`
has `service`/`service_id` = Control egs/Calluna AND gog/2049187585, Cold War
battlenet/zeus, Amnesia gog/1186009992, Danger Scavenger itchio/926077. The
`directory` column is too sparse to join on (23 of 260 service rows, and for
EGS it is the store root). **The join that works is `(GAME_NAME, STORE)` from
the process environment == `(name, service)` in pga.db**, exact string match,
with service normalised (`ea_app`->`ea`, `steamwindows`->`steam`,
`flathub`->none). So: the daemon records only what the environment gives it
for free (no SQLite in the daemon, network-free, no new dependency); the
setup tool gains a Lutris library source (`src/setup/lutris_library.rs`, the
Heroic one's shape) reading pga.db via `rusqlite` (optional under the
`setup` feature; already in Cargo.lock through gamedb-build) and fills
`codename` with `codename_source = "lutris-library"`, never over a user
override. The marker file stays a cheap daemon-side bonus where present.

Owner questions the design left open, decided as routine by Claude:
launcher-keyed launches are recorded even when detectable.json named them
(exe provenance feeds gamedb enhancement; the setup tool hides no-gap
entries); the on-disk file keeps its name `umu-misses.json` this release
(both binaries install together, a rename is cheap later); the proposed
Lutris yml join is superseded by the pga.db join above.

## 1. The synthesis (verbatim from the workflow)

# Final design: identity misses for gamedb

## Decision

Start from the minimal-widen shape: the daemon's single launch hook stops asking "did umu miss this" and starts asking "did a launcher hand us a process without a store identity". It records launcher-keyed launches (Lutris and Heroic) in the existing stash, reading only facts Lutris and Heroic pin to the launch itself: three environ variables Lutris already sets on every process of the tree and one small JSON file Lutris writes beside the game. No SQLite, no YAML crate, no network, no new dependency. `umu_id` stays a plain `String` on disk (empty for non-umu entries) so an old daemon or setup binary keeps parsing new files; every new field goes only into the daemon-adopted block of the two-writer merge, so ADR-009 holds unchanged.

Three grafts resolve what the reviewers flagged. First, the trigger is narrowed to `lutris:` and `heroic:` keys plus the umu marker, not "every non-Steam key", so `umu:<id>` launches that already carry a curated umu identity do not flood the stash. Second, `game_exe` in the gamedb fold becomes store-aware: the `.exe` rule stays for umu/Proton entries (wine64-preloader and wine are not in the wrapper list and must never become page exes), and is relaxed only for native-runner entries. Third, "miss" becomes a setup-side predicate: `is_umu_miss()` is the single gate for the umu pipeline, and a `gamedb gap` predicate against `GamedbIndex` decides what the gamedb tab shows, so the stash can be a record of launches while the tabs show only gaps. A page note naming the join and a CONTRIBUTING rule for itch codenames come from the launcher-truth approach.

## Definition of an identity miss

A GameMode-grouped launch whose merge key is `lutris:<uuid>` or `heroic:<app>`, or whose environ carries the umu marker (`GAMEID`/`UMU_ID` of `umu-0` or `umu-default`). Steam-keyed launches (`steam:<appid>`) and curated umu launches (`umu:<id>` without marker) never enter the stash: their identity is authoritative.

Each stash entry carries two orthogonal facts:
- `umu_id` non-empty: also a umu-database miss, subject to the existing scope gate (protonfix needed).
- store identity (`store` + `codename`) present or absent: whether a launcher's own record named the game.

Predicates evaluated by gamebus-setup, never by the daemon:
- umu miss: `!umu_id.is_empty()`.
- gamedb gap: the entry's identifiers (`steam-<appid>`, `<store>-<codename>`, `exe:<basename>`) are not all known to `GamedbIndex`, and `title_source != "detectable"`.
- weak identity: `confidence != high` or `title_source` in `wrapper-layer`, `lutris-wrapper`.

Danger Scavenger: key `lutris:<uuid>`, no marker, `STORE=itchio`, `GAME_NAME=Danger Scavenger`, `GAME_DIRECTORY=/media/Data/Spiele/itchio/danger-scavenger`, `.lutrisgame.json` gives `service=itchio, appid=926077`. Entry keyed `itchio:926077`, `umu_id=""`, title from lutris-wrapper argv, exe `Danger_Scavenger.x86_64`. umu miss: no. gamedb gap: yes. Heroic EGS with `GAMEID=umu-0`: unchanged, `umu_id="umu-0"`, store/codename from `HEROIC_APP_SOURCE`/`HEROIC_APP_NAME`. Lutris Wine game with `GAMEID=umu-default` (Control): umu miss as today, now also `STORE=gog` and codename from `.lutrisgame.json` when present, so eight per-uuid entries collapse to one `gog:2049187585`.

## Data model (`src/umu_report.rs`)

Resolution half (daemon-owned). Existing fields unchanged except:
- `umu_id: String` gains `#[serde(default)]`. Empty means "not a umu miss". Always serialised, so old binaries parse new files. Add `Miss::is_umu_miss(&self) -> bool`.

New resolution fields, all `#[serde(default, skip_serializing_if = "Option::is_none")]`:
- `launcher: Option<String>`: `"lutris"` or `"heroic"`, from the key prefix.
- `launcher_name: Option<String>`: `GAME_NAME` (Lutris) or `HEROIC_APP_NAME`.
- `launcher_dir: Option<String>`: `GAME_DIRECTORY`.
- `codename_source: Option<String>`: `"lutris-config"` (from `.lutrisgame.json`), `"heroic-env"` (from `HEROIC_APP_NAME`), else None.
- `runner: Option<String>`: `"native"` when raw_exe is not under a Wine prefix and no umu marker, `"proton"` otherwise. Drives `game_exe`.

`title_source` gains the live value `lutris-wrapper` (already documented at umu_report.rs:60, never emitted today).

`store` vocabulary widens to umu spelling for all schema stores. `guess_store` gains a first branch: `STORE=` environ value normalised (`ea_app -> ea`, `humblebundle -> humble`, `steamwindows -> steam`, `flathub`/empty/unknown -> `none`); `HEROIC_APP_SOURCE` and path sniffing remain fallbacks.

Two-writer rules: the five new fields are appended to the daemon-adopted-from-disk list (`merge_from_disk`, umu_report.rs:437-445). The annotation list (:449-456) is untouched. None of the new fields is user-editable; existing overrides cover store/codename/title. `entry_key` unchanged; callers supply a stable fallback (below).

Compat: files without the new fields load via defaults; old test `a_pre_s9b_stash_still_loads` passes; a new test loads an entry with `umu_id: ""`.

## Daemon changes (`src/enricher.rs`)

Constraints: the daemon stays network-free (all reads under `$HOME` via `std::fs` and `serde_json`, both already in tree) and the dependency tree is unchanged. pga.db is not read: it would need rusqlite in the daemon, and the environ plus `.lutrisgame.json` carry every field the page needs.

Write point: the launch hook at enricher.rs:311-319 stays the single write point. Guard becomes:

```
let umu_id = umu_miss_id(&environ).unwrap_or("");
let launcher_keyed = key.starts_with("lutris:") || key.starts_with("heroic:");
if !umu_id.is_empty() || launcher_keyed { ... }
```

Inside, in order:
1. Read `STORE`, `GAME_NAME`, `GAME_DIRECTORY`, `HEROIC_APP_SOURCE`, `HEROIC_APP_NAME` via `env_value`.
2. `lutris_game_marker(dir) -> Option<(service, appid)>`: only when key is `lutris:` and `GAME_DIRECTORY` set; reads `<dir>/.lutrisgame.json`, serde_json, fields `service` and `appid`. Missing or malformed file is a silent None. Same precedent as `heroic_title`.
3. Store = `guess_store(STORE, HEROIC_APP_SOURCE, raw_exe)`; codename = marker appid (`codename_source = lutris-config`) else `HEROIC_APP_NAME` (`heroic-env`).
4. Stable fallback key: `lutris:<slug(GAME_NAME)>` when the name is present; `heroic:<app>` as today; the per-launch uuid only when no name exists. Never `exe:<basename>` in the daemon (Wine launches would all collapse onto `wine64-preloader`).
5. `note_launch(store, codename, umu_id, fallback, launcher facts)`; insert `(store, codename, fallback)` into the map renamed `stash_keys` so `note_title` and `note_group_identity` (:332, :568, :791, :1163) recompute the same `entry_key`.

Identity label: `Identity` gains `source: IdentitySource { Curated, Walk, LutrisArgv }`; `classify_member` sets `LutrisArgv` on the `identify_via_lutris_ancestor` path (:474-489, :1182-1187). `note_group_identity` maps `GameProcess -> detectable/High`, `Wrapper+LutrisArgv -> lutris-wrapper/Medium`, `Wrapper -> wrapper-layer/Medium`.

Join from pid, exactly: `/proc/<pid>/environ` -> `STORE`, `GAME_NAME`, `GAME_DIRECTORY` (Lutris game.py:692-699 sets all three per launch); `<GAME_DIRECTORY>/.lutrisgame.json` -> `service`, `appid`; title from lutris-wrapper argv (existing layer 4). Heroic unchanged. The `LUTRIS_GAME_UUID` is per-launch and stored nowhere by Lutris; it remains the merge key for grouping but is never the stash key when a name or codename exists.

Cost: one extra file read and one persist per launcher-keyed launch, which the umu path already pays.

## Setup-tool changes

umu pipeline (`src/setup/umu_misses/`): `verify`, `draft`, `FixCheck`, `check_open_prs`, `export::partition`, CSV/MD writers, and list counts iterate `entries.filter(is_umu_miss)`. Non-umu entries are never API-checked, drafted a `umu-` id, or scope-gated. `list()` prints `-` for empty `umu_id`. `KNOWN_STORES` gains `itchio`, `battlenet`, `ea`, `steam`. Online lookup keeps refusing non-gog/egs. Empty-state and hint strings say "identity misses" and mention Lutris/Heroic launches.

gamedb fold (`src/setup/gamedb/pages.rs`):
- `candidates` computes the gamedb gap predicate against `GamedbIndex`; entries known to the index are `InGamedb`/`Enhance` as today, curated-only entries are hidden by default.
- `source_of`: `codename_source` first (`lutris-config -> lutris`, `heroic-env -> heroic-config`), then `title_source` (`lutris-wrapper -> lutris`, `heroic-config`, `heroic-library`, `detectable` verbatim, rest `manual`).
- `confidence_of` for the store entry: `high` when `codename_source` is `lutris-config` or `heroic-env`; otherwise the daemon's title confidence; `lutris-wrapper`-only titles are forced to `medium`.
- `game_exe(path, runner)`: when `runner == proton` or `is_umu_miss()`, keep the `.exe` rule; otherwise accept any basename that is not `is_shared_helper` and not `is_wrapper_executable` (move that function to `src/naming.rs` so both binaries share it). Existing `.exe` pin test stays for proton entries.
- Note: umu-derived note only when `is_umu_miss()`; Lutris-resolved entries get `Identified from Lutris's .lutrisgame.json: service=<s>, appid=<a>`.
- Group key and canonical id already handle `itchio` (`ID_STORES` contains it).

TUI (`src/setup/ui/`): misses tab shows non-umu rows with glyph `g` and excludes them from "Verified N". gamedb tab gains a filter cycling all / gaps / umu / weak. Detail view adds `Launcher: lutris Danger Scavenger (itchio/926077)` from the launcher fields.

gamedb submodule: add a CONTRIBUTING line under `[[stores.<store>]]`: an `itchio` codename is the itch.io game id (Lutris `service_id`), not the Lutris slug. No schema or lint change is needed.

## The Danger Scavenger page

File `gamedb/games/danger-scavenger.toml`, canonical id `itchio-926077` (derived; `gamedb` line optional, emitted for clarity):

```toml
title = "Danger Scavenger"
gamedb = "itchio-926077"
note = "Identified from Lutris's .lutrisgame.json: service=itchio, appid=926077."

[[stores.itchio]]
codename = "926077"
exe = "Danger_Scavenger.x86_64"
seen = "2026-08-23"
source = "lutris"
confidence = "high"
```

Lints clean with `.scripts/gamedb-lint.py`; no `[ids]` block (nothing to put there).

## Test plan (all offline)

Unit, `umu_report.rs`: pre-S9b file loads; entry with `umu_id: ""` round-trips; new fields survive an annotator persist and an annotator-side field survives a daemon persist; `guess_store` normalises every Lutris spelling; `is_umu_miss`.

Unit, `enricher.rs` (fixture environ + tempdir with `.lutrisgame.json`): Danger Scavenger fixture writes `itchio:926077` with `lutris-wrapper`/Medium, exe, `codename_source=lutris-config`, `runner=native`, `umu_id=""`; Heroic `umu-0` fixture unchanged byte-for-byte; Lutris umu-default with marker file collapses to one `gog:<id>` over repeated uuids; Lutris launch with name but no marker keys `lutris:<slug>`; `steam:` and unmarked `umu:` keys never record; malformed marker file yields no codename and no panic.

Unit, setup: verify/partition/draft skip non-umu entries; `KNOWN_STORES` cycles to itchio; `game_exe` accepts `Danger_Scavenger.x86_64` for native, rejects it for proton, rejects `wine64-preloader` and `python3.13` for both; `source_of`/`confidence_of` tables; gap predicate against a fixture index.

Integration: the fixture stash folds to the page above byte-exact and passes `.scripts/gamedb-lint.py` in a scratchpad copy of the data set; add it to `.scripts/gamedb-lint-fixtures/` and update `expected.txt`.

## Work split

Task A (daemon), owns `src/umu_report.rs` (fields, merge lists, `guess_store`, `is_umu_miss`), `src/enricher.rs`, `src/naming.rs` (relocated `is_wrapper_executable`). Task B and C consume the field names above as a frozen contract.

Task B (umu pipeline and misses TUI), owns `src/setup/umu_misses/*`, `src/setup/ui/misses.rs`, `src/setup/ui/mod.rs` strings. Depends only on `is_umu_miss()` and the new field names.

Task C (gamedb fold and gamedb TUI), owns `src/setup/gamedb/*`, `src/setup/ui/gamedb.rs`, `gamedb/CONTRIBUTING.md`, `.scripts/gamedb-lint-fixtures/`, ADR-009 amendment 3 in `.second-brain`.

To avoid a merge conflict, Task A lands the `umu_report.rs` field additions and the `is_wrapper_executable` move first as a small base commit the other worktrees branch from; the rest of A proceeds in parallel.

## Open questions for the owner

1. Should launcher-keyed launches that detectable.json already names (curated title, no gap) still write a stash entry, or be skipped at the daemon? The design records them and hides them in setup; skipping saves stash growth but loses the exe provenance.
2. Is the optional setup-side yml join (for Lutris installs without a `write_json` step, e.g. GOG/EGS through Lutris) wanted as a follow-up, given the yml is not flat (`exe` nested under `game:`) and needs a two-level line matcher?
3. Keep the file name `umu-misses.json`, or rename to `identity-misses.json` with a one-release read-both shim?
