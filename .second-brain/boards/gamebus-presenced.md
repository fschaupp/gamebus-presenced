---
date: 2026-08-04
type: board
tags: [board, kanban, gamebus-presenced]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

Kanban board for the gamebus-presenced project. Tracks all tasks across slices S0-S9c (as of 2026-08-08). S0-S8 complete on master: S0 (D-Bus surface), S1 (GameMode source), S2 (Discord IPC listener), S3 (proxy + correlator + restart cache), S4a (Steam enrichment), S4b (naming), S4c (packaging), S4d (ancestor-walk), S4e (game identification + Steam scan + appid_records simplification), S4f (game groups), S5 (setup TUI, STAGED), S6/S6b (MPRIS + Lutris naming hints), S7 (publication hygiene), S8 (Heroic). S9 (umu-miss stash) + S9b (umu-database contribution pipeline) landed on master 2026-08-07 via --no-ff merge 58afefd (branch feat/s9-umu-miss-report kept, 9 commits 4c8a16e→b638404); master pushed to gitea. Public branch re-composed 2026-08-07 evening (17 cherry-picked commits → 13 clean publishable commits on branch `public`, backup ref `public-original`). **v0.1.0 RELEASED the same night** at https://github.com/fschaupp/gamebus-presenced via S10 (network-free builds + release workflow + `.scripts/release.sh`); the disclosed (`Assisted-by:` + README AI note) history still needs a one-time force-push + re-tag (status TBD as of 2026-08-08). S9c (matchup workbench + mislabel defense) merged to master 2026-08-08 via --no-ff merge fb8a702 (branch `fix/umu-export-conformance` kept, 9 commits) and rolled out live the same day (reinstall + daemon restart). **2026-08-22:** the umu team's scope rule (the database only wants games that require a Proton fix) reframed the contribution half - the protonfix scope gate is built and COMMITTED as `e41d657` on master, PR #151 (Control) turns out not to qualify, and a data-source survey seeded new backlog items. Since no upstream will take this project's identity knowledge, the plan is now its own PR-based data repo of per-game TOML pages with built artifacts - see [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]] and [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]].

## gamebus-presenced Kanban Board

### 📥 Backlog

- [ ] 🟢 **Group split on resolved umu id - unify groups across a process tree**
	Wrapper keys `lutris:<uuid>` while the game keys `steam:<appid>`, so two records exist during play. [[wiki/projects/gamebus-presenced]]
- [ ] 🟢 **PR #151: capitalize egs codename calluna → Calluna**
	Builds App Name; Lutris matches case-sensitively. Owner's PR, their call. Moot if the PR is withdrawn under the 2026-08-22 scope rule (see ⏳ Waiting On).
- [ ] 🟢 **Publish shared-helpers.txt separately + opt-in fetch flow**
	URL in endpoints.toml, CLI only.
- [ ] 🟡 **gamebus-gamedb: TOML game pages + lint, in-tree first** · @2026-08-22
	Generalize shared-helpers.txt into per-game TOML pages (`[[stores.<store>]]` arrays, provenance per claim) plus a `helpers.toml` that the build flattens to today's `shared-helpers.txt`, and expose the existing umu validator as a lint over that directory. SCAFFOLDED 2026-08-22 as `gamedb/` on branch `feat/corrections-data` (`d4181e4`): README with the scope rule, two seeded pages (Control/egs, Project Hospital/gog), `helpers.toml`, and both JSON Schemas. Named gamebus-gamedb and licensed ODbL 1.0 (both decided 2026-08-22; license text vendored). Still owed: the Rust lint, and the artifact runner. Every page carries a canonical `gamedb` id (steam-<appid>, else the umu id, else a lettered store codename, else a minted gamedb-<uid>), lint-recomputed so it cannot drift; the prefix names the namespace and the number is never parsed; `ids.umu` appears only for games really in umu-database. SPLIT OUT 2026-08-23 to `/media/Data/Projekte/gamebus-gamedb` (see Waiting On for the publish chain). Design settled in [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]]. Rationale: no upstream accepts this knowledge and a fork of anyone's catalog would rot. [[wiki/concepts/Game Identity Data Sources]] [[wiki/projects/gamebus-presenced]]
- [ ] 🟢 **Daemon reads the Parquet artifact directly** · @2026-08-22
	The owner's intent for client-side storage: structure carried in the file, densely compressed. SQLite via `rusqlite` is the interim, chosen by Claude and corrected 2026-08-22. At this scale the daemon loads the whole file into a `HashMap` at startup either way, so the real trade is the `parquet` crate's weight in the daemon's dependency tree. No data migration involved - the runner emits both. [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]]
- [ ] 🟡 **Icons and cover art over the D-Bus** · @2026-08-22
	detectable.json (already fetched and cached by the tools) carries `icon_hash` for 19,752 of 23,900 entries and `cover_image_hash` for 16,758; the Discord CDN URL is constructible from entry id plus hash and was verified live 2026-08-22. So richer presence needs no new data source. Lutris `coverart` (IGDB images) is a possible fallback; redistribution licensing TBD. [[wiki/concepts/Richer Presence Over the Bus]] [[wiki/projects/gamebus-presenced]]
- [ ] 🟢 **Lutris as a second opinion on Steam appids** · @2026-08-22
	`lutris.net/api/games/<slug>` returns `steamid`, and it matched the project's detectable.json-derived drafts exactly on all three games checked (Control 870780, Borderlands 3 397540, Project Hospital 868360). Two independent sources agreeing would firm up a drafted id - which is what the new scope check's firm-vs-guessed distinction hangs on. [[wiki/concepts/Game Identity Data Sources]] [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]]
- [ ] 🟢 **Report the Spellcraft row to Discord** · @2026-08-22
	Their detectable entry for "Spellcraft" lists `spellcraft dev (staging)/unitycrashhandler64.exe`; it is the only UnityCrashHandler in all 23,900 entries and the sole cause of the Project Hospital mislabel. Fixing it upstream helps everyone; shared-helpers.txt stays as the local defense either way. [[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]]
- [ ] 🟢 **Owed vault records: S5 dev log + setup-tool ADR, S4f dev log + ADR**
### 🔨 In Progress

- [ ] 🔴 **feat/identity-misses: widen the stash from umu-only to any identity gap, gamedb always-active** · @2026-08-30
	15 commits (c2c1bf9 through e0e8e28), NOT merged to master, gate green at 424 tests. The daemon records what a launcher (Lutris, Heroic, and Steam via its own appmanifest - read only when the appid is absent from detectable.json, never for an ordinary launch) taught it about a launch with no authoritative identity; gamedb-database participation for a umu miss becomes opt-in per entry (promoted by hand, or suggested by a cross-store match / a protonfix hit) instead of automatic; the setup tool fills codenames from Lutris's own `pga.db`, and the `p` pick offers Lutris library identities next to Heroic's. Live-verified against the owner's real stash and a real Steam launch (Danger Scavenger, appid 1169740, absent from detectable.json, named from its own appmanifest). Same-evening completion work: repo-wide ASCII-hyphen strip (76 files), footer keybinding hints made state-honest (`v` now verifies from the gamedb tab too, inert keys stay out of the line), Assisted-by trailers on all of the session's commits. [[wiki/logs/2026-08-30 - gamebus-presenced Identity Misses Implementation]] [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]]

### ⏳ Waiting On

- [ ] 🟡 **Merge feat/identity-misses (which continues feat/gamedb-export) to master** · @2026-08-30
	Owner's call, standing rule (only the owner merges/pushes). gamebus-setup and the daemon are already reinstalled from the branch tip for live testing; the merge to master itself is still pending.
- [ ] 🟢 **Cut a gamebus-gamedb data release containing the danger-scavenger page** · @2026-08-30
	The page exists in the owner's local gamebus-gamedb checkout, keyed `itchio-926077` (frozen on assignment), but no published data release contains it yet. Until one does, the gamedb tab derives a provisional id from precedence alone and shows `steam-1169740` for the same game instead - confirmed working as designed (ids are never re-pointed once published; the tab's precedence choice for an unpublished page is expected to differ from a page's own frozen id). `.scripts/release.sh` in the checkout, then push the tag, then `r` on the gamedb tab to refresh.
- [ ] 🟡 **Withdraw or keep PR #151 (owner decision)** · @2026-08-22
	Control has no protonfix upstream (verified 2026-08-22: no `gamefixes-steam/870780.py`, and no 870780 branch in Proton's own script), so by the umu team's scope rule the entry does not qualify. Supersedes the earlier "wait for PR #151 to merge" item; if it is withdrawn, the local Lutris `umu-games.json` patch needs restoring from `umu-games.json.bak-gamebus` and the calluna capitalization backlog item is moot. [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]]
- [ ] 🟡 **Republish the disclosed history (owner's call)**
	Delete the GitHub release + remote tag v0.1.0, `git push --force github public`, re-push the tag; the workflow republishes from the Assisted-by history. Then delete the `public-original` backup ref, and optionally add the alias email spritzwine.absently488@passinbox.com to the GitHub account so commits attribute. Status as of 2026-08-08: TBD - not confirmed done. [[wiki/logs/2026-08-07 - gamebus-presenced v0.1.0 Release]]
- [ ] 🟢 **Restore the Lutris `umu-games.json`** · @2026-08-08
	The 2026-08-08 swap test left a patched runtime file with `umu-games.json.bak-gamebus` beside it. Restoring it was previously gated on PR #151 merging; with the PR likely withdrawn (see above), restore it outright.

### ✅ Done
- [x] ~~🟡 **gamebus-gamedb published: repo live, tools release v0.1.0 pinned by digest, first data release v2026.08.23**~~ ✅ 2026-08-23
	https://github.com/fschaupp/gamebus-gamedb - Lint green on main, artifacts as release assets, date tags, `.scripts/release.sh` + `.scripts/update-tools.sh` in the data repo. CI-built binaries byte-identical to local builds. [[wiki/logs/2026-08-23 - gamebus-gamedb Published and the Export Loop]]
- [x] ~~🟡 **gamebus-gamedb: data set, lint, artifact builder, CI and docs**~~ ✅ 2026-08-23 (branch `feat/corrections-data`, through `52a070a`)
	TOML page per game with `[[stores.<store>]]` arrays; canonical `gamedb` id derived from precedence and frozen on assignment; every identifier resolves to exactly one page, so a game reaching the set twice fails the merge. Python lint plus a Rust port held to the same fixture report; `gamedb-build` writes JSON, Parquet, SQLite, the flat helper list and a checksum manifest, byte-deterministic bar the manifest. ODbL 1.0 on the data. Three agents built the pieces in parallel; the CI worker also caught that `core.filemode=false` had swallowed the exec bit on both checks. [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]]
- [x] ~~🔴 **umu submission scope gate: exports carry only games that need a Proton fix**~~ ✅ 2026-08-22 (`e41d657` on master, 11 files, +813/-57)
	`--verify` checks the upstream protonfix list (Open-Wine-Components/umu-protonfixes) per entry, `--fetch` refreshes it beside the database, and only games with a fix reach an export, which links the fix as evidence. A negative answer stays honest: under a guessed id, no fix shows nothing either way. Gate green at 310 tests, clippy `-D warnings`, fmt. [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]] [[wiki/projects/gamebus-presenced]]
- [x] ~~🟡 **S9c matchup workbench + mislabel defense merged (fb8a702, 9-commit branch fix/umu-export-conformance kept) + rolled out (reinstall, daemon restart)**~~ ✅ 2026-08-08
	Conforming exports (empty NOTE, gogdb-numeric gate); `p` pick from umu db + Heroic library; `o` store lookups (GOG catalog/by-id, egdata Builds App Name); `t`/store/codename overrides; shared-helpers.txt; stash mirrors the elected identity (Project Hospital/"Spellcraft" incident); ui + umu_misses module split. [[wiki/logs/2026-08-08 - gamebus-presenced S9c Matchup Workbench and Mislabel Defense]] [[wiki/projects/gamebus-presenced]]
- [x] ~~🟡 **Local umu-db swap test: Lutris umu-games.json patched, Control launch resolved umu-870780 live, loop closed**~~ ✅ 2026-08-08 (backup beside the file; retires when PR #151 merges)
- [x] ~~🟡 **Merge `feat/release-pipeline` to master (--no-ff, branch kept)**~~ ✅ 2026-08-07 (merge d6adb69, parents f0454e6 + a9dfa0e; trailers switched to Assisted-by first; gate green on merged master; master now 3 commits ahead of gitea, push is the owner's call)
- [x] ~~🟡 **v0.1.0 released: S10 network-free builds + GitHub release workflow + .scripts/release.sh**~~ ✅ 2026-08-07 (night; pushed by owner, workflow published the release)
	build.rs gone, install fetches detectable.json best-effort via installed CLI; release.sh gates, sets version, commits, tags; exec-bit fixed via update-index (core.filemode=false here). Mastodon posts drafted. All 17 public commits then rewritten with Assisted-by trailers + README AI note. [[wiki/logs/2026-08-07 - gamebus-presenced v0.1.0 Release]]
- [x] ~~🟡 **Publish/push the `public` branch (owner's call)**~~ ✅ 2026-08-07 (pushed to GitHub, first release live)
- [x] ~~🟡 **Public branch re-composition: 17 cherry-picked commits → 13 clean publishable commits on `public`**~~ ✅ 2026-08-07 (worktree /media/Data/Projekte/gamebus-presenced.worktrees/public, backup ref `public-original`)
	All em-dashes and internal references (S-phase tags, spec §, ADR numbers, diary dates) removed from every commit's tree via git filter-branch + deterministic cleanup script; fresh tweet-size messages (max 200 chars, no Co-Authored-By); every commit passes cargo check --all-targets; full licensing set threaded through history (Apache-2.0 LICENSE + NOTICE + "Copyright 2026 Florian Schaupp" + Discord disclaimer + umu prior-art in README). [[wiki/projects/gamebus-presenced]] [[wiki/logs/2026-08-07 - gamebus-presenced Public Branch Re-Composition]]
- [x] ~~🟡 **Merge feat/s9-umu-miss-report to master (--no-ff, branch kept) + push to gitea**~~ ✅ 2026-08-07 (merge 58afefd, by owner)
- [x] ~~🟡 **S9b: verify/draft/export pipeline + 2 adversarial review rounds (10 findings fixed) + interactive TUI pane + endpoints.toml**~~ ✅ 2026-08-07 (branch feat/s9-umu-miss-report through b638404)
- [x] ~~🟡 **S9: umu-miss stash written by the daemon**~~ ✅ 2026-08-07 (4c8a16e at 00:09, on feat/s9-umu-miss-report)
- [x] ~~🟡 **S8: Heroic Epic/GOG detection via HEROIC_APP_NAME + legendary install records**~~ ✅ 2026-08-06 (186c427, 27473ac)
- [x] ~~🟡 **S7: publish hygiene - wrapper pattern blacklist, withhold nameless, teardown name monotonicity**~~ ✅ 2026-08-06 (489e7fd, c1904e5)
- [x] ~~🟡 **S6/S6b: MPRIS naming hints + Lutris wrapper titles + any-key scan adoption**~~ ✅ 2026-08-06 (commits 5a620e9, 70d84e4)
- [x] ~~🟡 **S4e: detectable.json path-prefixed entries (83% of DB) - basename-bucketed index with path-suffix matching + backslash normalisation**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Three-layer game identification (wrapper cmdline / descendant walk / umu tmpdir sandbox-family scan)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Unresolved-wrapper retry tick (15s) for delayed game launches**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Bounded /proc Steam-appid scan for games without GameMode**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: identify_process filter for utility processes (wineserver, tabtip.exe, etc.)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: appid_records: HashMap<String, u32> - one record per merge key, tree_depth decides**~~ ✅ 2026-08-04 *(tiebreaker superseded 2026-08-06 by S4f class election)*
- [x] ~~🟡 **S4f: game groups - class-elected sticky representative, deferred migration, ListGames reseed**~~ ✅ 2026-08-06 (repo: PLAN.md §S4f, commit 18cb92b)
- [x] ~~🟡 **S5: gamebus-setup TUI - status dashboard + user/system install, STAGED**~~ ✅ 2026-08-06 (repo: PLAN.md §S5, merge e56bbb8)
- [x] ~~🟡 **S4e: Merge key generalisation - steam:<appid> / lutris:<uuid> / umu:<id>**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: SteamAppId=0 rejected (Heroic/GOG Proton compat)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Tracing filter fix - RUST_LOG=debug was ignored by add_directive**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Cache excludes Steam-only records (scan re-finds on restart)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: is_wrapper_executable clears wrapper names (env, bash, etc.)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4e: Discord integration tests skip when real Discord is running**~~ ✅ 2026-08-04
- [x] ~~🟢 **S4d: Bounded ancestor-walk join (ppid chain, descendant absorbs ancestor)**~~ ✅ 2026-08-04
- [x] ~~🟢 **S4c: gamebus-presence monitor CLI (pretty-prints bus state)**~~ ✅ 2026-08-04
- [x] ~~🟢 **S4c: gamebus-presence fetch-detectable CLI subcommand (refresh to $XDG_CACHE_HOME)**~~ ✅ 2026-08-04
- [x] ~~🟢 **S4c: systemd user unit + D-Bus activation file**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4b: build.rs fetch detectable.json from Discord applications/detectable endpoint**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4b: Naming lookup - precedence: Discord name > detectable.json > executable stem**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: Create `src/enricher.rs` - Enricher middleware between sources and correlator**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: Steam probe - `/proc/<pid>/environ` for SteamAppId/SteamGameId/UMU_ID/STORE**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: Correlator `steam` slot in Partials + merge rule**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: Steam partial removal tied to last non-Steam source**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: tracing::debug! on Discord-pid join-miss**~~ ✅ 2026-08-04
- [x] ~~🟡 **S4a: Integration test - Steam appid enrichment via /proc environ**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create Cargo.toml with dependencies (zbus, tokio, serde, thiserror, tracing)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/main.rs - daemon entry point**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/error.rs - custom error types**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/dbus/mod.rs - module exports**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/dbus/types.rs - core types (Activity, Source, Kind, AppIds)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/dbus/connection.rs - bus connection management**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/dbus/manager.rs - Manager interface implementation**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/dbus/activity.rs - Activity interface stub**~~ ✅ 2026-08-04
- [x] ~~🟡 **Verify compilation with cargo check**~~ ✅ 2026-08-04
- [x] ~~🟡 **Test D-Bus service with busctl**~~ ✅ 2026-08-04
- [x] ~~🟡 **Create src/sources/gamemode.rs - GameMode proxy + NameOwnerChanged watcher**~~ ✅ 2026-08-04
- [x] ~~🟡 **Resolve per-game details from GameMode.Game object properties (Executable/Timestamp, /proc fallback)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Implement ActivityInterface with all read-only properties**~~ ✅ 2026-08-04
- [x] ~~🟡 **Add ActivityAdded/ActivityRemoved signals + HasActivity change emission**~~ ✅ 2026-08-04
- [x] ~~🟡 **Fix ListActivities to return ao (object paths)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Wire daemon core event loop (mpsc from watcher)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Unit tests: from_gamemode, name fallback, idempotent add/remove, remove_by_source**~~ ✅ 2026-08-04
- [x] ~~🟡 **Integration test: gamemoderun sleep 30 end-to-end**~~ ✅ 2026-08-04
- [x] ~~🟡 **busctl acceptance: monitor signals, HasActivity flip**~~ ✅ 2026-08-04
- [x] ~~🟡 **Unify sources under SourceEvent channel (source-agnostic core)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Add rsrpc crate dependency (pinned rev) for SET_ACTIVITY payload model**~~ ✅ 2026-08-04
- [x] ~~🟡 **Discord frame codec + handshake/READY + lock-step echo**~~ ✅ 2026-08-04
- [x] ~~🟡 **tokio UnixListener on discord-ipc-0 with stale-socket handling**~~ ✅ 2026-08-04
- [x] ~~🟡 **SO_PEERCRED pid via libc::getsockopt (client args.pid ignored)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Activity::from_discord mapping (name, details/state, timestamps, assets, party)**~~ ✅ 2026-08-04
- [x] ~~🟡 **Mutable ActivityInterface with PropertiesChanged for mid-session updates**~~ ✅ 2026-08-04
- [x] ~~🟡 **Integration test: discord-rich-presence client end-to-end**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3a: upstream discovery at connect time (walk discord-ipc-1..=9)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3a: pump mode - whole-frame forwarding + SET_ACTIVITY tap**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3a: upstream loss closes client connection; standalone fallback**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3a: proxy fidelity test (fixture upstream, byte-identity)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3b: correlator.rs per-source partial store keyed by pid**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3b: merge rules both orders + field precedence; pid_<pid> absorbs**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3b: split/degrade - die with last source, no regression**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3b: same-pid join integration test (RPC client + RegisterGameByPID)**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3c: write-through cache keyed pid+starttime; startup re-adoption**~~ ✅ 2026-08-04
- [x] ~~🟡 **S3c: restart re-adoption test (SIGKILL + respawn)**~~ ✅ 2026-08-04
