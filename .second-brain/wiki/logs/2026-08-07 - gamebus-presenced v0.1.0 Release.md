---
date: 2026-08-07
type: devlog
tags: [devlog, rust, release, ci, github-actions]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

Third [[wiki/projects/gamebus-presenced|gamebus-presenced]] session of 2026-08-07 (night, after
[[wiki/logs/2026-08-07 - gamebus-presenced Public Branch Re-Composition|the re-composition]]):
S10 made builds network-free and added a GitHub release workflow, `.scripts/release.sh` cut
v0.1.0, the owner pushed to https://github.com/fschaupp/gamebus-presenced and **the workflow
published the first release successfully**. Afterwards every public commit was rewritten to
carry an `Assisted-by:` trailer plus a README AI-assistance note (EU AI Act transparency),
which requires a one-time force-push + re-tag that is still pending.

## S10 - network-free builds + release workflow (branch `feat/release-pipeline`)

Branch `feat/release-pipeline` off master, two commits (9db4de7, 9e67bc8), **unmerged**
(owner's standing rule):

- `build.rs` deleted - it existed only to fetch `detectable.json` at build time. With it went
  `[build-dependencies]` and the `OUT_DIR` search tier in BOTH binaries (daemon
  `find_detectable_json()` and setup `DetectableTier::BuildDir`; `env!("OUT_DIR")` would not
  even compile without a build script). Builds need no network at all now.
- The install plan's last step is the freshly **installed** CLI running `fetch-detectable`
  (never the build-tree copy), `best_effort: true` so an offline install still succeeds; the
  daemon degrades to executable names and the status screen offers the fetch as a one-key
  remedy. `explain()` lost its now-unused `action` parameter; the scope note keeps the
  "rewrites your cache" headline only for the fetch-only plan (an install leads with write
  scope). Two new unit tests pin the plan shape and the wording.
- `.github/workflows/release.yml`: on `v*` tags - gate (fmt, clippy `-D warnings`, tests),
  release build, tarball `gamebus-presenced-$V-x86_64-linux.tar.gz` (3 binaries, `data/`
  units, endpoints.toml, README, LICENSE/NOTICE where present), sha256, release via the
  runner's own `gh`. Tag must match Cargo.toml's version. No third-party release actions.
- Verified before recommending the push: the integration tests really do print `SKIP` and
  pass when `dbus-daemon` is missing or gamemoded cannot be activated on the private bus
  (checked `tests/common/mod.rs` + every test head) - so the gate is honest on a bare runner.

## Release flow on `public`

- Owner cherry-picked the two commits onto `public` **excluding PLAN.md and scrubbing the
  tree em-dashes themselves** (verified: zero banned patterns arrived). Only the messages
  needed rewording to the branch's tweet-size rule (were 433/677 chars) - rewritten via
  `git commit-tree`, trees byte-identical.
- `.scripts/release.sh` (also cherry-picked to master as `f0454e6`): refuses dirty tree or
  existing tag; version = explicit `X.Y.Z` or `major|minor|patch` bump from Cargo.toml; gate
  (same as workflow) + release build (refreshes Cargo.lock); commit `release: vX.Y.Z`
  (`--allow-empty` for the re-release/first-release case); annotated tag. Never pushes.
  `cd "$(git rev-parse --show-toplevel)"` makes it cwd-independent.
- **Filesystem gotcha (worth remembering):** this machine's mount reports every file `rwx`
  and `core.filemode=false`, so `chmod +x` never reached the git index - the script was
  committed `100644` and would arrive non-executable on a fresh clone. Fix:
  `git update-index --chmod=+x`. Both branches corrected.
- Dogfooded: `./.scripts/release.sh 0.1.0` ran the full gate green and produced the release
  commit + tag `v0.1.0`.
- **Owner pushed; the GitHub workflow ran and published the v0.1.0 release** - first public
  release of the project. GitHub needed no manually created keys (automatic `GITHUB_TOKEN` +
  workflow-level `permissions: contents: write`), unlike the owner's Codeberg/Forgejo
  experience where release flows want explicit tokens.

## AI Act disclosure (owner's request, after the release)

All 17 public commits rewritten via `--msg-filter` + `--tree-filter`:

- Every commit message now ends with the trailer `Assisted-by: Claude Fable 5
  <noreply@anthropic.com>` (replaces the no-Co-Authored-By rule; 200-char guideline covers
  the prose, not the trailer).
- README carries, from the FIRST commit, one line above "Prior art it stands on":
  *"Built with AI assistance: Claude (Anthropic) pairs on this codebase; a human reviews,
  tests, and ships every change."*
- Tag `v0.1.0` re-created on the new tip. **Pending (owner):** delete the GitHub release +
  remote tag, `git push --force`, re-push the tag so the workflow republishes from the
  disclosed history. Safe window: repo public < 1 day, no forks.
- Release commits made by `.scripts/release.sh` carry no trailer by design (human-run).

## Mastodon announcement

Two posts drafted (in scratch, delivered in conversation): main post 497 effective chars
("It's alive!" hook, the three fragment-holders riff, MPRIS-style credibility phrase,
status-bars/overlays/desk-pets audience line, repo link, #LinuxGaming #Rust #FOSS) and a
follow-up reply 493 chars crediting umu's work (per-game ids for everything non-Steam,
umu-launcher link) plus the give-back pipeline (misses recorded, verified against the live
API, exported submission-ready). Accuracy caveat flagged to owner: "one command" produces
the submission text; the upstream PR itself is still a manual paste.

## Open

- Owner: force-push + re-tag the disclosed history (see above).
- ~~Merge `feat/release-pipeline` to master~~ - done the same night: --no-ff merge
  `d6adb69` (parents f0454e6 + a9dfa0e, branch kept). The two branch commits were
  rewritten first so their trailers follow the new Assisted-by rule. Gate green on the
  merged master (fmt, clippy -D warnings, 13 test binaries).
- master is unpushed ahead of gitea (f0454e6 + merge + vault commit); pushing is the
  owner's call.
