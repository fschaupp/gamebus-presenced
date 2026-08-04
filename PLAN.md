# Plan: gamebus-presenced

**Note**: the reasoning, the D-Bus surface and the source-by-source analysis
live in [`docs/design/gamebus-presence.md`](docs/design/gamebus-presence.md).
This file is the roadmap and the status line.

## Status: S0, S1, and S2 done. S3 next.

Repo created 2026-08-03. The original S1 ("D-Bus surface + GameMode source")
was split: the surface was extracted as S0 so the interface could be verified
on the bus before any source existed. S0 and S1 both landed 2026-08-04,
verified against the real session bus and a real gamemoded. S2 landed the
same day, verified with a genuine `discord-rich-presence` client against the
daemon's own `discord-ipc-0`.

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

### S3 — Proxy + correlator (PLANNED)

Forward frames verbatim to a real Discord (which will have taken `ipc-1`, since
the client walks the range for the first free socket) so this works alongside a
running Discord. Plus the correlator: pid join across sources, merge and split
rules, and the pid + `/proc/<pid>/stat` start-time runtime cache that re-adopts
records across a daemon restart.

### S4 — Enrichment and packaging (PLANNED)

Steam appid from `/proc/<pid>/environ`, `detectable.json` naming (Discord's
`applications/detectable`, ~1834 entries, cached on disk and entirely
optional), a `gamebus-presence monitor` CLI, systemd user unit and D-Bus
activation file.

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
