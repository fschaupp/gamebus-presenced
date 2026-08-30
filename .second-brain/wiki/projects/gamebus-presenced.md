---
date: 2026-08-04
type: project
status: in-progress
updated: 2026-08-23
tags: [project, rust, d-bus, discord, gamemode]
related-people: []
related-projects: []
job: Personal
repo: /media/Data/Projekte/gamebus-presenced
graduated-from: ""
ai-first: true
---

## For future Claude

`gamebus-presenced` is a Rust project implementing a unified "what is this machine playing" presence on the Linux session bus. It collects fragments from multiple sources (GameMode for pid/executable, Discord IPC for title/chapter text, Steam for appid) and correlates them by pid into a single activity record published via D-Bus interface `org.gamebus.Presence.v1`. S0 (D-Bus surface), S1 (GameMode source), S2 (Discord IPC listener), S3 (proxy + correlator + restart cache), S4a (Steam enrichment via Enricher middleware), S4b (naming via detectable.json), S4c (packaging: CLI + systemd), S4d (ancestor-walk join), S4e (game identification + scan), S4f (game groups, 2026-08-06), S6/S6b (MPRIS hints + Lutris wrapper titles, 2026-08-06), S7 (publication hygiene, 2026-08-06), S8 (Heroic detection, 2026-08-06), and S9 (umu-miss stash, 2026-08-07) are COMPLETE; S5 (gamebus-setup TUI) landed 2026-08-06, STAGED. S9 and S9b (umu-database contribution pipeline) landed on master 2026-08-07 via --no-ff merge 58afefd (branch `feat/s9-umu-miss-report` kept, 9 commits 4c8a16e through b638404; pushed to gitea). A publishable branch `public` now exists (2026-08-07 evening): 13 re-composed Apache-2.0-licensed feature commits, every one compiling, scrubbed of internal references - see [[wiki/logs/2026-08-07 - gamebus-presenced Public Branch Re-Composition]]. **v0.1.0 was RELEASED the same night** at https://github.com/fschaupp/gamebus-presenced via the S10 release pipeline (network-free builds, `.github/workflows/release.yml`, `.scripts/release.sh`); the history was then rewritten to carry `Assisted-by:` trailers + a README AI note (EU AI Act), so a one-time force-push + re-tag is pending - see [[wiki/logs/2026-08-07 - gamebus-presenced v0.1.0 Release]]. S9c (matchup workbench + mislabel defense) merged to master 2026-08-08 as --no-ff `fb8a702` (branch `fix/umu-export-conformance` kept), rolled out (reinstall + daemon restart) - see [[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]]. Since then: the umu scope gate reframed contributions (2026-08-22, [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]]) and the project's identity knowledge became **gamebus-gamedb**, its own published data repo with a full loop back into the daemon's tooling (2026-08-23, [[wiki/logs/2026-08-23 - gamebus-gamedb Published and the Export Loop]], [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]]). Authoritative detail for S4f/S5 is the repo (PLAN.md) until vault records exist. This note tracks the project's status, decisions, and recent activity.

## Overview

### The Problem
Every piece of the answer exists on a Linux desktop, but no two pieces are in the same place:
- **GameMode** (`com.feralinteractive.GameMode`) knows pid + executable, not the game's name
- **Steam** knows appid, nothing else
- **Discord Rich Presence** (`$XDG_RUNTIME_DIR/discord-ipc-0`) knows title, chapter text, elapsed time, artwork - but it's a write-only proprietary socket
- **Lutris/Heroic** publish nothing; they just forward to Discord

Today, a status bar, stream overlay, or desk pet needs four integrations to learn what a single game launch already told the system four times over.

### The Solution
One session-bus service that:
1. Collects every source independently
2. Correlates them by pid into a single activity record
3. Publishes on `org.gamebus.Presence.v1` the way MPRIS publishes media

### Architecture
```
Discord IPC listener → transparent proxy to real Discord (or standalone)
GameMode watcher → pid/executable signals
Steam probe → registry.vdf + /proc environ
         ↓ all feed into correlator
    D-Bus surface: org.gamebus.Presence.v1.Manager
```

## Status

**S0 through S9c on master; S5 landed (STAGED). v0.1.0 RELEASED 2026-08-07 (night) from branch `public` at https://github.com/fschaupp/gamebus-presenced - the GitHub workflow built, bundled, and published it. S10 (network-free builds + release workflow) merged to master 2026-08-07 late night as --no-ff `d6adb69` (branch `feat/release-pipeline` kept, trailers switched to Assisted-by, gate green). S9c (umu-export conformance + matchup workbench + mislabel defense) merged to master 2026-08-08 as --no-ff `fb8a702` (branch `fix/umu-export-conformance` kept, 9 commits, gate: 302 tests + clippy -D warnings + fmt), ROLLED OUT - reinstall + daemon restart, active. Known open item: a resolved umu id splits the wrapper/game group (`lutris:<uuid>` vs `steam:<appid>` → two records during play), future slice. Pending: force-push + re-tag of the Assisted-by-disclosed public history; master push to gitea (owner's call). (as of 2026-08-08)**

- Repo created: 2026-08-03
- Design doc: `docs/design/gamebus-presence.md` (complete)
- Roadmap: `PLAN.md` (S0-S4 done)
- S0: D-Bus surface foundation implemented and verified on session bus (2026-08-04)
- S1: GameMode source implemented and verified end-to-end on session bus (2026-08-04)
- S2: Discord IPC listener implemented and verified with a genuine discord-rich-presence client (2026-08-04)
- S3: proxy + correlator + restart cache implemented and verified (byte-identity vs fixture, same-pid join, SIGKILL/respawn re-adoption) (2026-08-04)
- S4a: Steam enrichment via Enricher middleware (/proc/<pid>/environ probe; reactive-only until S4e added the 15s bounded scan) (2026-08-04)
- S4b: naming enrichment via detectable.json (enrichment-only fallback, never overrides more authoritative source) (2026-08-04)
- S4c: packaging (gamebus-presence CLI with monitor + fetch-detectable, systemd user unit, D-Bus activation) (2026-08-04)
- S4d: ancestor-walk join for wrapper-tree dedup (ppid-chain walk, descendant absorbs ancestor) (2026-08-04)

## Slices (from PLAN.md)

### S0 - D-Bus surface foundation (COMPLETE)
Extracted from the original S1. D-Bus interface foundation: `org.gamebus.Presence.v1.Manager` with `ListActivities`, `HasActivity`, `Version` properties; `Activity` type; zbus v4 bindings; service verified on session bus.

### S1 - GameMode source (COMPLETE)
The `org.gamebus.Presence.v1` surface fed by the GameMode watcher: `GameRegistered`/`GameUnregistered` on `com.feralinteractive.GameMode`, seeded by `ListGames`. Per-game executable + registration timestamp resolved from `com.feralinteractive.GameMode.Game` object properties (`/proc/<pid>/exe` fallback). Per-activity objects at `.../Activity/pid_<pid>` with `ActivityAdded`/`ActivityRemoved` signals and `HasActivity` change emission; gamemoded availability tracked via `NameOwnerChanged`. Verified by integration test (`gamemoderun sleep 30`) and busctl acceptance.

### S2 - Discord IPC listener, no upstream (COMPLETE)
Standalone Discord Rich Presence listener on `discord-ipc-0` (stale-socket unlink, live-owner degrade). Handshake/READY, lock-step echo, `SET_ACTIVITY` parsed with the pinned `rsrpc` crate's `rsrpc::cmd` model (+ its `fix()` normalisation) and published as activity objects at `.../Activity/discord_<pid>`; pid from `SO_PEERCRED`, never client-supplied. `ActivityInterface` is now mutable (`PropertiesChanged` in place for mid-session updates); sources unified under one `SourceEvent` channel. Verified with a genuine discord-rich-presence client.

### S3 - Proxy + correlator (COMPLETE)
Transparent proxy to a real Discord on `ipc-1..9`: frames forwarded whole (byte-identity verified against a fixture), `SET_ACTIVITY` tapped passively, upstream loss closes the client connection (reconnect lands in standalone mode). Correlator in `src/correlator.rs`: per-source partials keyed by exact pid, published activity is a derived view; `pid_<pid>` absorbs `discord_<pid>` on a join; Discord's human-facing fields win, GameMode owns executable; degrade-in-place, die with last source. Restart cache at `$XDG_RUNTIME_DIR/gamebus-presenced/cache.json` keyed pid + `/proc/<pid>/stat` start-time; startup re-adoption before sources spawn. umu/Proton wrapper-tree join is a documented miss (S4 candidate: bounded ancestor-walk). See [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]].

### S4 - Enrichment and packaging (COMPLETE 2026-08-04)

Brainstormed via 6-question Socratic interview - see [[wiki/concepts/2026-08-04 - Brainstorm - S4 Enrichment and Packaging]].

**S4a - Steam source (reactive via Enricher middleware)** ✅
New `src/enricher.rs`: `Enricher` struct sits between sources and correlator (`sources → Enricher → Vec<SourceEvent> → Correlator`). On `SourceEvent::Updated` with a pid, probes `/proc/<pid>/environ` for `SteamAppId`/`SteamGameId`/`UMU_ID`/`STORE`; if found, emits `SourceEvent::Updated(Source::Steam)`. Steam partial removal tied to last non-Steam source (pid-reuse safe - Enricher tracks `{pid: Set<Source>}`). `Activity::from_steam(pid, appid)` maps to `app_ids["steam"]`. Correlator gains `steam` slot. `tracing::debug!` on join-miss for ancestor-walk decision. Integration test: `sleep` spawned with `SteamAppId=480`, registered via `RegisterGameByPID`, merged record carries `app_ids["steam"]`.

**S4b - Naming enrichment (enrichment-only fallback)** ✅
`build.rs` fetches `detectable.json` from Discord's `applications/detectable` endpoint (23858 entries, 12.3MB); ships as installation data file (`$PREFIX/share/gamebus-presenced/detectable.json`, not binary-embedded). `gamebus-presence fetch-detectable` CLI refreshes to `$XDG_CACHE_HOME`. Naming precedence: Discord name > detectable.json lookup > executable stem. Anti-goal: never overrides a more authoritative source. Naming DB loaded after sources spawn to avoid blocking the Discord listener.

**S4c - Packaging** ✅
`gamebus-presence` CLI binary (`src/bin/gamebus-presence.rs`): `monitor` pretty-prints bus state (activities, sources, names, appids); `fetch-detectable` downloads Discord's detectable.json to `$XDG_CACHE_HOME`. systemd user unit (`data/gamebus-presenced.service`) and D-Bus activation file (`data/org.gamebus.Presence.v1.service`) for session-start activation.

**S4d - Ancestor-walk join (wrapper-tree dedup)** ✅
Bounded ppid-chain walk (`MAX_ANCESTOR_DEPTH` = 10) for the umu/Proton wrapper-tree case. The Enricher tracks `{pid: steam_appid}` and, on a new Steam probe, checks if another tracked pid shares the same appid AND is in the same process tree. The descendant absorbs the ancestor. Three bugs found during live testing with Brotato: merge direction reversed, cache-adopted records couldn't be removed, `SteamAppId=default` false positive. See [[wiki/logs/2026-08-04 - gamebus-presenced S4]] for details.

**S4e - Game identification + Steam process scan** ✅
Live testing with Brotato, CoD: Black Ops Cold War, Amnesia: The Bunker, and Resident Evil 2 surfaced eleven gaps, all fixed. (1) detectable.json path-prefixed entries (83% of the DB) missed - basename-bucketed index with path-suffix matching + backslash normalisation. (2) Wrapper processes unidentified - three-layer identification: wrapper cmdline, connected descendant walk (exe + Wine cmdline), sandbox-family scan via the umu `var/tmp-XXXXXX` cmdline token (Flatpak-portal severs the tree). (3) Delayed game launches - `unresolved_wrappers` retried every 15s via a main-loop tick. (4) Games without GameMode invisible - bounded `/proc/*/environ` Steam-appid scan in the same tick, closing the S4a reactive-only gap proven real by Resident Evil 2 launched without gamemoderun. (5) Steam scan exploded into ~20 records - `identify_process` filter for utility processes, then **replaced pairwise ancestor-walk merge with `appid_records: HashMap<String, u32>`** (merge key → deepest pid, one record per key, `tree_depth` decides). (6) Merge key generalised beyond Steam - `steam:<appid>` / `lutris:<uuid>` / `umu:<id>`. (7) `SteamAppId=0` rejected. (8) Tracing filter overrode RUST_LOG. (9) Cache re-adopted stale Steam-only records. (10) Wrapper executables shown as game names. (11) Discord integration tests fail when real Discord is running. Final verification: 1 record for Amnesia: The Bunker, 1 record for Resident Evil 2. See [[wiki/logs/2026-08-04 - gamebus-presenced S4]].

## Key Decisions

### D-Bus Interface Design
- Bus name: `org.gamebus.Presence.v1` (the `.v1` spelling is deliberate for versioning)
- Root path: `/org/gamebus/Presence/v1`
- Manager interface: `org.gamebus.Presence.v1.Manager`
- Activity interface: `org.gamebus.Presence.v1.Activity`

See `docs/design/gamebus-presence.md` § The D-Bus surface for full details.

### Versioning Strategy
- Version is a separate element: `…Presence.v1`, not `…Presence1`
- Additive changes bump the `Version` property on Manager
- Incompatible changes earn a `.v2` name that runs beside `.v1`

### Correlator Design
- Sources joined by pid (Discord IPC gives pid via `SO_PEERCRED`, GameMode hands over `(pid, executable)`)
- With a pid, `/proc/<pid>/environ` yields `SteamAppId`, `/proc/<pid>/exe` the real binary
- One activity record can carry Discord's title/chapter, GameMode's executable, and Steam's appid
- Sources that cannot be tied to a pid publish lower-confidence records

### D-Bus Implementation Decisions
- **Library**: Use zbus v4 for D-Bus bindings with tokio runtime
- **Activity Type**: Use simplified Activity type without lifetime parameters to avoid zvariant::Value Clone issue
- **Manager Interface**: Includes `ListActivities` method, `HasActivity` and `Version` properties
- **Activity Interface**: Per-activity objects with properties exposed via D-Bus
- **Activity IDs**: `pid_<pid>` with underscore - hyphens are illegal in D-Bus object path elements (ADR-005)
- **GameMode details**: signals carry per-game object paths; executable/timestamp come from `com.feralinteractive.GameMode.Game` properties (ADR-005)

### Discord Proxy Strategy
- Bind `discord-ipc-0` at session start
- Discord starts later, finds 0 taken and binds ipc-1
- With Discord present: forward every frame verbatim upstream
- Without Discord: answer handshake and echo commands ourselves (arRPC behaviour)
- Unlink stale sockets on start; losing the race to Discord is a warning, never a crash

### Restart Survival
- Other sources (GameMode, Steam) re-derive from scratch on startup
- Runtime cache keyed by pid + process start-time from `/proc/<pid>/stat` (field 22)
- On startup, re-adopt records whose process is still alive AND start-time still matches

### S9/S9b umu-Miss Pipeline (2026-08-07)
- umu-miss stash is split into two writer-owned halves - daemon owns misses, setup tool owns annotations - killing the two-writer lost-update found in adversarial review. See [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]] (2026-08-07)
- umu API title lookup is substring-fuzzy → advisory-only, never a verdict (2026-08-07)
- User deletions are the annotation-half `dismissed` flag, never key removal (2026-08-07)
- All remote endpoints live in the shipped `endpoints.toml`; the daemon reads none of it (2026-08-07)

### Public Branch (2026-08-07)
- Public branch style rules: commit messages max 200 chars, no Co-Authored-By trailers, no em-dashes anywhere (code, docs, messages), no internal references (S-phase tags, spec § refs, ADR numbers, diary dates), sizeable feature commits, and every commit must pass `cargo check --all-targets`. See [[wiki/logs/2026-08-07 - gamebus-presenced Public Branch Re-Composition]] (2026-08-07)
- License: Apache-2.0 single license (was "MIT OR Apache-2.0 (proposed)"), canonical LICENSE file + NOTICE with "Copyright 2026 Florian Schaupp"; LICENSE appendix placeholders stay verbatim; no email in copyright lines (2026-08-07)
- Git author identity on the public branch stays the alias `fschaupp <spritzwine.absently488@passinbox.com>` - a deliberate privacy relay; copyright name and git identity are independent (2026-08-07)
- **`Assisted-by:` replaces `Co-Authored-By:` everywhere** (2026-08-07, EU AI Act transparency): Claude-assisted commits end with `Assisted-by: Claude Fable 5 <noreply@anthropic.com>`; on `public` all 17 commits carry it and the README discloses AI assistance in one line above "Prior art it stands on" (present from the first commit). Human-run release commits (`.scripts/release.sh`) carry no trailer. The tweet-size rule covers the prose, not the trailer. (2026-08-07)
- Releases are cut with `.scripts/release.sh` (gate → set version → `release: vX.Y.Z` commit → annotated tag; never pushes) and published by `.github/workflows/release.yml` on the pushed tag; builds are network-free since S10 - `detectable.json` is fetched at install/on demand, never at build time (2026-08-07)

### S9c Matchup Workbench + Mislabel Defense (2026-08-08)
- The umu-miss stash mirrors the group's ELECTED identity, never a member's claim - `note_group_identity` after every `set_identity` site; the Project Hospital/"Spellcraft" incident proved the event-stream hook wrong while the monotone group election was right (2026-08-08)
- The export checklist carries only human-verifiable boxes - never restates what code guarantees; the misread "Store ids are lowercase" box was dropped, the guarantee lives in a comment + inverted test asserts (2026-08-08)
- `shared-helpers.txt` layers with UNION semantics (bundled → installed → user config) - local files add shared-helper exe names, never remove them; installed beside `endpoints.toml` (2026-08-08)
- The egs codename is the sandbox Builds "App Name" (e.g. `Calluna`), never the egdata "Namespace" (`calluna`) - Lutris matches case-sensitively and DB precedent is capitalized; the workbench structurally never offers the namespace (2026-08-08)
- Process: never commit while exploring (subagents included); feature-branch commits stay docs-free - PLAN.md goes in one closing docs commit for public cherry-pickability (2026-08-08)

### gamebus-gamedb Publication and Export (2026-08-23)
- **Data releases are date-tagged** (`v2026.08.23`, `.N` suffix same-day): a data set has no API
  surface to be semantic about, and a date answers "how old is my copy?" at a glance. Daemon
  releases keep semver; the two never share numbering. (2026-08-23)
- **Tools are pinned by release tag AND sha256 digest** in the data repo's workflows, and a pin is
  proven before it is written: `update-tools.sh` runs the new binaries against the data first.
  Empty pins fail loudly rather than passing on nothing. (2026-08-23)
- **A published page this machine knows more about is an enhancement, not a hold-back**: the page
  text is taken as it stands and additions are appended with toml_edit, byte-for-byte round-trip,
  idempotent on re-run. Never regenerate a page from the index - it is lossy (per-store notes,
  comments). (2026-08-23)
- **A page exe must end `.exe` and not be a shared helper** before export: the stash records what
  the launcher reported, and for a wrapper that is the wrapper (`python3.13`, `env`) - written to
  a page it would poison the alias table for every game launched the same way. Sound because every
  umu launch is a Windows game under Proton. (2026-08-23)
- **The index names each game's page** (`page` column, gamedb-build 0.1.1) so an enhancement lands
  on the right file even once titles collide; consumers fall back to slug(title) against older
  indexes. (2026-08-23)

### umu Scope Gate (2026-08-22)
- **Our identity knowledge gets its own PR-based data repo, not a fork and not an upstream
  submission** - one TOML page per game, CI lint from our own validator, condensed artifacts
  (Parquet + SQLite + JSON + umu-shaped CSV) built by a runner and shipped as release assets.
  The repo optimizes for review, the artifacts optimize for lookup, which is what keeps every
  format choice reversible. See
  [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]] (2026-08-22, Proposed)
- **umu-database submissions are gated on an upstream protonfix** - an entry is only drafted and
  exported when a fix for the id it would carry exists in Open-Wine-Components/umu-protonfixes.
  Reason: the umu team told the owner directly, and the umu-database README says it in its opening
  paragraph - "We focus on games that requires fixes in Proton. Games that run out of the box have
  no need be added to the database." Earlier sessions had anchored on the README's ID rules further
  down and missed the scope rule; the consequence is that the submitted PR #151 (Control) does not
  qualify, since no `gamefixes-steam/870780.py` exists. Historical enforcement is loose (only 240
  of 1,202 database rows have a fix behind them), so existing content is not precedent. See
  [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]] (2026-08-22)
- "No fix found" is reported honestly per id class: a FIRM id (database id, detectable.json Steam
  sku, or manual assignment) with no fix proves the game runs out of the box; a GUESSED id
  (store-codename or title-slug draft) proves nothing - the fix could sit under a Steam appid we
  never learned (2026-08-22)

## Open Decisions

- **MPRIS as a source** - deliberately deferred. Already a good standard with its own consumers; wrapping it mostly duplicates. Move into S4 if needed.
- **arRPC-compatible bridge on 1337** - would let this replace arRPC for Vesktop users. Cheap once S2 exists; not currently in scope.
- ~~**Licence** - MIT OR Apache-2.0 proposed in the design doc.~~ RESOLVED 2026-08-07: Apache-2.0 single license (see Key Decisions → Public Branch).

## Non-Goals

- Publishing presence *to* Discord (that direction is well served)
- Being a Discord client mod
- Shipping UI of any kind
- Windows support (unix-socket and D-Bus shaped throughout)

## Recent Activity

- 2026-08-23: gamebus-gamedb PUBLISHED (https://github.com/fschaupp/gamebus-gamedb): tools release
  gamedb-tools-v0.1.0 pinned by digest, first data release v2026.08.23 (date tags), release and
  update-tools scripts in the data repo. gamebus-setup gained the `gamedb` subcommand and a fourth
  TUI tab on branch `feat/gamedb-export` (NOT merged): stash folded one-page-per-game, checked
  against the published index, exported or - when the game is already published - ENHANCED in
  place via toml_edit (Control gained its exe as a one-line diff, byte-identical elsewhere).
  Wrapper exes (python3.13, env) filtered before they poison the alias table. See
  [[wiki/logs/2026-08-23 - gamebus-gamedb Published and the Export Loop]].
- 2026-08-22: umu SCOPE GATE - the umu team (and the umu-database README's own opening
  paragraph) confirmed the database only wants games that REQUIRE a Proton/umu fix, a rule
  earlier sessions had read past. PR #151 (Control) therefore does not qualify: no
  `gamefixes-steam/870780.py` upstream and no `870780` branch in Proton's `proton_9.0` script.
  Built the gate into gamebus-setup: new `src/setup/umu_misses/fixes.rs` (304 lines) indexing
  Open-Wine-Components/umu-protonfixes from GitHub's recursive tree listing, cached at
  `~/.cache/gamebus-presenced/umu-protonfixes.txt` with a 7-day refresh and a
  `GAMEBUS_UMU_PROTONFIXES` override (local checkout or exported list; also keeps tests off the
  network); `Miss` gained an annotation-half `fix: Option<FixCheck>`; `--verify` records the
  per-entry fix check, `--fetch` refreshes both lists, `--export`/`--export-md` submit only
  entries with a fix, the MR text states the scope rule and links the fix file per row, the TUI
  gained a "Needs umu" line and a dark `○` glyph. Honesty split found live: no-fix on a FIRM id
  proves out-of-the-box, no-fix on a GUESSED id proves nothing. Measured upstream: 488
  `gamefixes-*` files serving 377 ids, 113 of them symlinks; enforcement historically loose (240
  of 1,202 database rows have a fix, 182 fix ids are not in the database at all). Live run
  against a COPY of the real stash: none of the 15 misses qualifies. Gate green (310 tests,
  clippy `-D warnings`, fmt). COMMITTED the same day as `e41d657` on master (11 files, +813/-57)
  once the owner closed the exploration; withdrawing PR #151 is still the owner's call, the
  PLAN.md S9 extension is still owed as its own docs commit, and gamebus-setup is not yet
  reinstalled; the daemon is untouched. See
  [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]].
- 2026-08-22: CORRECTIONS REPO DESIGNED (nothing implemented) - with no upstream willing to take
  this project's identity knowledge, the decision is to publish it as its own public git repo
  shaped the way umu-database should have been: one TOML page per *game* with
  `[[stores.<store>]]` arrays nested inside (never one file per store entry, which would hide the
  cross-store identity claim), reviewed as a pull request, validated by a CI lint that downloads a
  pinned release of our existing Rust validator, with generated artifacts (`identities.json`,
  `.parquet`, `.sqlite`, umu-shaped `.csv`, flattened `shared-helpers.txt`, plus a checksum
  manifest) published as release assets and never committed. DuckDB in the runner produces both
  binary artifacts (its SQLite extension writes as well as reads); the daemon reads the SQLite one
  via `rusqlite`, keeping arrow out of its dependency tree. See
  [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]].
- 2026-08-08: S9c - umu-export conformance (NOTE column always empty, gog rows
  need numeric gogdb ids, per-store evidence links; triggered by the first real
  submission, upstream PR #151 for Control), manual-matchup workbench in the
  misses pane (`p` pick, `o` store lookups via GOG catalog/api.gog.com/egdata,
  `t` title override), and the Project Hospital mislabel defense (stash mirrors
  the elected group identity + shipped `shared-helpers.txt` with union
  layering). Local umu-db swap test closed the full contribution loop (patched
  Lutris' `umu-games.json` → live launch resolved `umu-870780`, no new miss)
  but exposed the group-split wart: the resolved id keys the game `steam:<appid>`
  while the wrapper stays `lutris:<uuid>` → two records during play (open,
  future slice). Merged --no-ff `fb8a702` (branch `fix/umu-export-conformance`
  kept, 9 commits), rolled out, daemon restarted; en-dash fix `075121b` on
  master. See
  [[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]].
- 2026-08-07 (night): **v0.1.0 released** - S10 on `feat/release-pipeline`
  (build.rs deleted, builds network-free, install fetches detectable.json
  best-effort via the installed CLI; `.github/workflows/release.yml`),
  `.scripts/release.sh` dogfooded for the release commit + tag, owner pushed
  to https://github.com/fschaupp/gamebus-presenced and the workflow published
  the release. Then all 17 public commits rewritten with `Assisted-by:`
  trailers + README AI note (force-push pending), and two Mastodon
  announcement posts drafted (main + umu-credit follow-up). See
  [[wiki/logs/2026-08-07 - gamebus-presenced v0.1.0 Release]].
- 2026-08-07 (evening): Public branch re-composition - the owner's 17
  cherry-picked commits became 13 publishable feature commits on branch
  `public` (worktree `gamebus-presenced.worktrees/public`), every one passing
  `cargo check --all-targets`; ~340 em-dashes and ~150 internal references
  scrubbed from every commit via `git filter-branch --tree-filter` with
  replace-or-die scripts; Apache-2.0 LICENSE + NOTICE threaded through the
  whole history; original kept as ref `public-original`. Push pending. See
  [[wiki/logs/2026-08-07 - gamebus-presenced Public Branch Re-Composition]]
  and [[wiki/concepts/Deterministic History Re-Composition with Tree Filters]].
- 2026-08-07: S9b - umu-miss verification/drafting/export pipeline in
  gamebus-setup (`--verify`/`--fetch`/`--export`/`--export-md`/`--check-prs`),
  TWO adversarial review rounds (18 + 9 agents; 10 confirmed findings, all
  fixed - worst: two-writer stash lost-update, see
  [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]]),
  interactive TUI misses pane (tab bar; `v` fetch+verify, `a` assign id, `s`
  store correction, `d` dismiss), and `endpoints.toml` (all remote URLs in
  one shipped config). All on branch `feat/s9-umu-miss-report` (9 commits,
  4c8a16e→b638404; merged to master the same morning as --no-ff 58afefd and pushed). See
  [[wiki/logs/2026-08-07 - gamebus-presenced S9b umu Contribution Pipeline]].
- 2026-08-06: S6 MPRIS hints, S6b Lutris argv titles + any-key scan adoption
  (5a620e9, 70d84e4), S7 publish hygiene - withhold nameless, wrapper pattern
  blacklist, name monotonicity (489e7fd, c1904e5), S8 Heroic detection
  (186c427, 27473ac), all on master; S9 umu-miss stash landed just past
  midnight (4c8a16e, 2026-08-07 00:09, on the S9b branch). See
  [[wiki/logs/2026-08-06 - gamebus-presenced S6-S9 Naming Layers and umu Stash]].
- 2026-08-06: S4f (game groups: class-elected sticky representative, deferred
  migration, ListGames reseed - fixes the Brotato/Amnesia record losses) and
  S5 (gamebus-setup TUI, STAGED) landed on master (merge e56bbb8, fix
  18cb92b). The S6-S9 dev log now exists (see above); S5 and S4f dev logs +
  a setup-tool ADR are STILL pending - see repo PLAN.md and
  [[wiki/concepts/2026-08-06 - Learnings Review]].

- 2026-08-04: S4d implementation completed - ancestor-walk join for wrapper-tree dedup. Enricher tracks `{pid: steam_appid}`, `is_ancestor()` ppid-chain walk (bounded to 10 hops), descendant absorbs ancestor. Three bugs found during live Brotato testing: merge direction reversed, cache-adopted records couldn't be removed (correlator `drop_partial` fix), `SteamAppId=default` false positive (numeric-only appids). Also fixed: naming precedence (Steam's detectable.json-enriched name beats GameMode's executable stem). See [[wiki/logs/2026-08-04 - gamebus-presenced S4]].
- 2026-08-04: Dead-code cleanup (owner-confirmed): `Manager::remove_by_source` (superseded by the correlator) and the unused S0 error variants `NameAcquisition`/`Config`/`Internal` removed; `cargo clippy --all-targets` is now fully clean, zero warnings. The test-removal exchange produced the workflow preference [[wiki/concepts/test-deletion-visibility]].
- 2026-08-04: S3 implementation completed - proxy (byte-identical forwarding, tap, upstream-loss close), correlator (`pid_<pid>` absorbs `discord_<pid>`, degrade-in-place, die-with-last-source, exact-pid join), restart cache (pid + start-time, re-adopted before sources spawn). 31 unit + 5 integration tests green. See [[wiki/logs/2026-08-04 - gamebus-presenced S3]] and [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]].
- 2026-08-04: S3 planned with owner - landing order proxy -> correlator -> cache; absorption and upstream-loss-close decisions confirmed; umu research: launcher assigns identity via env vars (no runtime detection to reuse), `UMU_ID` enrichment deferred to S4, wrapper-tree join deferred to S4.
- 2026-08-04: S2 implementation completed - standalone Discord IPC listener. `discord-ipc-0` bound with stale-socket handling, handshake/READY, lock-step echo, `SET_ACTIVITY` via pinned `rsrpc` crate payload model, `SO_PEERCRED` pid, `discord_<pid>` objects, mutable `ActivityInterface` with `PropertiesChanged`. See [[wiki/logs/2026-08-04 - gamebus-presenced S2]] and [[wiki/decisions/adr-006-rsrpc-crate-dependency]].
- 2026-08-04: S1 implementation completed - GameMode source feeding the D-Bus surface. Watcher with `NameOwnerChanged` availability tracking, per-activity objects at `.../Activity/pid_<pid>`, `ActivityAdded`/`ActivityRemoved` signals, `HasActivity` change emission, `ListActivities` as `ao`. Two live discoveries recorded in [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]]. Verified by integration test + busctl acceptance. See [[wiki/logs/2026-08-04 - gamebus-presenced S1]] for details.
- 2026-08-04: S0 implementation completed - D-Bus interface foundation (`org.gamebus.Presence.v1.Manager` with `ListActivities`, `HasActivity`, `Version` properties; `Activity` type; zbus v4 bindings; service verified on session bus). See [[wiki/logs/2026-08-04 - gamebus-presenced S0]] for details.
- Dev logs: [[wiki/logs/2026-08-04 - gamebus-presenced S0]], [[wiki/logs/2026-08-04 - gamebus-presenced S1]], [[wiki/logs/2026-08-04 - gamebus-presenced S2]], [[wiki/logs/2026-08-04 - gamebus-presenced S3]], [[wiki/logs/2026-08-04 - gamebus-presenced S4]], [[wiki/logs/2026-08-06 - gamebus-presenced S6-S9 Naming Layers and umu Stash]], [[wiki/logs/2026-08-07 - gamebus-presenced S9b umu Contribution Pipeline]], [[wiki/logs/2026-08-07 - gamebus-presenced Public Branch Re-Composition]], [[wiki/logs/2026-08-07 - gamebus-presenced v0.1.0 Release]], [[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]], [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]] (S4f/S5 logs pending - repo PLAN.md is authoritative)
- Kanban board: [[boards/gamebus-presenced]]
- ADRs: [[wiki/decisions/adr-001-zbus-v4-tokio-runtime]], [[wiki/decisions/adr-002-simplified-activity-type]], [[wiki/decisions/adr-003-d-bus-service-naming]], [[wiki/decisions/adr-004-manager-and-activity-interfaces]], [[wiki/decisions/adr-005-activity-object-ids-and-gamemode-game-objects]], [[wiki/decisions/adr-006-rsrpc-crate-dependency]], [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]], [[wiki/decisions/adr-008-appid-records-one-record-per-merge-key]], [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]], [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]]

## Dependencies

- `zbus` - D-Bus bindings
- `tokio` - Async runtime
- `serde`/`serde_json` - Serialization
- `libc` - `SO_PEERCRED` via getsockopt
- `rsrpc` (MIT, pinned git rev `062e0fd`) - Discord IPC payload model (`rsrpc::cmd`), used since S2
- `discord-rich-presence` (dev) - genuine RPC client for integration tests

Note: `pog5/rsrpc` is GPLv3 - fine to learn from, not to vendor.

## Related

- [[GameMode]] - Feral Interactive's game mode daemon
- [[Discord IPC]] - Discord's local RPC protocol
- [[D-Bus]] - Freedesktop's message bus system
- [[MPRIS]] - Media Player Remote Interfacing Specification (model for presence interface)

## Sources

- Design doc: `docs/design/gamebus-presence.md` (local)
- PLAN.md: `PLAN.md` (local)
- Prior art: [rsRPC](https://github.com/SpikeHD/rsRPC), [arRPC](https://github.com/OpenAsar/arrpc), [GameMode](https://github.com/FeralInteractive/gamemode)
