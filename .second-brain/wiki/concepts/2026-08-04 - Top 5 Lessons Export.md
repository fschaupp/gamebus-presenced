# Top 5 Lessons — gamebus-presenced (2026-08-04)

> Extracted from the second learnings review of the gamebus-presenced project vault. All five were discovered and reinforced in a single day of intensive development (S0-S4, 61 unit + 6 integration tests, 8 ADRs).

## 1. Verify live state before trusting docs or assumptions

Two design-doc assumptions broke against the live system during implementation. D-Bus object path elements reject hyphens at runtime — the design doc used `pid-<pid>`. GameMode signals carry per-game object paths, not executables — the design doc claimed the executable was in the signal payload. Both were discovered by reading the real API, not the docs.

The principle extends to instrumentation: the ancestor-walk join was instrument-first (debug log before code) because we didn't know the miss rate. The detectable.json path-suffix matching was discovered empirically from the data — 83% of entries are path-prefixed, which the basename-only index missed entirely.

## 2. One record per key — prefer HashMap over pairwise merge

When deduplicating by a shared key across a process hierarchy, a single `HashMap<key, representative>` mapping each key to exactly ONE record (decided by `tree_depth` — deepest pid wins) is simpler and more robust than pairwise ancestor-walk merging.

The pairwise approach converged incorrectly for siblings (same-depth processes like `wineserver` and `re2.exe` — both children of `pv-adverb`). Results depended on scan order. Utility processes absorbed the actual game. Multiple fix attempts (`same_tree`, `find_all_related`, `tree_depth` comparison) all shared the fundamental problem: pairwise merging is O(n²), order-dependent, and fragile for same-level items.

The breakthrough was the simplest possible design: one HashMap, one record per key, depth decides. O(1) per new item, deterministic, order-independent. "Why not just a HashMap with appids as key?" — the owner's suggestion was exactly right.

## 3. Integration-test against the real thing

Real clients and fixtures caught 11 bugs that unit tests missed across four development slices. Genuine `discord-rich-presence` RPC client (not a mock). `gamemoderun sleep 30` (not a fake GameMode registration). Recording fixture upstream for proxy byte-identity (not a hand-crafted pipe). Live games (Brotato, Amnesia, Resident Evil 2) for process-identification verification.

The bugs caught: merge direction reversed, `SteamAppId=default` false positive, detectable.json path-suffix gap, ~20-record explosion per game, utility processes absorbing the actual game, cache-adopted records unremovable, `ppid > 1` excluding init, naming DB blocking startup, and more. None of these would have surfaced from unit tests alone — they required real process trees, real env vars, real D-Bus signals, and real Wine wrapper chains.

Tests skip gracefully when the environment isn't available (no session bus, no gamemoded, real Discord running). The skip is a feature, not a failure — it means the test only runs when it can test the real thing.

## 4. Let the test environment's reality shape assertions

`libgamemodeauto` is preloaded globally on this machine — every process registers with gamemoded. `HasActivity == false` is never assertable in tests because unrelated processes hold GameMode-sourced activities. `SO_PEERCRED` on the test's own IPC socket yields the test process's pid (used deliberately for the same-pid correlator test). gamemoded's dead-client reaper takes 4-18 seconds — tests use explicit `UnregisterGameByPID` for determinism. Discord's proxy rejects test client_ids — tests skip when a real Discord is running.

Each of these is environment-specific, not a code bug. Fighting the environment (asserting `HasActivity == false` when games are running) produces false test failures. Accepting the environment (removing the overly broad assertion, skipping when Discord is present) makes tests reliable.

## 5. Reuse battle-tested code; check the licence first

The `rsrpc` crate's `rsrpc::cmd` model handles Discord's wire format (serde edge cases, timestamp normalisation via `fix()`). Hand-writing this would have been weeks of subtle bugs. The crate is MIT-licensed — depend on it, don't vendor it.

The `pog5/rsrpc` fork is GPLv3 — learn from it, never vendor it. `umu-launcher` and `umu-database` are GPL-3.0 — query/interop only. `discord-rich-presence` is a dev-dependency for integration tests — genuine RPC client, not a mock.

The corollary: check licences before reuse. The design doc's "small dependencies" guidance doesn't mean "no dependencies" — it means "no dependencies that drag a browser engine or an HTTP stack into a session daemon." `ureq` for the build-time fetch and CLI is fine. `rsrpc`'s transitive deps (`simple-websockets`, `tokio-tungstenite`, `interprocess`, `chrono`) compile but never execute — an accepted cost of crate reuse.
