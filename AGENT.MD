# AGENT.MD

## Purpose

Rules and context for AI agents working in this repository. Read this, then
[`docs/design/gamebus-presence.md`](docs/design/gamebus-presence.md) — that
design doc is the single source of truth for what this project is and why.

## What this is

`gamebus-presenced` publishes **one** answer to "what is this machine playing,
and what is it doing in there?" on the session bus, by collecting sources that
each hold a fragment (Feral GameMode has the pid, Steam has the appid, Discord
Rich Presence has the title and chapter text) and correlating them by pid.

- Bus name: `org.gamebus.Presence.v1` (see the versioning section of the design
  doc before changing this — the `.v1` spelling is deliberate)
- Consumer number one is `epaper-hubd` in `/media/Data/Projekte/ardu_rs/epaper_rs`,
  but **this project is not desk-pet-shaped and must stay that way** for anyone
  else to adopt it.

## Status

**Working.** S0-S4e are verified against a real bus; S5 is STAGED (see
`PLAN.md` for exactly what is and is not proven). Between them they cover: D-Bus surface, GameMode/Discord/Steam sources,
correlator, restart cache, naming enrichment, and the `gamebus-setup` install
and status tool. `PLAN.md` holds the slice roadmap and what is verified against
a real bus.

## Working rules

These carry over from the sibling `epaper_rs` project, where they were promoted
after recurring three or more times. They apply here unchanged.

1. **Plan before implementing.** A slice gets a spec and a sign-off before code.
   Never pick an undecided design point unilaterally — surface it.
2. **Verify against the real system; mark unverified work STAGED.** For this
   project the "real system" is a real session bus, a real `gamemoded`, and a
   real RPC client on the socket. A green `cargo test` proves the pure
   functions and nothing about the daemon. Say STAGED when that is what it is.
3. **Verify live state before acting.** Read the actual interface with
   `busctl --user introspect`, not from memory. D-Bus APIs change and this repo
   integrates four of them.
4. **Check the instrument before believing the reading.** An empty
   `ListActivities`, a missing signal, a socket that accepted — all are claims.
   Prefer positive proof: the object appeared on the bus, the frame round-tripped.
5. **Evidence-first debugging.** Make the failing path show what it saw. For
   wire protocols that means dumping the actual bytes before theorising.
6. **Doc-sync in the same sitting.** Update `PLAN.md` and the design doc when
   the code diverges from them, not later.

## Conventions

- **Git identity** is set repo-locally: `fschaupp <spritzwine.absently488@passinbox.com>`.
- **Commit messages**: subject-only, present tense, describing intent rather
  than mechanics. No trailers.
- **Branches**: feature branches, never commit directly to `main`.
- **Licence**: MIT OR Apache-2.0 (proposed, not yet applied).
- **Dependencies stay small.** `zbus`, `tokio`, `serde`. Nothing that drags an
  HTTP stack or a browser engine into a session daemon. This constrains the
  *daemon*: `gamebus-setup` adds `ratatui` behind a default-on `setup` feature
  and is never linked into `gamebus-presenced`; `--no-default-features` builds
  the daemon without it. Note that
  [pog5/rsrpc](https://github.com/pog5/rsrpc) is **GPLv3** — learn from it, do
  not vendor it. [rsRPC](https://github.com/SpikeHD/rsRPC) is MIT and is the
  reasonable base for the Discord listener.

## Verification

Once code exists, the gates are:

```bash
cargo test                  # pure functions, plus integration tests that run
                            # the daemon on a private session bus and a private
                            # XDG_RUNTIME_DIR (tests/common/mod.rs) — safe to
                            # run with a real gamebus-presenced installed and
                            # a real Discord client connected
cargo clippy -- -D warnings
busctl --user introspect org.gamebus.Presence.v1 /org/gamebus/Presence/v1   # live surface
gamemoderun sleep 30        # registers a real game with the real gamemoded
```

The `discord-rich-presence` crate as a dev-dependency is a genuine RPC *client* —
point it at our socket in an integration test and assert the resulting D-Bus
objects. That exercises the real wire format rather than a fixture.

## Knowledge base

Long-form session knowledge lives in the local second brain vault at
`.second-brain/` — a dedicated vault for this project following the
obsidian-second-brain pattern. See `_CLAUDE.md` for operating instructions.

The vault uses **wiki-style** folder structure:
- `wiki/projects/` - Project notes (e.g., `gamebus-presenced.md`)
- `wiki/entities/` - People, companies, tools
- `wiki/daily/` - Daily notes
- `wiki/tasks/` - Standalone tasks
- `wiki/logs/` - Dev session logs
- `wiki/concepts/` - Ideas, frameworks, synthesis
- `wiki/reviews/` - Weekly/monthly reviews
- `wiki/decisions/` - ADRs
- `boards/` - Kanban boards
- `raw/` - Immutable source captures
- `Research/` - Research outputs

**Standing rule: no agents write the vault.** Vault writes happen in-session, by
the main assistant, never fanned out to subagents. All notes must follow the
AI-first rules defined in `.second-brain/_CLAUDE.md` Section 0.
