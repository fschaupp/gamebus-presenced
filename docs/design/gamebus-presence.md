# gamebus-presenced — one bus for "what is this machine playing"

Status: **working.** S0 (D-Bus surface), S1 (GameMode source), S2 (Discord IPC
listener), S3 (proxy + correlator + restart cache) and S4 (Steam enrichment,
naming, packaging) landed 2026-08-04 and are verified on the session bus; S5
(the `gamebus-setup` install and status tool) landed 2026-08-06 and is STAGED —
its user-level install is exercised, its system/pkexec path is not. See
[`PLAN.md`](../../PLAN.md) for what each slice covers and how it was verified.

## The problem

Every piece of the answer exists on a Linux desktop. No two pieces are in the
same place, and none of them know about each other.

| source | knows | does not know |
|---|---|---|
| Feral GameMode (`com.feralinteractive.GameMode`) | pid, executable path | the game's name, anything about its state |
| Steam (`~/.steam/registry.vdf`, `SteamAppId` in the env) | the appid | nothing past "an appid is running" |
| Discord Rich Presence (`$XDG_RUNTIME_DIR/discord-ipc-0`) | title, "Chapter 3", elapsed time, artwork | it is a write-only firehose into a proprietary client |
| Lutris / Heroic | which of *their* games they launched | they publish nothing; both just forward to Discord |
| MPRIS | media, thoroughly, as a real standard | games |

So today, answering "what is running and what is it doing" means running four
integrations and correlating them yourself. Every consumer — status bars,
desk pets, stream overlays, home automation, presence mirrors — reimplements
the same join. That is the mess this project deletes.

**One daemon collects every source, correlates them into a single activity
record, and publishes it on the session bus the way MPRIS publishes media.**

## Why nothing existing fits

- **arRPC** and its Rust ports ([rsRPC], [pog5/rsrpc], [arrpc-rs]) implement the
  Discord socket correctly, and stop there. Their fan-out is a WebSocket on
  1337 because their consumers are JS client mods that cannot reach D-Bus.
  Discord-only, and by design.
- **`org.freedesktop.portal.GameMode`** exists but has only
  `RegisterGame`/`UnregisterGame`/`QueryStatus`, an `Active` bool, and **no
  signals and no `ListGames`**. That is deliberate: a portal is a sandboxed app
  asking on its own behalf. Observing peers is exactly what the sandbox model
  refuses. The portal is the wrong half of the API and always will be.
- **No freedesktop spec** for game/app activity exists or is proposed. Checked
  the portal repo and the specs index; the gap is real and unoccupied.
- The dozen "discord bridge" projects ([rpc-bridge], [wine-discord-ipc-bridge],
  …) are all transport plumbing — Wine named pipe to unix socket — or run the
  *other* direction, D-Bus/MPRIS into Discord.

## Architecture

```
  ┌──────────────┐
  │ Discord IPC  │ transparent proxy on discord-ipc-0 ──▶ real Discord (ipc-1)
  │   listener   │ SET_ACTIVITY frames, SO_PEERCRED ──┐
  └──────────────┘                                    │
  ┌──────────────┐                                    ▼
  │  GameMode    │ GameRegistered/Unregistered ──▶ ┌──────────┐    ┌───────────┐
  │   watcher    │ (pid, executable)               │ correlator│──▶│  D-Bus    │
  └──────────────┘                                 │  (by pid) │   │  surface  │
  ┌──────────────┐                                 └──────────┘   └───────────┘
  │ Steam probe  │ registry.vdf RunningAppID,          ▲            org.gamebus
  │              │ /proc/<pid>/environ SteamAppId ─────┘             .Presence.v1
  └──────────────┘
```

Sources are independent and individually optional. A missing source degrades
the record, never the daemon.

### The correlator is the whole point

Anyone can relay Discord to D-Bus. The reason this is worth building is that
**the sources can be joined by pid**, and nothing does that today:

- The Discord IPC listener gets the connecting process's pid for free via
  `SO_PEERCRED` on the unix socket. No guessing, no process-name matching.
- GameMode's `GameRegistered`/`GameUnregistered` signals and `ListGames` hand
  over `(pid, per-game object path)`; the executable and registration
  timestamp live on that object's `Executable`/`Timestamp` properties
  (`com.feralinteractive.GameMode.Game`), with `/proc/<pid>/exe` as fallback.
- With a pid, `/proc/<pid>/environ` yields `SteamAppId`, and `/proc/<pid>/exe`
  the real binary behind whatever wrapper script launched it.

So one activity record can carry *Discord's* title and chapter text, *GameMode's*
executable, and *Steam's* appid, correlated rather than guessed. Sources that
cannot be tied to a pid (Steam's `registry.vdf` alone) publish a lower-confidence
record and merge later if a pid shows up.

### The Discord half, concretely

Bind `discord-ipc-0` at session start. Discord starts later, finds 0 taken and
binds ipc-1 — its own docs describe the client walking the range "trying
sequentially until it can bind to one", which is also why Stable and Canary
coexist. Games connect to the first socket that accepts, which is us.

- **With Discord present**: forward every frame verbatim upstream and pipe
  responses back. The game sees normal Discord; Discord sees a normal client;
  we see everything. Frames are forwarded whole — never split a write, the pipe
  breaks on partial writes.
- **Without Discord**: answer the handshake and echo commands ourselves, which
  is the arRPC behaviour and what [rsRPC] already implements as a library.
- Unlink stale sockets on start. Losing the race to Discord is a warning and a
  fall back to observing nothing, never a crash.

### Surviving a restart

If the daemon restarts, games already connected get an EOF. Whether they come
back is **not ours to decide**: `discord-rpc`'s `rpc_connection.cpp` has no
internal retry timer, and `Open()` is re-attempted only when the embedder's own
update loop calls it again. Many games do that every frame and recover
silently; others never try. Per-integration behaviour is the worst kind to
depend on, so the design does not.

What saves it instead:

1. **The other sources re-derive from scratch.** GameMode and Steam repopulate
   on startup, so an activity never disappears — at worst the Discord-sourced
   fields (title, chapter text, artwork) go quiet.
2. **A runtime cache re-adopts the rest.** Records are persisted to
   `$XDG_RUNTIME_DIR`, keyed by **pid plus the process start-time** from
   `/proc/<pid>/stat` (field 22). On startup, every record whose process is
   still alive *and* whose start-time still matches is re-adopted; the
   start-time is what stops pid reuse from handing us a different process
   wearing a dead game's presence.

That makes a restart usually seamless without leaning on anyone else's update
loop. It is still best-effort by construction, which is the right bar: a
session daemon of this kind restarts for developer iteration and little else.

### Naming

Discord's [`applications/detectable`][detectable] endpoint (~1834 entries)
maps process names to game names and icons — the same list arRPC caches. That
is the difference between "playing Elden Ring" and "playing `eldenring.exe`",
and it enriches *all* sources, not just Discord: a GameMode-only registration
can still be named from its executable.

Cached on disk, refreshed rarely, and **entirely optional** — no network, no
naming, everything else still works.

## The D-Bus surface

Modelled on MPRIS and on systemd's interface conventions, since those are what
consumers already know how to read.

### On versioning

**The version is a separate element — `…Presence.v1`, not `…Presence1`.** This
is a deliberate departure from the neighbours (`org.freedesktop.login1`,
`org.mpris.MediaPlayer2`, and systemd's whole family), so it should not be
"corrected" back later.

The neighbours' style is habit, but the *versioning* underneath it is not.
[Poettering's rationale][versioning] is that a version belongs in the name
because "you can acquire more than one well-known service name on the bus, so
even if you rework everything you can keep compatibility". That is a real
structural constraint and not an aesthetic one: unlike a REST `/v1`, where both
versions are sub-resources of one endpoint, **a D-Bus well-known name *is* the
endpoint and has exactly one owner**. Version outside the name and two
generations can never run at once.

`org.gamebus.Presence.v1` satisfies that identically to `Presence1` — one
daemon can own `.v1` and `.v2` at the same time and serve both — while reading
unambiguously. Both are valid D-Bus: name elements may not *begin* with a
digit, and `v1` begins with a letter. The choice cascades consistently into
object paths (`/org/gamebus/Presence/v1`) and interface names
(`org.gamebus.Presence.v1.Manager`).

The same rationale says to bump **only for incompatible changes** — added
methods and properties do not count, provided clients handle `UnknownMethod`.
So the name suffix is the coarse lever, and the `Version` property on the
Manager (below) is the fine one, exactly as every xdg-desktop-portal interface
does it. Additive change bumps `Version`; a signature break earns a `.v2` name
that runs beside `.v1`.

```
bus name     org.gamebus.Presence.v1
root         /org/gamebus/Presence/v1                iface org.gamebus.Presence.v1.Manager
  ListActivities() → ao
  ActivityAdded(o) / ActivityRemoved(o)
  HasActivity (b, emits-change)   ← the cheap "is anything running" for panels
  Version (u)                     ← additive revisions; a break earns a .v2 name

activity     /org/gamebus/Presence/v1/Activity/<id>  iface org.gamebus.Presence.v1.Activity
```

Activity properties, all read-only, all `emits-change`:

| property | type | notes |
|---|---|---|
| `Sources` | `as` | `["discord","gamemode","steam"]` — which sources back this record |
| `Kind` | `s` | `game` \| `app` \| `unknown` |
| `Name` | `s` | resolved human name; falls back to the executable stem |
| `Details` / `State` | `s` | Discord's two free-text lines; empty when unknown |
| `ProcessId` | `u` | 0 when not correlated to a process |
| `Executable` | `s` | from GameMode or `/proc/<pid>/exe` |
| `AppIds` | `a{ss}` | `{"discord": "…", "steam": "…"}` — source-scoped ids |
| `Since` / `Until` | `t` | unix seconds, 0 = unset |
| `LargeImage`/`LargeText`/`SmallImage`/`SmallText` | `s` | artwork, resolved to URLs where possible |
| `PartySize` / `PartyMax` | `u` | 0 = unset |
| `Extra` | `a{sv}` | escape hatch for source fields we have not modelled |

Design rules worth stating so they do not erode:

1. **Empty string means "not known", never a placeholder.** Consumers render
   what they get.
2. **A property never regresses to unknown while a source still asserts it.**
   Losing Discord does not blank the name if GameMode still holds the pid.
3. **The record dies with the last source.** No lingering "last played".
4. **Read-only.** Writing presence *to* Discord is a different program's job
   and is an explicit non-goal (below).

## Non-goals

- Publishing presence back to Discord. That direction is well served and is the
  opposite of what this daemon is for.
- Being a Discord client mod, or shipping UI of any kind *in the daemon*. The
  `gamebus-setup` install-and-status tool is tooling around it, not a product
  surface: it is not a resident process, publishes nothing on the bus, and is
  only ever a client of it. Consumers still see nothing but the interface.
- Windows. The design is unix-socket and D-Bus shaped throughout.
- Guessing. A source that cannot be correlated says so via `Sources` and a
  missing `ProcessId`; it does not fabricate a join.

Possible later, explicitly not now: an arRPC-compatible bridge on 1337 so this
can replace arRPC outright for Vesktop users; an MPRIS source so "watching" and
"listening" join the same model.

## Slices

| | contents | usable after? |
|---|---|---|
| **S0** | D-Bus surface foundation, extracted from the original S1: `zbus`, `ListActivities` + signals skeleton. **Done 2026-08-04.** | Yes — interface verified on the bus. |
| **S1** | GameMode source feeding the surface: `GameRegistered`/`GameUnregistered`, `ListGames` seed, per-activity objects. **Done 2026-08-04.** | Yes — pid/executable presence, zero Discord involvement. This alone is what the epaper pet needs. |
| **S2** | Discord IPC listener with **no** upstream (the Discord-not-running case): handshake, frame codec, `SET_ACTIVITY` → activity objects, `SO_PEERCRED` pid. Payload model from the pinned [rsRPC] crate. **Done 2026-08-04.** | Yes — full rich presence on a machine without a Discord client. |
| **S3** | Transparent proxy to a running Discord, plus the correlator: pid join across sources, merge and split rules, and the pid+start-time runtime cache that survives a restart. **Done 2026-08-04.** | Yes — works alongside a real Discord client. |
| **S4** | Steam appid enrichment, `detectable.json` naming, a `gamebus-presence monitor` CLI, systemd user unit + D-Bus activation file. Extended in S4d/S4e with the umu/Proton wrapper-tree join and game identification. **Done 2026-08-04.** | Polish. |
| **S5** | `gamebus-setup`: what is working, what is not, and the remedy for each — plus the install itself, for a user-level (`~/.local`) or system (`/usr/local`) target. **STAGED 2026-08-06.** | Yes for a user-level install; the system target is unexercised. |

S1 and S2 are independent; either can land first.

## Verification

The awkward part of a presence daemon is testing it without launching a game.
Both halves have a real answer:

- **Discord side**: the [`discord-rich-presence`] crate as a dev-dependency is a
  genuine RPC *client*. Point it at our socket in an integration test and assert
  the resulting D-Bus objects. That tests the real wire format, not a fixture.
- **GameMode side**: `gamemoderun sleep 30` registers a real game with the real
  daemon. `busctl --user call … RegisterGameByPID` for the unhappy paths.
- **Consumer side**: `busctl --user monitor org.gamebus.Presence.v1` is the
  acceptance test a user can run themselves.
- **Proxy fidelity**: run with and without Discord present and assert the
  forwarded byte stream is identical to what the client sent.

Frame codec, correlator and merge rules are pure functions over bytes and
records — unit-testable without a socket or a bus, and that is where the bulk
of the tests belong.

## Licensing and dependencies

MIT/Apache-2.0 dual, the Rust norm, and compatible with anything that wants to
embed the correlator as a library. Note [pog5/rsrpc] is **GPLv3** — fine to
learn from, not to vendor. [rsRPC] is MIT and is the reasonable thing to build
the Discord listener on, or to lift the frame handling from.

Dependencies stay small: `zbus`, `tokio`, `serde`/`serde_json`. Nothing that
drags a browser engine or an HTTP stack into a session daemon. The setup tool's
`ratatui` sits behind a default-on `setup` feature and is linked into no binary
but its own; `cargo build --no-default-features` is the daemon-only build.

## Relationship to epaper_rs

`epaper-hubd` is consumer number one, not a component. It gains a source that
reads `org.gamebus.Presence.v1` over the bus it already speaks. Nothing about
this project is desk-pet-shaped, and it must stay that way for anyone else to
adopt it.

---

[rsRPC]: https://github.com/SpikeHD/rsRPC
[pog5/rsrpc]: https://github.com/pog5/rsrpc
[arrpc-rs]: https://github.com/BlankParticle/arrpc-rs
[rpc-bridge]: https://github.com/enderice2/rpc-bridge
[wine-discord-ipc-bridge]: https://github.com/0e4ef622/wine-discord-ipc-bridge
[detectable]: https://github.com/discord/discord-api-docs/issues/1862
[`discord-rich-presence`]: https://lib.rs/crates/discord-rich-presence
[versioning]: https://0pointer.de/blog/projects/versioning-dbus.html
