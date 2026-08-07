---
date: 2026-08-07
type: devlog
tags: [devlog, gamebus-presenced, public-branch, git-history, filter-branch, licensing, apache-2]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

Dev log for the 2026-08-07 **evening** session of [[wiki/projects/gamebus-presenced]] (the morning session is [[wiki/logs/2026-08-07 - gamebus-presenced S9b umu Contribution Pipeline]] — separate log, do not confuse them). The owner's 17-commit cherry-picked branch `public` (in worktree `/media/Data/Projekte/gamebus-presenced.worktrees/public`) was re-composed into **13 concise publishable feature commits**, every one passing `cargo check --all-targets`, with all internal references and em-dashes scrubbed from every commit via `git filter-branch --tree-filter`, and licensed Apache-2.0 with LICENSE + NOTICE threaded through the whole history. The original branch survives as backup ref `public-original`. Verified live this session via `git log public` in the worktree (as of 2026-08-07).

## The request

The owner created branch `public` by cherry-picking 17 commits from master — all features, excluding `PLAN.md`, `docs/`, agent files, and `.second-brain`. Ask: re-compose it into concise publishable feature commits with these constraints:

- No misdirections — commits must honestly describe what they contain
- No em-dashes anywhere (code, docs, commit messages)
- No Co-Authored-By tags
- Tweet-size messages (max 200 chars)
- Commits sizeable enough that a new contributor understands the project by reading them in order

## The result

**13 commits, every one green under `cargo check --all-targets`.** Progression (verified live, `git log --oneline public`, 2026-08-07):

1. init (7db6436)
2. daemon surface + GameMode source (5b1d45e)
3. Discord IPC (099dcda)
4. correlator/proxy/restart-cache (a2bfbaf)
5. Steam probing + detectable.json naming (3839de2)
6. packaging (8016d21)
7. wrapper identification (f3b04bd)
8. game-groups fix (dd5bd8a)
9. MPRIS/Lutris/exe naming (33aa39b)
10. publish-once-named hygiene (751b7a2)
11. Heroic (660e0c2)
12. gamebus-setup TUI (8d1443e)
13. umu pipeline (8468587)

The original 17-commit branch is kept as backup ref `public-original` (delete after publishing).

## Method: three passes

The whole approach is generalized in [[wiki/concepts/Deterministic History Re-Composition with Tree Filters]] (concept note written in parallel with this log).

### Pass 1 — `clean.py`, deterministic cleanup

A deterministic cleanup script: ~60 exact-pair rewrites plus generic regexes, **replace-or-die semantics** (a pair that no longer matches aborts the run instead of silently skipping). Removed:

- ~340 em-dashes
- ~150 internal references: S-phase tags (S0-S9b), `spec §` pointers to the private spec, ADR numbers, R-rule numbers, "Spec test N" labels, diary dates ("observed live 2026-08-06"), design-doc citations
- Test identifiers renamed: `S9B_STASH` → `FIXTURE_STASH`, `write_s9b_fixtures` → `write_umu_fixtures`
- README rewritten: dropped links to non-public `PLAN.md` and `docs/design`

### Pass 2 — history rebuild

Cherry-pick chain with fresh messages and preserved author dates; two content squashes (scaffolding commits merged; a test-reliability fix folded into wrapper-identification). Then `git filter-branch --tree-filter clean.py` over **every** commit, so no commit in the published history ever contains the removed text. The script was verified byte-reproducible against the hand-validated final tree before filtering.

### Pass 3 — per-commit compile fixes (`fixes.py`, replace-or-die)

The cherry-picked intermediate commits did not compile: conflict resolutions had pulled later file versions in. Fixes applied per commit:

- Stripped premature `pub mod discord;` and its spawn from the daemon commit
- Stripped enricher/naming from the core commit's `main.rs` and inlined the `steam()` test helper
- Removed a premature `[[bin]] gamebus-presence` Cargo.toml entry
- Injected `tests/common/mod.rs` at its first user (4 commits)

Two structural squashes came out of this pass: dbus + gamemode became one "daemon surface + first source" commit (`main.rs`'s event loop is inseparable from sources), and Steam-enrichment + naming merged (the cherry-picked S4a commit already contained the naming integration). The chain was rebuilt with `git commit-tree` — snapshot trees, zero conflicts.

## Notable discovery: the original history never compiled either

Confidence: **high** (verified by reading the original commits this session). The ORIGINAL master history's intermediate commits also never compiled — 712e4ae (S0) `main.rs` already declared `mod sources;` before `src/sources` existed, and f1491fb (S3) `main.rs` already referenced the enricher. There was no clean historical version to restore; the compile fixes had to be hand-written.

## Verification battery

- Per-commit `cargo check --all-targets`: all 13 green
- Per-commit `git grep` for banned patterns (em-dashes, S-tags, spec refs, ADR numbers, diary dates): clean
- Message constraints checked programmatically: longest message 185 chars (limit 200)
- Final tree verified byte-identical to the tree that passed the full gate: fmt, clippy `-D warnings`, 13 test binaries, 240+ tests

## Licensing round

Each change threaded through ALL commits via tree-filter and verified per-commit:

- **Apache-2.0 single license** (supersedes the design doc's "MIT OR Apache-2.0 (proposed)") in README and Cargo.toml `license` field
- Canonical LICENSE from https://www.apache.org/licenses/LICENSE-2.0.txt (11,358 bytes, md5-verified identical in every commit); the LICENSE appendix placeholders stay verbatim
- NOTICE file: "Copyright 2026 Florian Schaupp" — no email in copyright lines
- README Licence line: "Apache-2.0. Copyright 2026 Florian Schaupp."
- New README prior-art entry for umu: umu-launcher (https://github.com/Open-Wine-Components/umu-launcher) and umu-database (https://github.com/Open-Wine-Components/umu-database)
- `## Disclaimer` section at the README bottom with the owner's verbatim text: "Note: This tool is not affiliated with Discord Inc. It's an independent project that uses Discord's public API for utility purposes."

Git author identity on the public branch deliberately stays the alias `fschaupp <spritzwine.absently488@passinbox.com>` — a privacy relay. Copyright name (real name in NOTICE) and git identity are independent by design.

## Open

- Owner pushes/publishes `public` (TBD when)
- Delete `public-original` after publishing

## Related

- [[wiki/projects/gamebus-presenced]] — project note (Key Decisions updated with the public-branch style rules and license this session)
- [[wiki/logs/2026-08-07 - gamebus-presenced S9b umu Contribution Pipeline]] — the earlier session today
- [[wiki/concepts/Deterministic History Re-Composition with Tree Filters]] — the generalized method
