---
date: 2026-08-06
type: devlog
tags: [devlog, gamebus-presenced, s6, s7, s8, s9, mpris, naming, heroic, umu, hygiene]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

Dev log for the 2026-08-06 evening of [[wiki/projects/gamebus-presenced]] - four slices landed on master in one run (21:44 to 00:09): S6 MPRIS naming hints + S6b Lutris wrapper argv layer, S7 publication hygiene, S8 Heroic support, and the S9 umu-miss stash (S9 opened on branch `feat/s9-umu-miss-report` just past midnight). Each master landing is a branch commit plus its `--no-ff` merge commit with the same subject (e.g. 5a620e9 merged as 70d84e4). The next session turned the stash into a full contribution pipeline - see [[wiki/logs/2026-08-07 - gamebus-presenced S9b umu Contribution Pipeline]]. All facts below verified against `git log master`, PLAN.md §S6-§S9, and commit bodies on 2026-08-07.

## S6 - MPRIS naming hints (5a620e9, merged as 70d84e4, 21:44)

The *naming half* of the long-deferred "MPRIS as a source" decision, and only that half - the full source stays deferred (MPRIS already has its own consumers).

- New watcher `src/sources/mpris.rs` follows `org.mpris.MediaPlayer2.*` bus names (ListNames seed + NameOwnerChanged), resolves each player's pid via `GetConnectionUnixProcessID` - a **kernel-verified pid join**, not string matching - reads the `Identity` property, and emits `SourceEvent::NameHint { pid, name }`.
- Hints are consumed entirely by the enricher: they create no records, keep none alive, appear in no `Sources` list, and never reach the correlator. **A hint is never a source.**
- Precedence extends the S4b anti-goal: a hint replaces only a *default* name - empty, or the executable stem - one rung above the stem and below Discord, group identity, and detectable.json. The stem case is the point: a native binary like `Brotato.x86_64` whose detectable entry only lists the Windows exe finally gets a human name when anything in its group exposes MPRIS.
- Names are monotone: a closing player never un-names a record. Dead pids are pruned from the hint map on the tick.
- Two bugs the tests surfaced: `apply_naming` early-returned without a naming DB (skipping hints exactly where they matter most), and "fill only empty names" was useless against stem defaults.
- **Documented miss (as of 2026-08-06, verified live):** Flatpak-sandboxed players resolve to the `xdg-dbus-proxy` pid, so their hints join nothing. Deliberate - no fabricated joins across sandbox boundaries.

Verified: 88 unit tests (+6 hints, +3 lutris) and `tests/mpris_naming.rs` - a private-bus integration test serving a genuine MPRIS player (`zbus::interface`, real `Properties.Get` wire) joined by pid via the bus's own gamemoded; record named after the player's Identity, MPRIS absent from `Sources`, name survives player exit (rule §0.6, real-wire testing).

## S6b - Lutris wrapper argv layer + any-key scan adoption (same commit)

Two live failures in one evening, both fixed in the S6 commit:

1. **"Ubisoft Connect" shown instead of "Far Cry Primal".** A Lutris-launched `UbisoftConnect.exe` had neither a detectable.json entry nor any MPRIS player. Fix: identification **layer 4** reads the game title straight from a `lutris-wrapper` ancestor's argv (`lutris-wrapper <title> <n> <n> <command…>`; the two counters are found right-to-left so titles ending in digits survive), bounded by the same `MAX_ANCESTOR_DEPTH` walk. Unlike layers 1-3 it works without a naming database.
2. **Far Cry Primal ran unadopted while its record kept the launcher's name.** The tick scan's adoption was gated on a *Steam* appid, so lutris/umu-keyed game processes (`SteamAppId=default`, `LUTRIS_GAME_UUID=<uuid>`) were invisible to it. The scan now probes **every merge-key type** for adoption into an existing group; a Steam appid is required only where it always was - creating a *new* group. With that, the launcher-named record upgrades to the game (`fcprimal.exe` is in detectable.json → GameProcess class → dethrone + identity upgrade) within one tick.

## S7 - Publication hygiene (489e7fd, merged as c1904e5, 23:11)

Born from one evening's journal: 107 pid publishes in 4 hours, mostly µs-lived helper corpses.

- **Pattern wrapper blacklist.** `is_wrapper_executable` became three rules over a backslash-aware basename: literals (shells, launch plumbing, the Wine service set - `wineserver`, `services.exe`, `conhost.exe`, … - and `steamwebhelper`), prefix families (`steam-runtime-`, `pressure-vessel-`, `pv-`, `srt-`, `{i386,x86_64}-linux-gnu-`), and version-trimmed interpreters (`python3.13` → `python`). Live symptom fixed: a `/usr/bin/python3.13` wrapper had classified GameProcess from the game path in its own argv, pinning its group so the real game could never dethrone it. Deliberately off the list: `wine64-preloader` (Wine games are only identifiable via the cmdline layer) and `sleep` (integration fixture).
- **Publish-once-named** (owner decision 2026-08-06). An ungrouped GameMode record whose name is empty after enrichment - what the monitor showed as "(unknown)" - is withheld from the bus and publishes the moment anything names it (hint, late identification, Discord join, or the 15s reseed), with its original `Since`. Death while withheld is fully silent: zero bus traffic for keyless helper corpses. Grouped records are never withheld (a merge key is game evidence); stem-named records still publish (the S1 pid+executable contract).
- **No name regression at teardown.** At game close the dying process re-emits with an unreadable exe, and the update used to clear the published name to "(unknown)" before removal. The correlator now enforces name monotonicity: an empty merged name carries the published name forward until the single ActivityRemoved.
- **`valid_steam_appid` rejects "0".** `merge_key_from_environ` accepted `SteamAppId=0` while `find_steam_appid` rejected it - every non-Steam title pooled into one bogus `steam:0` group whose members dethroned each other all evening. One shared definition now serves both; a zero Steam id falls through to the real Lutris/umu key.

Verified: 97 unit tests (+8) and `tests/withheld_publication.rs` on the private bus - a registered keyless `sh` wrapper never appears and never produces a removal while a plain `sleep` control publishes and dies cleanly.

## S8 - Heroic support (186c427, merged as 27473ac, 23:48)

Control, launched through Heroic (Epic/legendary), was completely invisible: environ carries `SteamAppId=0` and `GAMEID=umu-0` - both correctly rejected - and no Lutris UUID, so the whole tree was keyless; its blacklisted wrappers were withheld (S7 working as designed) and nothing remained to publish.

- `HEROIC_APP_NAME=<codename>` is present in every process of the tree and is now the **merge key of last resort** (`heroic:Calluna`; any Steam/Lutris/umu key wins).
- Group identity comes from the launcher's own install records: legendary's `installed.json` maps codename → display title (`Calluna → "Control"`), tried at the Flatpak and native config paths. Local files, no network, launcher-curated; Wrapper class, so a detectable.json hit on the real game process still upgrades.
- **Considered and rejected: the umu-database as a naming source.** It is protonfixes-scoped and does not know this game - `codename=Calluna` → `[]`, verified live against the API on 2026-08-06. `GAMEID=umu-0` is by definition the umu-miss case, and its one useful mapping (numeric umu id ⇒ Steam appid) has been implemented since S4. This rejection is what seeded S9.

Live result: Control (Epic via Heroic) detected and named.

## S9 - umu-miss stash (4c8a16e, branch `feat/s9-umu-miss-report`, 00:09)

Owner's idea: every launch that goes through umu without a database entry (`GAMEID=umu-0`, `UMU_ID=umu-default`) is a gap in the shared umu-database - and by session end, this daemon has usually worked out what the game was. Turn that into a contribution.

- The enricher stashes each miss at `$XDG_DATA_HOME/gamebus-presenced/umu-misses.json` (`src/umu_report.rs`): store guess (`HEROIC_APP_SOURCE`, else install-path heuristics, else `none`), codename (`HEROIC_APP_NAME` - for EGS exactly the App Name the database wants), the resolved title with its source and a **confidence label** (high: launcher config or detectable hit; medium: wrapper layers; low: hints/stems), and the game exe.
- Resolutions only upgrade, never downgrade; writes are atomic; **the daemon stays network-free** - no verification, no fetching, just recording.
- `gamebus-setup umu-misses` lists the stash for review; `--export` emits submission-shaped CSV matching the database's own header. Low-confidence and unresolved entries are listed but excluded from export.

Verification, id drafting, exports, and the TUI pane came the following session - [[wiki/logs/2026-08-07 - gamebus-presenced S9b umu Contribution Pipeline]]. Note (learned there, 2026-08-07): this initial stash implementation loaded the file once at startup and flattened its map over disk on every persist - the lost-update defect between the stash's two eventual writers, caught by adversarial review and fixed in ca97250.

## Related

- [[wiki/projects/gamebus-presenced]] - project note
- [[wiki/logs/2026-08-07 - gamebus-presenced S9b umu Contribution Pipeline]] - the next session: verify/draft/export pipeline, two adversarial reviews, endpoints.toml
- [[wiki/logs/2026-08-04 - gamebus-presenced S4]] - S4 log (naming layers 1-3, merge keys, wrapper blacklist origins)
- [[wiki/concepts/2026-08-06 - Learnings Review]] - same-evening learnings review (eb609e2)
- [[wiki/concepts/2026-08-06 - synthesis - record-identity-and-merging]] - record-identity synthesis these slices extend
- `PLAN.md` §S6-§S9 - roadmap source for this log
