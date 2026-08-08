# Plan: gamebus-presenced

**Note**: the reasoning, the D-Bus surface and the source-by-source analysis
live in [`docs/design/gamebus-presence.md`](docs/design/gamebus-presence.md).
This file is the roadmap and the status line.

## Status: S0-S4f and S6-S10 done and verified (S9c matchup + mislabel defense 2026-08-08). S5 (setup tool) landed 2026-08-06, STAGED.

Repo created 2026-08-03. The original S1 ("D-Bus surface + GameMode source")
was split: the surface was extracted as S0 so the interface could be verified
on the bus before any source existed. S0 and S1 both landed 2026-08-04,
verified against the real session bus and a real gamemoded. S2 landed the
same day, verified with a genuine `discord-rich-presence` client against the
daemon's own `discord-ipc-0`. S3 landed the same day: proxy verified
byte-identical against a fixture upstream, correlator verified by a same-pid
join of a real RPC client and `RegisterGameByPID`, restart cache verified by
SIGKILL + respawn re-adoption.

## Slices

### S0 — D-Bus surface foundation (DONE 2026-08-04)

Extracted from the original S1. `org.gamebus.Presence.v1.Manager` with
`ListActivities`, `HasActivity`, `Version`; the `Activity` type; zbus v4
bindings; service verified on the session bus.

### S1 — GameMode source (DONE 2026-08-04)

The remaining half of the original S1: the GameMode watcher feeding the
surface — `GameRegistered`/`GameUnregistered` on
`com.feralinteractive.GameMode`, seeded by `ListGames`. Per-game details
(executable, registration timestamp) come from the per-game
`/com/feralinteractive/GameMode/Games/<pid>` objects' `Executable`/`Timestamp`
properties, with `/proc/<pid>/exe` as fallback — the signals themselves carry
the game object path, not the executable (design doc corrected). Per-activity
objects at `.../Activity/pid_<pid>` (hyphens are illegal in D-Bus object path
elements), `ActivityAdded`/`ActivityRemoved` signals, `HasActivity` change
emission, and gamemoded availability tracked via `NameOwnerChanged`.
Integration test: `gamemoderun sleep 30` plus explicit `UnregisterGameByPID`;
`busctl --user monitor org.gamebus.Presence.v1` acceptance verified.

Independently useful: pid + executable presence with no Discord code at all,
which is already everything `epaper-hubd` needs.

### S2 — Discord IPC listener, no upstream (DONE 2026-08-04)

Standalone Discord Rich Presence listener: binds `$XDG_RUNTIME_DIR/discord-ipc-0`
(unlinks stale sockets, warns and degrades if a live owner exists), answers
the handshake with READY, echoes commands lock-step, and turns `SET_ACTIVITY`
into activity objects at `.../Activity/discord_<pid>` — pid from `SO_PEERCRED`,
never from client-provided data. The payload model is `rsrpc::cmd` from the
pinned, MIT-licensed [rsRPC] crate (its `fix()` normalises timestamps to
milliseconds); rsRPC's `RPCServer` itself was unusable (no public event hook,
bundles a WebSocket fan-out, blocking threads), so only the transport is
ours: tokio `UnixListener`, per-connection tasks, `libc::getsockopt` for
`SO_PEERCRED`. Mid-session updates re-emit `PropertiesChanged` in place
(`ActivityInterface` is now mutable). Sources were unified under one
`SourceEvent` channel, so the core is source-agnostic. Integration test:
`discord-rich-presence` client → handshake → SET_ACTIVITY → property
assertions → in-place update → clear → removal.

### S3 — Proxy + correlator (DONE 2026-08-04)

Transparent proxy: with a real Discord running (it takes `ipc-1` since we
bound `ipc-0`), every client connection is pumped through verbatim — frames
forwarded whole, responses from upstream, `SET_ACTIVITY` tapped passively for
our own records. Upstream loss mid-connection closes the client connection;
reconnect lands in standalone (S2) mode. Verified byte-identical against a
fixture upstream, no real Discord needed.

Correlator: per-source partial records keyed by pid in `src/correlator.rs`;
the published activity is a derived view. `pid_<pid>` absorbs `discord_<pid>`
on a join (stable identity from the more reliable source), Discord's
human-facing fields win, GameMode owns executable, records degrade in place
when a source leaves and die with the last source. Join is exact-pid; the
umu/Proton wrapper-tree case (GameMode sees the wrapper, Discord connects
from a child) is a documented miss. Verified by a same-pid join of a genuine
RPC client + `RegisterGameByPID`.

Restart cache: published records written through to
`$XDG_RUNTIME_DIR/gamebus-presenced/cache.json` on every change, keyed by
pid + `/proc/<pid>/stat` start-time. Startup re-adopts records whose process
is still alive with a matching start-time (pid-reuse guard). Verified by
SIGKILL + respawn re-adoption test.

### S4 — Enrichment and packaging (PLANNED)

Brainstormed 2026-08-04 (6-question Socratic interview, see
`.second-brain/wiki/concepts/2026-08-04 - Brainstorm - S4 Enrichment and Packaging.md`).

#### S4a — Steam source (reactive via Enricher middleware) (DONE 2026-08-04)

New `src/enricher.rs`: `Enricher` struct sits between sources and correlator
(`sources → Enricher → Vec<SourceEvent> → Correlator`). On
`SourceEvent::Updated` with a pid, probes `/proc/<pid>/environ` for
`SteamAppId`/`SteamGameId`/`UMU_ID`/`STORE`; if found, emits
`SourceEvent::Updated(Source::Steam)`. Steam partial removal tied to last
non-Steam source (Enricher tracks `{pid: Set<Source>}` — pid-reuse safe).
`Activity::from_steam(pid, appid)` maps to `app_ids["steam"]`; correlator
gains a `steam` slot. `tracing::debug!` on Discord-pid join-miss for the
ancestor-walk decision. Integration test: `sleep` spawned with
`SteamAppId=480`, registered via `RegisterGameByPID`, merged record carries
`app_ids["steam"]`.

#### S4b — Naming enrichment (enrichment-only fallback) (DONE 2026-08-04)

`build.rs` fetches `detectable.json` from Discord's `applications/detectable`
endpoint (23858 entries, 12.3MB); ships as an installation data file
(`$PREFIX/share/gamebus-presenced/detectable.json`, not binary-embedded).
`gamebus-presence fetch-detectable` CLI refreshes to `$XDG_CACHE_HOME`.
(Superseded by S10: no build-time fetch at all — the install plan runs the
freshly installed CLI's `fetch-detectable` as its last, best-effort step.)
Naming precedence: Discord name > detectable.json lookup (by appid or
executable) > executable stem. Anti-goal: never overrides a more
authoritative source. umu-database as cached secondary (protonfixes-scoped,
GPL-3.0, query-only). Naming DB loaded after sources spawn to avoid blocking
the Discord listener.

#### S4c — Packaging (DONE 2026-08-04)

`gamebus-presence` CLI binary (`src/bin/gamebus-presence.rs`): `monitor`
pretty-prints bus state (activities, sources, names, appids);
`fetch-detectable` downloads Discord's detectable.json to `$XDG_CACHE_HOME`.
systemd user unit (`data/gamebus-presenced.service`) and D-Bus activation
file (`data/org.gamebus.Presence.v1.service`) for session-start activation.

#### S4d — Ancestor-walk join (DONE 2026-08-04)

Bounded ancestor-walk (ppid chain, `MAX_ANCESTOR_DEPTH` = 10) for the
umu/Proton wrapper-tree case. The Enricher tracks `{pid: steam_appid}` and,
on a new Steam probe, checks if another tracked pid shares the same appid
AND is in the same process tree (via `is_ancestor` ppid-chain walk). The
descendant absorbs the ancestor (closer to the actual game process). Two
bugs found and fixed during live testing with Brotato: (1) merge direction
was reversed (ancestor vs descendant), (2) cache-adopted records have no
correlator partials, so `drop_partial` couldn't remove them. Also fixed:
`SteamAppId=default` (Steam client processes) no longer treated as a game
appid — only numeric values accepted.

#### S4e — Game identification + Steam process scan (DONE 2026-08-04)

Live testing with Brotato, CoD: Black Ops Cold War, Amnesia: The Bunker,
and Resident Evil 2 surfaced eleven gaps, all fixed. (1) detectable.json
path-prefixed entries (83% of the DB) missed — basename-bucketed index with
path-suffix matching + backslash normalisation. (2) Wrapper processes
unidentified — three-layer identification: wrapper cmdline, connected
descendant walk (exe + Wine cmdline), sandbox-family scan via the umu
`var/tmp-XXXXXX` cmdline token (Flatpak-portal severs the tree). (3)
Delayed game launches — `unresolved_wrappers` retried every 15s via a
main-loop tick. (4) Games without GameMode invisible — bounded
`/proc/*/environ` Steam-appid scan in the same tick. (5) Steam scan
exploded into ~20 records — `identify_process` filter for utility
processes, then **replaced pairwise ancestor-walk merge with
`appid_records: HashMap<String, u32>`** (merge key → deepest pid, one
record per key, `tree_depth` decides). (6) Merge key generalised beyond
Steam — `steam:<appid>` / `lutris:<uuid>` / `umu:<id>`. (7)
`SteamAppId=0` rejected. (8) Tracing filter overrode RUST_LOG. (9) Cache
re-adopted stale Steam-only records. (10) Wrapper executables shown as game
names. (11) Discord integration tests fail when real Discord is running.

### S5 — Setup and status tool (STAGED 2026-08-06)

`gamebus-setup`, a third binary: a terminal tool that reports what is working,
installs the daemon, and fixes what it finds. S4c shipped the two unit files but
nothing that installed them, and several failure modes were invisible — a
`detectable.json` found only via the compile-time `OUT_DIR` path (so a copied
binary silently loses naming), and a `discord-ipc-0` already owned by another
process, where `bind()` warns once at startup and then returns `Ok(None)` — no
`SourceLost`, nothing on the bus, so anyone who was not reading the log at the
moment it started sees a daemon that simply never reports Discord.

Structure: `probe()` does the I/O and returns plain data; `rows()`/`overall()`
are pure functions from that data to what is displayed, including the remedy
each problem offers, so all the judgement unit-tests without a bus, a terminal
or root. Actions are likewise planned as data (`plan()` is pure) before
`execute()` runs them, which makes `gamebus-setup plan` a real dry run and lets
the confirm screen list the exact writes.

Decisions: unit files are rendered from `const` templates pinned to
`data/*.service` by a golden test — the D-Bus activation file's `Exec=` takes no
specifiers, so a user-level install *must* generate both. systemd facts come
from the `systemctl` subprocess (the same command the README tells the user to
run, and `--global enable` has no D-Bus equivalent); bus facts come from zbus.
A system install re-executes `apply --privileged-only` under `pkexec`, so
ratatui never runs as root; `layout()` for the system target reads no
environment, because `pkexec` scrubs it. Escalation is offered for the *system*
target only — a user action escalated the same way would resolve `$HOME` to
`/root` in the child and install where nobody agreed to.

ratatui sits behind a default-on `setup` feature; `--no-default-features`
builds the daemon and CLI alone. The zbus client proxies moved to
`src/client.rs`, shared by `gamebus-presence` and `gamebus-setup` via `#[path]`,
so the property reads have one implementation and two renderers.

The system prefix is `/usr/local`, not `/usr`: `/usr` is what a distribution
package owns, and a hand-run installer writing there collides with any future
package and fails outright on image-based distributions. This needed one change
in the daemon — `find_detectable_json()` now walks `XDG_DATA_DIRS` (default
`/usr/local/share:/usr/share`) instead of hardcoding `/usr/share`, which is
strictly backwards compatible since `/usr/share` remains in the default.

**STAGED — what is proven and what is not.** Proven: 48 pure unit tests
(path layouts, the golden test pinning the rendered units to `data/*.service`,
the `systemctl` output parsers, the whole `rows()`/`overall()` judgement layer,
plan step lists, the atomic replace); 8 CLI integration tests (a dry run writes
nothing, the system plan names `/usr/local` and says it needs root, `apply`
refuses without `--confirm` and refuses a privileged target rather than failing
halfway); a full install → status → uninstall round trip into a throwaway
`HOME`, including the real `systemctl --user daemon-reload` and D-Bus
`ReloadConfig`; the status probe against the real session bus with a real
`gamemoded` and a real `RegisterGameByPID`; and the TUI driven under a pty
(status view, confirm modal listing the exact writes, cancel, monitor view
showing a real activity).

Not yet proven, and the reason this is STAGED rather than DONE: an install into
a real `~/.local` with the daemon starting from the installed unit; D-Bus
activation starting it on demand; autostart surviving a logout; the `--target
system` path and its `pkexec` escalation, which has never run; and the
`ETXTBSY` reinstall-over-a-running-binary case against a real running daemon.

### S4f — Game groups: reliable identity under helper churn (DONE 2026-08-06)

Two live failures forced this. Brotato (native, Steam): "deepest pid wins"
migrated the record onto a transient pressure-vessel helper that exited 0.3ms
later, and the record died with it while GameMode still held two live ancestor
registrations — the 15s rescan could not recover it because `identify_process`
required a NamingDb hit and Discord's entry lists only the Windows
`brotato.exe`. Amnesia (Lutris/umu): the merge replaced an *identified* record
("Amnesia: The Bunker") with an unidentified `i386-linux-gnu-inspect-library`
helper, twice, ending with an empty bus while the game ran. Root cause: depth
overrode identification, and every helper was a full-strength merge candidate
because libgamemodeauto registers them all with GameMode.

The fix (designed by a three-way competition — surgical / game-identity /
defensive — judged from consumer and maintainer personas): **game groups** in
the new pure `src/group.rs`. All pids probing to one merge key form a group;
members carry a class (Helper < Plain < IdentifiedWrapper < GameProcess,
wrapper list consulted on the *raw* exe); the first member becomes
representative and only a strictly-greater class dethrones it (one
publish-first Migrate pair, at most once per session). Equal-or-lower
candidates are **absorbed** — they never reach the correlator, so helper churn
produces zero bus traffic. A dead representative does not kill the record
while any member still holds evidence (**deferred migration**: the sweep
re-elects at the next tick); removal happens only when the last evidence goes,
Steam partial first so the record degrades in place and dies with its last
source — exactly one ActivityRemoved per session (a recorded deviation from
the spec's literal removal order, in favour of its own no-flapping promise).
Identity is monotone: a resolved name is never overwritten by an unidentified
member. The rescan re-seeds from ListGames (a `Notify` into the gamemode
source) and its new-group gate accepts `/steamapps/` + a resolving appid, so
native binaries with `.exe`-only detectable entries recover without a restart.
The correlator gained publish-dedup (no-op republishes emit nothing);
`pid_<pid>` identity per ADR-007 is unchanged — consumers need no changes.

Verified: 150 tests including truth-table units for every group rule and two
private-bus integration tests (`helper_churn_never_reaches_bus`,
`record_survives_rep_death`); three adversarial audits, four confirmed
findings fixed (wrapper cmdline inflation, removal order, grouped steam-partial
reap, scan-group Since pinning); live against the running Amnesia session.

### S6 — MPRIS naming hints (DONE 2026-08-06)

The naming half of the deferred MPRIS decision, and only that half. A new
watcher (`src/sources/mpris.rs`) follows `org.mpris.MediaPlayer2.*` bus names
(ListNames seed + NameOwnerChanged), resolves each player's kernel-verified
pid via `GetConnectionUnixProcessID`, reads its `Identity` property, and emits
a `SourceEvent::NameHint { pid, name }`. Hints are consumed entirely by the
enricher: they create no records, keep none alive, appear in no `Sources`
list, and never reach the correlator.

Precedence extends the S4b anti-goal: a hint replaces only a *default* name —
empty, or the executable stem `from_gamemode` sets — one rung above the stem
and below Discord, group identity, and detectable.json. The stem case is the
point: a native binary like `Brotato.x86_64` whose detectable entry lists only
the Windows exe finally gets a human name when the game (or anything in its
group) exposes MPRIS. Group records accept any member's hint while their
identity is unresolved; an in-place refresh applies late hints immediately,
and the ListGames reseed converges the rest within 15s. Names are monotone —
a closing player never un-names a record; dead pids are pruned from the hint
map on the tick. Two fixes surfaced by the tests: `apply_naming` used to
early-return without a naming DB (skipping hints exactly where they matter
most), and "fill only empty names" was useless against stem defaults.

Two limits found live the same evening: sandboxed players resolve to the
`xdg-dbus-proxy` pid, so their hints join nothing (documented miss — no
fabricated joins across sandbox boundaries), and a Lutris-launched
`UbisoftConnect.exe` had neither a detectable.json entry nor any MPRIS player
— which produced **S6b**: identification layer 4 reads the game title straight
from a `lutris-wrapper` ancestor's argv (`lutris-wrapper <title> <n> <n>
<command…>`, counters found right-to-left so titles ending in digits survive),
bounded by the same `MAX_ANCESTOR_DEPTH` walk, and — unlike layers 1-3 —
working without a naming database. Its live verification then exposed a third
gap: the tick scan's adoption was gated on a *Steam* appid, so lutris/umu-keyed
game processes (`SteamAppId=default`, `LUTRIS_GAME_UUID=<uuid>`) were invisible
to it — Far Cry Primal ran unadopted while its record kept the launcher's name.
The scan now probes every merge-key type for adoption into an existing group;
a Steam appid is required only where it always was, creating a new group. With
that, the launcher-named record upgrades to the game (`fcprimal.exe` is in
detectable.json → GameProcess class → dethrone + identity upgrade) within one
tick of the game process appearing.

Verified: 88 unit tests (+6 hints, +3 lutris incl. a real spawned
wrapper-tree walk) and a private-bus integration test serving a genuine MPRIS
player (`zbus::interface`, real
`Properties.Get` wire) from the test process, joined by pid via the bus's own
gamemoded — record named after the player's Identity, MPRIS absent from
`Sources`, name survives player exit.

### S7 — Publication hygiene (DONE 2026-08-06)

Three fixes born from one evening's journal (107 pid publishes in 4 hours).

**Pattern blacklist.** `is_wrapper_executable` became three rules over a
backslash-aware basename: literals (shells, launch plumbing, the Wine service
set — `wineserver`, `services.exe`, `conhost.exe`, … — and `steamwebhelper`),
prefix families (`steam-runtime-`, `pressure-vessel-`, `pv-`, `srt-`,
`{i386,x86_64}-linux-gnu-`), and version-trimmed interpreters
(`python3.13` → `python`). Live symptom fixed: a `/usr/bin/python3.13`
wrapper had classified GameProcess from the game path in its own argv —
pinning its group so the real game could never dethrone it. Deliberately off
the list: `wine64-preloader` (Wine games are only identifiable via the
cmdline layer) and `sleep` (integration fixture, stem asserted).

**Publish-once-named.** An ungrouped GameMode record whose name is empty
after enrichment — a blacklisted wrapper or unreadable exe, what the monitor
showed as "(unknown)" — is withheld from the bus and publishes the moment
anything names it (hint, late identification, Discord join, or the 15s
reseed), with its original `Since`. Its death while withheld is fully silent:
the µs-lived keyless-helper corpses now produce zero bus traffic. Grouped
records are never withheld (a merge key is game evidence); stem-named records
still publish (the S1 pid+executable contract). Owner decision 2026-08-06.

**No name regression.** At game close the dying process re-emits with an
unreadable exe, and the update used to clear the published name to
"(unknown)" before removal. The correlator now enforces design rule 2 for
names: an empty merged name carries the published name forward, so the record
stays truthful until its single ActivityRemoved.

Also: `merge_key_from_environ` accepted `SteamAppId=0` while
`find_steam_appid` rejected it — every non-Steam title pooled into one bogus
`steam:0` group whose members dethroned each other all evening. One
`valid_steam_appid` definition now serves both, and a zero Steam id falls
through to the real Lutris/umu key.

Verified: 97 unit tests (+8: merge-key zeros, two pattern truth tables, five
withhold paths, name monotonicity) and `tests/withheld_publication.rs` on the
private bus — a registered keyless `sh` wrapper never appears and never
produces a removal while a plain `sleep` control publishes and dies cleanly.

### S8 — Heroic support (DONE 2026-08-06)

Control, launched through Heroic (Epic/legendary), was invisible: its environ
carries `SteamAppId=0` and `GAMEID=umu-0` — both correctly rejected — and no
Lutris UUID, so the whole launch tree was keyless; its blacklisted wrappers
were withheld (S7 working as designed) and nothing remained to publish.
`HEROIC_APP_NAME=<codename>` is present in every process of the tree and is
now the merge key of last resort (`heroic:Calluna`; any Steam/Lutris/umu key
wins). Group identity comes from the launcher's own install records:
legendary's `installed.json` maps codename → display title
(`Calluna → "Control"`), tried at the Flatpak and native config paths — local
files, no network, launcher-curated, Wrapper class so a detectable.json hit
on the real game process still upgrades. Considered and rejected: the
umu-database — it is protonfixes-scoped and does not know this game
(`codename=Calluna` → `[]`, verified live), and `GAMEID=umu-0` is by
definition the umu-miss case; its one useful mapping (numeric umu id ⇒ Steam
appid) has been implemented since S4.

### S9 — umu-database miss report (DONE 2026-08-07)

Every launch that goes through umu without a database entry (`GAMEID=umu-0`,
`UMU_ID=umu-default`) is a gap in the shared umu-database — and by the time a
session ends, this daemon has usually worked out what the game was. S9 turns
that into a contribution pipeline (owner's idea): the enricher stashes each
miss at `$XDG_DATA_HOME/gamebus-presenced/umu-misses.json` — store guess
(`HEROIC_APP_SOURCE`, else install-path heuristics, else `none`), codename
(`HEROIC_APP_NAME` — for EGS exactly the App Name the database wants), the
resolved title with its source and a confidence label (high: launcher config
or detectable hit; medium: wrapper layers; low: hints/stems), and the game
exe. Resolutions only upgrade, never downgrade; writes are atomic; the daemon
stays network-free.

`gamebus-setup umu-misses` lists the stash for review;
`--export` emits submission-shaped CSV matching the database's own header.
Low-confidence and unresolved entries are listed but excluded from export.
The setup TUI shows the same stash as its third view (a tab bar under the
header names all three; Tab cycles them): per-entry resolution evidence,
verification verdict, and drafted id. The pane drives the flows on labeled
keypresses — `v` fetch+verify (the one network verb, labeled as such in
the footer), `s` cycles a store correction (stored as a setup-owned
override so the daemon's writes never revert it; verification and exports
read the effective store), `a` assigns a umu id by hand, which passes the
same mandatory collision check as every draft before it saves. Exporting
stays a CLI invocation.

### S9b — verify, draft, and export umu-database submissions (2026-08-07)

The stash becomes a real contribution pipeline, all of it in `gamebus-setup`
(the daemon stays network-free). The file now has TWO writers, each owning
half of every entry — the daemon the resolution half, the setup tool the
annotation half (verification, drafted id, PR mark) — and every persist
merges the other writer's half from disk first, so neither a launch after
`--verify` nor a `--verify` during a session loses the other's work. A
stash that fails to parse is reported and never written over.

- **`--verify`** checks every miss against the database, local copy first —
  `--db <file>` or `GAMEBUS_UMU_DB` (the git checkout's CSV or the API's
  JSON dump both parse), else the `--fetch` cache — then confirms whatever
  the local copy did not settle against the public API
  (https://umu.openwinecomponents.org/umu_api.php, overridable via
  `GAMEBUS_UMU_API` for tests and self-hosting; all remote endpoints —
  Discord's public detectable endpoint, the umu API, the upstream repo and
  its open-PRs URL — live in `endpoints.toml`, shipped with the install and
  overridable per-key from `~/.config/gamebus-presenced/`, so an upstream
  API change is a config edit; build.rs reads the same file). Three
  verdicts, persisted
  into the stash: `already-in-database` (store+codename found — the launcher
  missed, not the database), `cross-store-id` (the title exists under
  another store; the id to reuse), `confirmed-missing`. No local copy *and*
  no API is an honest error with the stash untouched. Caveat found live:
  the API's title lookup substring-matches (`?title=Control` returns Ground
  Control's ids) and returns no title to compare against, so API title hits
  are advisory notes only — `cross-store-id` comes exclusively from exact
  local-title matches and the drafting collision check.
- **Id drafting** for confirmed-missing entries, per the database's own
  rules, strongest basis first: the title's Steam appid from
  detectable.json's `third_party_skus` → `umu-<appid>`; else a codename
  that carries at least one letter → `umu-<codename>` (Proton parses a
  numeric second part as a SteamAppId, so pure-numeric GOG codenames never
  become ids); else the standalone slug `umu-<lowercased-title>`. Every
  draft is collision-checked against the full database — a mandatory step,
  which is why no local database means no drafts: a same-title collision IS
  the cross-store id (used as such), a different-title collision discards
  the draft and records the conflict.
- **`--fetch`** refreshes the cached full dump
  (`$XDG_CACHE_HOME/gamebus-presenced/umu-database.json`) — one request to
  the bare endpoint, validated by parsing before it replaces the cache,
  never implicit.
- **`--export`** now prefills verified/drafted ids (the NOTE column says
  which basis and when it was collision-checked); `umu-FIXME` remains only
  where nothing could be drafted safely. Launcher-side misses and
  possibly-already-submitted entries are held back and listed with reasons.
- **`--export-md [file]`** writes a slim merge request: title line, one
  paragraph of provenance, the CSV rows in a fenced block, a per-entry
  evidence list, and a checklist mirroring the README's rules.
- **`--check-prs`** (opt-in, best-effort) scans open upstream merge requests
  via the unauthenticated GitHub API for rows matching our entries —
  matched entries are annotated and held back from exports. Failures are
  reported and non-fatal.

#### S9b conformance fix (DONE 2026-08-08)

The first real submission round (Control, submitted upstream as PR #151,
pending as of 2026-08-08) showed the exports drifting from the database's
own rules. Three fixes, all in the
export path: the CSV NOTE column is now always empty — it belongs to the
database and carries game-related remarks only (the README's Genshin
example); provenance lives exclusively in the merge request's evidence text.
GOG rows are gated on the codename being a numeric gogdb.org product id
(Heroic launches satisfy this by construction — Heroic's GOG app name IS
that id; anything else is held back with the reason). EGS codenames stay the
launcher's App Name, which per the README is exactly the egdata.app Builds
"App Name" value; the checklist ticks that box only when the daemon itself
derived the store from HEROIC_APP_NAME. Evidence lines now link the
per-store authority (gogdb.org product page, egdata.app) and carry the
verification's advisory notes.

#### S9c — Manual matchup and the mislabel defense (DONE 2026-08-08)

The misses pane became a full matchup workbench. `p` picks from two
labeled, zero-network candidate sections: local umu-database matches
(Enter records the verdict) and identities from Heroic's store_cache
libraries (Enter sets store + codename overrides — identity, not verdict;
`v` verifies it afterwards). `o` asks the store's own database once per
keypress: GOG catalog search commits the numeric product id; egdata
commits the Builds "App Name" via the sandbox builds route (the lowercase
namespace is structurally never offered — a real hand-submission mixup);
a gog entry with a numeric codename flips to a by-id product lookup that
sets the *title* instead. A stale (>7 days) fetch cache warns inside pick
mode with `v` as the exit. All overrides (`store_override`,
`codename_override`, `title_override` via the `t` verb) live on the
annotation half and thread through verify, drafting, the gog gate, and
both exports with honest provenance.

The defense behind it, found live (Project Hospital stashed as
"Spellcraft" off `UnityCrashHandler64.exe`): shared helper executables
never name a game — the list ships as `shared-helpers.txt` (bundled →
installed → user config, union semantics, installed beside
endpoints.toml); and the stash now mirrors the group's ELECTED identity
instead of member claims in arrival order, inheriting the election's
monotonicity (regression test replays the incident). Dismissing hops the
selection to the neighbor so triage runs top-down. `ui` and `umu_misses`
split into per-concern modules. The export checklist carries only what a
human must verify — mechanical guarantees claim no box — and everything
that leaves the machine (CSV, merge-request text, stash notes,
shared-helpers.txt) is em-dash-free.

#### S10 — Release pipeline (DONE 2026-08-07)

Builds are network-free and reproducible: `build.rs` is gone (it existed only
to fetch `detectable.json`), and with it the `OUT_DIR` search tier in both
the daemon (`find_detectable_json`) and the setup tool (`DetectableTier::
BuildDir`). The naming database is fetched exclusively at runtime:
`gamebus-presence fetch-detectable` on demand, and the install plan appends
that same command — via the freshly **installed** CLI, never the build tree —
as its last, best-effort step. An offline install still succeeds; the daemon
degrades to executable names and the status screen offers the fetch as a
one-key fix. The plain-language plan summary names the download whenever any
plan contains it, and the scope note reserves the "rewrites your cache"
headline for the fetch-only plan (an install leads with its write scope).

`.github/workflows/release.yml` builds and publishes a release from every
`v*` tag: gate (fmt, clippy `-D warnings`, tests — integration tests skip
gracefully where the runner lacks gamemoded), release build, tarball bundle
(three binaries, `data/` units, `endpoints.toml`, README, plus LICENSE and
NOTICE where present), sha256 checksum, and a GitHub release created with the
runner's own `gh` — no third-party release actions. The tag must match
`Cargo.toml`'s version or the workflow refuses to bundle.

## Open decisions
- **MPRIS as a source** — the *naming* half landed as S6 (2026-08-06): player
  `Identity` as a low-precedence naming hint, never a source. The full source
  ("one record for everything the machine is doing", media records with
  playback state) stays deferred for the original reason: MPRIS already has
  its own consumers, wrapping it mostly duplicates.
- **arRPC-compatible bridge on 1337** — would let this replace arRPC outright
  for Vesktop users. Cheap once S2 exists; not currently in scope.
- **Licence not yet applied** — MIT OR Apache-2.0 proposed in the design doc.

## Not doing

Publishing presence *to* Discord; being a Discord client mod; any UI in the
daemon (`gamebus-setup` is tooling around it, not a product surface); Windows.
See the non-goals section of the design doc for why.
