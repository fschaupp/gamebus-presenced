---
date: 2026-08-06
type: export
tags: [export, lessons, learnings-review]
ai-first: true
---

## For future Claude

Shareable prose export of the Top 5 lessons from
[[wiki/concepts/2026-08-06 - Learnings Review]] (period: the S5 setup-tool and
S4f reliability sessions, 2026-08-05/06). The body below is written to be
copied out of the vault verbatim.

---

# Five lessons from making a presence daemon that cannot break

*gamebus-presenced, S5 + S4f - a setup TUI, and the fix for two live failures.*

## 1. Depth is not identity

Our merge rule said "the deepest process in the tree wins" - a reasonable
proxy for "closest to the game," until the Steam runtime made it a lie. The
deepest pids on a Proton launch are short-lived helpers (`pv-adverb`,
`inspect-library`), and with `libgamemodeauto` preloaded, every one of them
registers as a game. The record migrated onto a helper, the helper exited
0.3 ms later, and the record died while the game played on. The fix wasn't a
better depth heuristic - it was admitting depth is not identity: candidates
now carry an evidence class (identified game > plain process > known helper),
only a strictly better class dethrones a live representative, and depth
breaks ties within a class only.

## 2. Adversarial review catches your own wrong fixes

Four review rounds, four sets of confirmed findings - the best one refuting
the author's own fix for a confirmed bug. The umask fix (`DirBuilder::
mode(0o755)`) looked explicit and passed review by reading; a skeptic
*measured* it and got 0700, because `mkdir(2)` masks its mode argument
exactly like the code that had been "fixed". The pattern that works: finders
propose with a concrete failure scenario, skeptics try to kill each finding,
and only survivors reach a human. Verify the mechanism, never the intention.

## 3. Records die with evidence, not with a pid

A reliability service must not tie a record's life to whichever process it
happened to anchor on. When our representative pid died, the record died -
even though the game's own registrations were still live. Now removal
requires the *last evidence* to be gone, a dead anchor just triggers a
re-election at the next tick, and teardown degrades the record in place so
consumers see exactly one removal per session. If your output must not flap,
make absence of evidence - not death of a proxy - the removal condition.

## 4. Isolate the identity in tests, not just the filesystem

Our integration tests isolated `XDG_RUNTIME_DIR` and thought they were
hermetic. Then the daemon got *installed*, and every test failed: the real
daemon owned the D-Bus name, the test daemons died with `NameTaken`, and the
assertions ran against somebody else's process. Path isolation is not
identity isolation. Each test now gets a private `dbus-daemon --session` -
which as a bonus activates its own empty-state `gamemoded`, removing the last
dependency on what the developer happens to be playing.

## 5. Measure, don't argue

Every claim that decided something this week was settled by observation:
`systemd-analyze --user unit-paths` before choosing `/usr/local`; the
daemon's journal before diagnosing the merge bug; a 10-line program under
`umask 077` before trusting a permissions fix. The claims that were merely
argued - including our own fix comments - are the ones that turned out wrong.

---

*From the 2026-08-06 learnings review; 150 tests, 8 ADRs, two live-verified
fixes.*
