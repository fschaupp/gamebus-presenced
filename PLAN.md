# Plan: gamebus-presenced

**Note**: the reasoning, the D-Bus surface and the source-by-source analysis
live in [`docs/design/gamebus-presence.md`](docs/design/gamebus-presence.md).
This file is the roadmap and the status line.

## Status: S0, S1, S2, S3, S4a, S4b, S4c, S4d, and S4e done. All slices complete.

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

## Open decisions
- **MPRIS as a source** — deliberately deferred. It is already a good standard
  with its own consumers, so wrapping it mostly duplicates. Move into S4 if
  "one record for everything the machine is doing" turns out to matter more
  than the duplication costs.
- **arRPC-compatible bridge on 1337** — would let this replace arRPC outright
  for Vesktop users. Cheap once S2 exists; not currently in scope.
- **Licence not yet applied** — MIT OR Apache-2.0 proposed in the design doc.

## Not doing

Publishing presence *to* Discord; being a Discord client mod; any UI; Windows.
See the non-goals section of the design doc for why.
