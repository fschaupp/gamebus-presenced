---
date: 2026-08-07
type: devlog
tags: [devlog, gamebus-presenced, s9b, umu, setup-tui, adversarial-review, endpoints, csv]
related-projects: [gamebus-presenced]
ai-first: true
---

## For future Claude

Dev log for the 2026-08-07 session (00:47-03:25) of [[wiki/projects/gamebus-presenced]]: S9b turned the umu-miss stash from [[wiki/logs/2026-08-06 - gamebus-presenced S6-S9 Naming Layers and umu Stash]] into a full umu-database contribution pipeline inside `gamebus-setup` - verify, draft collision-checked ids, export CSV + merge-request text, interactive TUI pane - while the daemon stays network-free. Two adversarial review rounds (vault rule §0.7) confirmed 10 findings total; the worst was a lost-update between the stash's two writers, fixed with per-entry owned halves ([[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]], written in parallel with this log). Everything is on branch `feat/s9-umu-miss-report` - 9 commits ahead of master's 27473ac, **NOT merged** (owner decides). Commits verified via `git show --stat`; live results verified against the actual stash file and running daemon on 2026-08-07.

## S9b - verify, draft, export (2457cfe, 00:47)

Biggest commit of the session (+1655 lines): `src/setup/umu_misses.rs` (new, 841 lines), `src/umu_report.rs` (+587), naming, CLI, tests. All of it in `gamebus-setup` - the daemon only ever *writes* the stash.

- **`--verify`** checks every miss against the umu-database, local copy first - `--db <file>` / `GAMEBUS_UMU_DB` / the `--fetch` cache - then the live API per remaining entry. Three verdicts persisted into the stash: `already-in-database` (store+codename found - the launcher missed, not the database), `cross-store-id` (the title exists under another store; the id to reuse), `confirmed-missing`. No local copy *and* no API is an honest error with the stash untouched.
- **Dual DB parser**: both the upstream git checkout's CSV (with quoted fields) and the API's full JSON dump parse, both built from captured samples of the real formats (rule §0.6).
- **Id drafting** for confirmed-missing entries, per the database's own rules, strongest basis first: the title's Steam appid from detectable.json's `third_party_skus` → `umu-<appid>`; else a codename carrying at least one letter → `umu-<codename>`; else the standalone slug `umu-<lowercased-title>`. **Numeric ids are never drafted** - Proton parses a numeric second part as a SteamAppId. Every draft passes a **MANDATORY collision check** against the full database (no local database ⇒ no drafts): a same-title collision IS the cross-store id and is used as such; a different-title collision discards the draft and records the conflict.
- **`--fetch`** refreshes the cached full dump (`$XDG_CACHE_HOME/gamebus-presenced/umu-database.json`) - one request, validated by parsing before replacing the cache, never implicit.
- **`--export`** prefills verified/drafted ids (the NOTE column names the basis and collision-check date); `umu-FIXME` remains only where nothing could be drafted safely. Held-back entries are listed with reasons.
- **`--export-md`** writes a slim merge request: title line, one paragraph of provenance, fenced CSV rows, per-entry evidence, and a checklist mirroring the upstream README's rules.
- **`--check-prs`** (opt-in, best-effort) scans open upstream merge requests via the unauthenticated GitHub API for rows matching our entries; failures are reported and non-fatal.

## Live-found API fact (9f3f3f3, first live --verify)

The umu API's `?title=` lookup **substring-matches** - `?title=Control` returned Ground Control's `umu-254820`/`umu-254840` - and its rows carry no title field to compare against, so a hit cannot be told apart from a false positive. It would have prefilled Ground Control's id for Control in the export. Fix: API title hits are demoted to advisory notes ("substring match - check before submitting"); `?store=&codename=` stays authoritative (verified exact, live); `cross-store-id` comes exclusively from exact local-title matches and the drafting collision check. A fake-API integration test locks this in.

Live end-to-end after the fix (as of 2026-08-07, confirmed in the actual stash file): **Control** (egs/Calluna) `confirmed-missing`, drafted `umu-870780` from its detectable.json Steam sku, collision-checked; **Borderlands 3** (egs/Catnip, in the database as `umu-397540`) recognized as a launcher-side miss and held back from the export.

## Adversarial review round 1 → ca97250

Full Workflow review per vault rule §0.7 (as run this session: 18 agents across 6 dimensions, plus a fresh-context strategic-fit judge). Verdict: **revise**. 12 findings raised; 6 survived the refutation round, 6 refuted. The judge found the worst one. All six fixed in ca97250:

1. **Lost update between the stash's two writers** (the judge's find). The daemon loaded the file once at startup and every persist flattened its in-memory map over disk - any umu-missed launch after `--verify` silently erased all verification annotations, and a `--verify` during a session erased misses the daemon recorded meanwhile. Fix: per-entry **OWNED HALVES** - daemon owns the resolution half, setup tool owns the annotation half (verification, drafted id, PR mark, overrides) - and every persist in both binaries merges the other writer's half from disk first. Covered by an interleaved two-writer test. Full rationale: [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]].
2. **CODENAME unescaped in CSV export.** Verbatim from an untrusted process's environment; a comma in it forged the UMU_ID cell - *reproduced by the reviewer* (mechanism verified, not intention). Now goes through `csv_field`.
3. **Corrupt stash silently loaded as empty, then flattened.** Now: parse failure → `load_error`, the report drops its write path (nothing can flatten a file we could not read), the daemon warns, the CLI errors loudly instead of printing "No misses recorded" over data loss.
4. **Absolute exe paths leaked `/home/<user>` into public MR text.** Executables export as basenames only.
5. **`store=none` with a real codename violated the README's standalone rule.** Store `none` now pairs with codename `none`; the launcher codename moves to the NOTE. Verification and exports read the *effective* store.
6. **MR checklist auto-ticked the Steam-id rule for slug drafts.** It now pre-ticks only when every id provably honors the rule (cross-store-verified or Steam-sku drafts); a slug draft leaves the box for the human.

Also from the review: sticky `possible_pr` was never re-evaluated - `--check-prs` now re-evaluates from scratch each run (a closed-unmerged PR no longer holds an entry out of exports forever) and matches the quoted-title CSV form upstream uses for comma-carrying titles. Dead `UmuDb::is_empty` removed.

## TUI misses pane (2f29211) + focused re-review → b06757f

The pane (third TUI view): per-entry list (state glyph, store, confidence, last seen) with a detail pane showing resolution evidence, verification verdict + note, drafted id with basis and collision-check date, and any possible open-PR duplicate. Stash read off the render path (spawn_blocking → `Msg::Misses`), refreshed once a second while watched.

Focused re-review (9 agents this round): 4 findings confirmed, fixed in b06757f:

1. **Identity-unstable selection under the 1s refresh** - a `last_seen` bump or new miss reorders rows and a bare index silently switched the detail pane to a different game mid-review. Selection now keyed by stash key, re-selected after every list replace; the key is also the final sort tie-break (previously HashMap iteration order reshuffled same-day entries every load).
2. **Enter in the read-only pane fired hidden Status actions** - whatever invisible check-row remedy was highlighted underneath. Status-pane keys are now gated to the Status view; covered by a test.
3. **Module doc said "two views"** - now three.
4. **Possible-PR warning clipped at 80x24** - the pane clips from the bottom, and the one line that stops a duplicate submission was first to vanish, with no scroll affordance. Warning moved to the top of the detail; export hint to the footer.

## Interactive pane (30b9b42) and dismiss (6589e18)

- **Tab bar** under the header names all three views (`status │ monitor │ umu misses`), active one highlighted; Tab cycles.
- **`v`** - fetch + verify, **the only network verb**, labeled "(net)" in the footer. Verify/fetch/export otherwise stay explicit CLI invocations.
- **`a`** - manual id assignment: prefilled with the current draft so a correction is an edit; Enter runs the same mandatory shape + collision check as every draft (`DraftBasis::Manual`); a same-title holder is accepted as the cross-store id, a different-title holder rejects, and a numeric id gets the Proton SteamAppId caveat spelled out. Id-entry mode owns the keyboard while open.
- **`s`** - cycles the entry's store through the 8 store ids counted from the upstream CSV; stored as `store_override` on the **annotation half**, so the daemon's writes never revert it. This is what lets the next verify's store+codename lookup hit when the daemon guessed `none`.
- **`d`** (6589e18) - dismiss/restore flag, also annotation-half. Deliberately **parked, not deleted**: a deleted key would be resurrected by the daemon's merge and re-recorded on the next launch anyway; `d` on a dismissed entry restores it. Dismissed entries sort to the bottom with a grey ✗ and are held out of both exports with the reason named.
- Verified live under tmux: store cycle, fetch+verify with honest offline notes, prefilled assignment round-trip.

## endpoints.toml (b638404, 03:25)

All remote URLs - Discord's public detectable endpoint, the umu API, the upstream repository, its open-PRs URL - now live in one shipped, commented `endpoints.toml` instead of consts scattered through the sources: an upstream API change becomes a config edit. Override order per key, first hit wins: user config (`~/.config/gamebus-presenced/`, partial files fine) → installed copy (rewritten by every install) → system data dirs → defaults bundled from the very same file. `GAMEBUS_UMU_API` still wins on top. `build.rs` reads the same file for its build-time fetch - exactly one place a URL is written down. **The daemon reads none of it**: `src/main.rs` cannot reach the endpoints module; network-free by construction. The parser is a hand-rolled TOML subset (`[section]`, `key = "value"`, comments) - a TOML crate would be a dependency for four keys, consistent with the hand-rolled CSV and date code.

## State at end of session (2026-08-07, ~03:30)

- Branch `feat/s9-umu-miss-report` is **9 commits ahead** of master's 27473ac (2457cfe, 9f3f3f3, 2f29211, ca97250, b06757f, 30b9b42, 6589e18, b638404, plus 4c8a16e from the previous evening), **NOT merged - owner decides**.
- All gates green: 13 test binaries (3 bins + 10 integration test files), clippy with `-D warnings` clean.
- Daemon restarted with the merge fix (systemd user unit active since 2026-08-07 01:24, verified); tools installed to `~/.local/bin` (`gamebus-presenced`, `gamebus-presence`, `gamebus-setup`, verified).
- Live stash (verified): Control drafted `umu-870780` across its egs and lutris-keyed entries, all `confirmed-missing`, each carrying the advisory substring-match note.

## Related

- [[wiki/projects/gamebus-presenced]] - project note
- [[wiki/logs/2026-08-06 - gamebus-presenced S6-S9 Naming Layers and umu Stash]] - previous evening: S6-S8 and the stash this pipeline builds on
- [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]] - ADR for the owned-halves merge model (written in parallel with this log)
- [[wiki/concepts/2026-08-06 - Learnings Review]] - the review round that promoted rule §0.7, which both review rounds here applied
- `PLAN.md` §S9b - roadmap source for this log
