---
date: 2026-08-30
type: devlog
tags: [devlog, rust, identity-misses, gamedb, gamebus-gamedb, lutris, steam, verification]
related-projects: [gamebus-presenced]
confidence: high
ai-first: true
---

## For future Claude

[[wiki/projects/gamebus-presenced|gamebus-presenced]] session of 2026-08-30:
the identity-misses feature went from a live bug report through design
(covered in [[wiki/concepts/2026-08-24 - Identity Misses Design]] and the
ADR-009 amendment, both written earlier this same session) to a fully
implemented, live-verified branch. This log covers the narrative arc: what
broke, what got built, and two corrections the owner made mid-session that
are worth remembering as standing rules - **never fake a stash entry**, and
**contract-proven is not live-proven**. Branch `feat/identity-misses`, NOT
merged to master. All facts below verified live 2026-08-30.

## 1. The trigger: one game, two bugs

The owner played Danger Scavenger, a native Linux itch.io game, through
Lutris (flatpak). Two problems surfaced in sequence.

**Bug 1 - wrong name on the bus.** A native game binary Lutris launches
directly registers with GameMode itself, so the daemon's existing
wrapper-only Lutris-ancestor-argv naming layer never ran for it, and the bus
showed the raw executable stem (`Danger_Scavenger`) instead of the title
Lutris's own wrapper argv carried (`Danger Scavenger`). Fixed in `c2c1bf9`
("fix(naming): Name a native game from its Lutris wrapper, not its binary"):
`classify_member`'s non-wrapper branch now also tries
`identify_via_lutris_ancestor`, tagged as a `Wrapper`-class, launcher-sourced
identity so a curated hit can still supersede it. Live-verified on the bus
after reinstall and restart.

## 2. The design and the refinement

The owner's direction: *"gamedb should not just record umu-misses - it
should also make the gamebus data more reliable: mapping it out when it is
not obvious would make sense too."* An ultracode workflow (11 agents: 4
subsystem readers, 3 competing designs, 3 judges, 1 synthesis) picked
"minimal-widen" (23.5 vs 18 vs 15). Full design, plus the amendment where
Claude measured the live machine and replaced the design's `.lutrisgame.json`
assumption with a `pga.db` join:
[[wiki/concepts/2026-08-24 - Identity Misses Design]].

Mid-implementation the owner refined the scope: *"gamedb should probably be
always active. the umu-part probably later on by an internal suggestion or
manual promotion (or when matched to an existing active umu-entry from
another store)."* This became the shape that shipped: the gamedb tab shows
every identity record by default; umu-database participation is opt-in per
entry via a new annotation-half `umu_promoted` field (a date, set with a `u`
TUI key), or one of two existing signals promoted to a suggestion - a
cross-store verification match, or the protonfix scope check finding a fix.
Landed as `7859ace`, with a two-writer regression test proving the promotion
flag survives the daemon's next write (the standard ADR-009 owned-halves
guarantee).

## 3. Implementation: four tasks, isolated worktrees, zero conflicts

A second ultracode workflow split the build into four independently-owned
tasks that integrated with no merge conflicts:

- **Task A** (`src/enricher.rs`): widened the launch-hook guard from "umu
  marker only" to "umu marker OR a `lutris:`/`heroic:` merge key"; reads
  `STORE`/`GAME_NAME`/`GAME_DIRECTORY`/`HEROIC_APP_SOURCE`/`HEROIC_APP_NAME`
  from the process environment and `.lutrisgame.json` for a Lutris marker
  codename; records via a new `note_launch_with(..., LaunchFacts{...})` API,
  landed first as its own frozen-contract commit (`7981d88`) so the four
  tasks could code against it in parallel.
- **Task B** (`src/setup/umu_misses/*`): `umu_candidate()` gates
  drafting/export/PR-check to promoted-or-suggested entries only; `--verify`
  still runs over every umu miss, since that is what produces the
  suggestions.
- **Task C** (`src/setup/gamedb/*`): grouping falls back to `launcher_name`
  before title; `game_exe` relaxes the `.exe`-only rule for native runners
  while staying strict for Proton/umu entries; a filter (`f`, cycling
  all/gaps/umu/weak) on the always-visible gamedb tab.
- **Task D** (new `src/setup/lutris_library.rs`): reads Lutris's `pga.db` via
  `rusqlite`, version-pinned to match the copy already in gamedb-build's
  lockfile so `Cargo.lock` gained zero new packages; matches `(name, store)`
  and can fill codenames.

**Integration finding:** the setup tool cannot durably write any
resolution-half `Miss` field at all - its own `merge_from_disk` re-adopts
that whole half from disk before every persist, flattening any
resolution-half write before the daemon is even involved. Task D's fill
therefore had to land as annotation-half `codename_override` plus a new
`codename_override_source` (provenance, cleared by a hand edit) rather than
the originally-sketched resolution-half field. Recorded in
[[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]]'s
2026-08-30 amendment.

Full gate green after integration: 418 tests (up from 380 at the base
commit), clippy clean under both `--all-features` and `--no-default-features`,
daemon dependency tree byte-identical. End-to-end proof against a
scratch-HOME copy of the owner's real stash: candidacy tagged correctly per
entry, the Lutris fill correctly found nothing to fill (the owner had
already hand-set Control's GOG codename, and the fill correctly refuses to
overwrite a human edit), and the Danger Scavenger page exported byte-exact
and passed both the Python and Rust lints.

## 4. The second bug, live: Steam published nothing at all

The owner: *"running the same game from steam now doesn't show it at all."*
The same game, launched via Steam instead of Lutris, produced zero bus
activity. Root cause: the `/proc` scan's new-group gate required
`detectable.json` to know the game - by exe or by appid - before creating
any record, and this Steam app (Danger Scavenger, appid `1169740`) isn't in
Discord's detectable list.

Fixed in `8ad5fb8` ("fix(naming): A Steam install names itself from its own
appmanifest"): Steam's own `steamapps/appmanifest_<appid>.acf` carries the
official name beside every install. The gate and `classify_member` now
accept a steamapps-library exe whose manifest resolves, needing no database
at all - a new `IdentitySource::SteamManifest` variant keeps this labeled
distinctly from a Discord-curated hit. Live-verified: the game appeared
correctly named on the bus after reinstall and restart, with no stash entry
yet - deliberately, see below.

## 5. "Don't fake it" - the correction that shaped the next commit

The owner, before any stash write existed for the Steam path: *"dont stash
an entry - it should come up naturally through the engine later. dont fake
it."* So the Steam-manifest fix initially recorded nothing - visibility
only. The owner then asked why the gamedb tab didn't carry the update, and
correctly diagnosed the distinction: recording what the engine genuinely
learned, through the *same* launch-hook door Lutris and Heroic already use,
is not faking; fabricating a stash entry from the scan hack would be. The
rule is about provenance, not about whether a write happens.

Claude added a genuinely gated write in `2ee3289` ("feat(gamedb): Carry a
Steam launch's knowledge onto the game's page"): a steam-keyed launch is
recorded only when the appid is absent from detectable.json (the mapping is
knowledge gamedb actually lacks) *and* Steam's own manifest names it
(nothing guessed). Ordinary Steam launches and shortcut appids (no
manifest) leave zero trace.

This also exposed two real bugs in the gamedb fold, both fixed in the same
commit:

1. `ID_STORES` (the list of stores whose codename identifies a page) had
   never included `steam`, because historically a Steam appid could only
   arrive via a drafted sku, never a store entry - so a lone `steam` store
   entry read as "nothing identifies it."
2. The itch.io copy and the Steam copy of Danger Scavenger, both running the
   identical executable, were becoming two separate unmerged gamedb
   candidates instead of one page with two store entries. Fixed with a
   cross-store fold: any two candidate groups sharing an executable merge
   before page assignment.

gamebus-gamedb's own schema/CONTRIBUTING gained a `steam` value in its
`source` vocabulary (`b34e4f4` in the data repo), since "Steam's own
manifest" is neither `detectable` nor `manual`.

## 6. Contract-proven is not live-proven

The owner later asked directly whether earlier demonstrations had been
"patching up the daemon's data to look right." The honest accounting:
the daemon-side write path had only ever been proven by unit tests
asserting the exact fields it writes - never exercised against the owner's
real, running daemon. Every demo of the gamedb fold, export, and lint had
run against scratch-HOME copies where Claude hand-injected an entry in the
contract's shape, to demonstrate the setup-tool half without waiting for a
live game session.

Claude named the distinction explicitly, and it is worth keeping as a
standing verification rule: **"contract-proven"** (a unit test asserts the
shape a write produces) is not the same claim as **"live-proven"** (the
actual running system produced it). The one remaining unproven claim - the
Steam stash-recording arm - was flagged rather than hand-seeded; the owner
relaunched the game for real, and the daemon wrote the exact contract-shaped
entry live (`steam:1169740`, `codename_source: steam-manifest`, ...),
closing the gap.

## 7. Frozen ids in the wild: the Late Bloomer case, for real

Because the owner had already hand-created `gamedb/games/danger-scavenger.toml`
with canonical id `itchio-926077` (from before the Steam fix existed) but had
not yet published a data release containing that page, the gamedb tab's
listing - which derives an id fresh from precedence against the currently
*published* index, which doesn't have this page - showed `steam-1169740`
instead: a Steam appid outranks a store codename in the precedence order.

The owner correctly diagnosed this as provisional display, not id
promotion: once a release ships with the itchio-keyed page, the id freezes
at `itchio-926077` forever (ids are never re-pointed, per
[[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]]), and
the Steam identity becomes a `+steam/1169740` enhancement addition on the
same page - which Claude had separately already verified by building a
local index from the owner's checkout. This is the "Late Bloomer" scenario
from the original id design, occurring for real for the first time.

## 8. State at end of session

Branch `feat/identity-misses`, 15 commits on top of master, not merged:

```
c2c1bf9  fix(naming): Name a native game from its Lutris wrapper, not its binary
7981d88  (base LaunchFacts/note_launch_with contract)
7859ace  (umu_promoted opt-in promotion)
69ac2a5  (Task A: daemon records identity misses)
8c9806f  (Task D: Lutris library source)
111b6ac  (Task B: umu pipeline gating)
fa364de  (Task C: gamedb fold)
a8d3358  (Lutris library integration wiring)
8ad5fb8  fix(naming): A Steam install names itself from its own appmanifest
2ee3289  feat(gamedb): Carry a Steam launch's knowledge onto the game's page
7e2a3fb  docs(vault): this session's records
bf7e7de  chore: replace all typographic dashes with ASCII hyphens
c55ad62  fix(setup): state-honest footer keybinding hints, v works on gamedb
46640be  feat(setup): the pick offers Lutris library identities next to Heroic's
e0e8e28  docs(vault): record the completion pass
```

The owner reinstalled and restarted the daemon multiple times mid-session to
verify live behavior at each stage, and confirmed the footer fix live.
gamebus-gamedb (the data repo) also gained `b34e4f4` (steam source
vocabulary).

## 9. Same-evening completion pass

Four follow-ups closed the feature out, all with `Assisted-by` trailers
(the repo's AI-disclosure convention, EU AI Act):

1. **ASCII hygiene.** Every em- and en-dash in the repo replaced with plain
   hyphens (76 files, zero hidden characters found) - a standing rule now:
   only ASCII hyphens, ever. Committed separately from the vault records so
   the dash strip stays reviewable on its own.
2. **State-honest footer.** The gamedb tab's keybinding hints could
   advertise keys that do nothing: `v` was named by several flows reachable
   from that tab ("press v to fetch it first") but bound on the misses tab
   only. `v` now verifies from both stash tabs, and the footer drops the
   entry verbs whenever no row is selected (empty stash, or a filter hiding
   every row - `f` stays, it is the way back). Footer selection extracted
   into a pure `footer_keys()` so the per-state lines are unit-tested.
3. **The parked pick wiring.** `lutris_library::candidates` had shipped
   behind `#[allow(dead_code)]`, "not wired yet". The `p` pick now joins
   the Lutris library on equal footing with Heroic's: a Lutris row is an
   identity candidate (Enter writes the store+codename overrides, source
   "your Lutris library", cross-store rows greyed like Heroic's), and a
   pga.db that exists but cannot be read is a warning naming the path
   rather than a failure. Covered by an env-locked integration test
   (scratch umu CSV + scratch pga.db; the shared ENV_LOCK serializes
   against the lutris_library tests).
4. **Trailers.** The session's commits got `Assisted-by: GLM 5.3
   <noreply@z.ai>` - filter-branch twice taught the lesson that a msg-filter
   must never let `grep -q` eat stdin before `$(cat)` reads it.

## Related

- [[wiki/concepts/2026-08-24 - Identity Misses Design]] - the design and its
  measured amendment
- [[wiki/decisions/adr-009-umu-miss-stash-two-writer-owned-halves]] - the
  2026-08-30 data-model amendment
- [[wiki/decisions/adr-010-corrections-repo-toml-pages-built-artifacts]] -
  the frozen-id design this session confirmed live
- [[wiki/logs/2026-08-23 - gamebus-gamedb Published and the Export Loop]] -
  the export/enhancement machinery this session extended
- [[wiki/projects/gamebus-presenced]]
