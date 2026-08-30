---
date: 2026-08-23
type: devlog
tags: [devlog, rust, gamedb, gamebus-gamedb, release, toml, tui]
related-projects: [gamebus-presenced]
confidence: high
ai-first: true
---

## For future Claude

[[wiki/projects/gamebus-presenced|gamebus-presenced]] session of 2026-08-23:
**gamebus-gamedb went public and gained its full contribution loop.** The repo
lives at https://github.com/fschaupp/gamebus-gamedb (owner pushed; only the
owner pushes, ever - their explicit rule this session). The daemon-side
export feature landed on branch `feat/gamedb-export` (NOT merged to master;
owner's call). All facts below verified live 2026-08-23.

## 1. Publication chain (all links verified)

- Tools release **gamedb-tools-v0.1.0** on gamebus-presenced: `gamedb-lint`,
  `gamedb-build`, `SHA256SUMS`. Tag pushed by the owner (a `gh` token without
  the workflow scope cannot push workflow-carrying commits; owner ran
  `gh auth refresh -s workflow`).
- Pin committed in gamebus-gamedb ([[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts|ADR-010]]'s
  fetch-by-digest design): lint `1fbc8daf...`, build `22440df0...`. The
  CI-built binaries produced **byte-identical artifacts** to local builds -
  determinism across machines, not just runs.
- First data release **v2026.08.23** (date tags, owner's choice: a data set
  has no API surface to be semantic about). `.scripts/release.sh` (date tag,
  gates with the pinned tools first) and `.scripts/update-tools.sh` (moves the
  pin to the newest tools release, proving it on the data first) exist in the
  data repo.
- Repo metadata set; Lint on main is green.

## 2. The export loop (`gamebus-setup gamedb`, branch feat/gamedb-export)

Two commits, built by briefed agents, verified independently:

- **`5f2d219`**: folds the umu-miss stash into pages (one per GAME - 15
  launches became 4 candidates), checks them against the published
  `identities.json` (cached, release-tagged, 7-day staleness,
  `GAMEBUS_GAMEDB_INDEX` override), exports `games/<slug>.toml` to `--out` /
  `--documents` / a TUI-configured dir (`setup.toml`, new). Fourth TUI tab.
  Live result: Control and Project Hospital recognized as published; Black
  Ops Cold War and Star Wars Outlaws written as exe-only pages keyed
  steam-1985810 / steam-2842040; both lints pass on the output.
- **`769cdf5`**: a published game this machine knows more about is an
  **enhancement, not a hold-back**: the page text is taken as it stands
  (checkout at `--out`, else `GAMEBUS_GAMEDB_PAGES`, else `--fetch-pages`)
  and additions are appended with `toml_edit`, byte-for-byte round-trip.
  Live: Control gained exactly `exe = ["Control_DX12.exe"]`, one-line diff,
  idempotent on re-run. The index gained a `page` column (gamedb-build
  0.1.1, needs a tools release before it is live). The gamedb tab gained the
  misses tab's matchup keys (p/o/t/s/a, x dismiss, Enter jumps) acting on a
  candidate's representative entry.

## 3. Defects found by running against the real stash

- **Wrapper processes as page exes**: the stash records what the launcher
  reported - `/usr/bin/python3.13`, `/usr/bin/env` - and those would have
  poisoned the alias table (`exe:python3.13` -> Black Ops). Filter: a page
  exe must end `.exe` (every umu launch is a Windows game under Proton) and
  not be a shared helper. Regression-tested.
- **Release tag lost to the second redirect**: GitHub redirects
  `latest/download` -> `download/<tag>/` -> asset CDN; following both loses
  the tag. The first hop is now resolved unfollowed and the tag read from
  `Location`.
- **Agent-brief bug caught by the agent**: the example GOG id for Control was
  Project Hospital's; an enhanced Control claiming it fails one-game-one-page.
- Earlier the same day: TOML top-level `exe` after `[ids]` would parse as
  `ids.exe` (agent caught, emitted before); `core.filemode=false` swallowed
  exec bits on the lint scripts (CI agent caught).

## 4. Open

- **GOG codename for Control**: the owner's store override says gog but the
  stash has no product id; the `o` lookup on the gamedb tab is the intended
  path, then the next export adds `+gog/<id>`.
- Cut **gamedb-tools-v0.1.1** (page column) when convenient; then
  `update-tools.sh` + a new data release.
- Merge `feat/gamedb-export` to master (owner), reinstall gamebus-setup
  (still not reinstalled since the scope gate).
- Two exe-only pages sit untracked in the owner's gamebus-gamedb checkout,
  ready as the first real PR, plus the Control enhancement.

## Related

- [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]]
- [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]] (why these games
  have nowhere else to go)
- [[wiki/concepts/Game Identity Data Sources]]
