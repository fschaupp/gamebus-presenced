# gamebus-presenced

One session-bus service that answers **"what is this machine playing, and what
is it doing in there?"** by collecting the sources that each hold a fragment
of the answer and correlating them into a single record.

```
GameMode  ─┐  knows the pid and the executable
Discord   ─┼─▶  gamebus-presenced  ─▶  org.gamebus.Presence.v1  ─▶  anyone
Steam     ─┘  knows the appid                                     on the bus
```

Today a status bar, a stream overlay or a desk pet needs four integrations to
learn what a single game launch already told the system four times over.
GameMode has the pid but not the name. Steam has the appid but not the state.
Discord has the title and the chapter text, but only inside a proprietary
write-only socket. Nothing joins them.

This joins them, and publishes the result the way MPRIS publishes media:
read-only properties with change signals, no client library required:

```sh
busctl --user monitor org.gamebus.Presence.v1
```

## Status

**Design draft.** No implementation yet - see
[`docs/design/gamebus-presence.md`](docs/design/gamebus-presence.md) for the
architecture, the D-Bus surface, and the reasoning about why the existing
options (arRPC, the GameMode portal) do not cover this.

*Built with AI assistance: Claude (Anthropic) pairs on this codebase; a human reviews, tests, and ships every change.*

## Prior art it stands on

- [rsRPC](https://github.com/SpikeHD/rsRPC): MIT Rust implementation of the
  Discord IPC socket, the reasonable base for that source.
- [arRPC](https://github.com/OpenAsar/arrpc): the original research into
  Discord's half-documented local RPC server.
- [GameMode](https://github.com/FeralInteractive/gamemode): whose
  `GameRegistered` signal is the cleanest source of the four.
- [umu](https://github.com/Open-Wine-Components/umu-launcher): the unified
  Proton launcher whose per-game ids name non-Steam titles, kept in the
  shared [umu-database](https://github.com/Open-Wine-Components/umu-database).
- [MPRIS](https://specifications.freedesktop.org/mpris/latest/): the model for
  what a good desktop presence interface looks like.

## Licence

Apache-2.0. Copyright 2026 Florian Schaupp.

## Disclaimer

Note: This tool is not affiliated with Discord Inc. It's an independent
project that uses Discord's public API for utility purposes.
