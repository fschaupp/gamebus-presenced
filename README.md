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

**Working.** GameMode, Discord and Steam sources, pid correlation, a restart
cache, game-name enrichment, and a setup tool. The existing
options (arRPC, the GameMode portal) each cover a single source; none of
them join the sources into one record.

Because the sources already name the game, the same tooling houses **compat
findings**. When a launch is stopped by something Wine cannot do (a kernel
anti-cheat driver, an unimplemented export, a volume-GUID path bug), whatever
detected it drops the finding into an inbox, and `gamebus-setup compat` keeps
it and drafts the ProtonDB and AreWeAntiCheatYet submissions from it. Nothing
is ever submitted on your behalf: it prints a draft and lists what that draft
still needs from a human, because the fields those sites require (a public
source, the anti-cheat vendor's product name) are exactly the ones no trace
can supply.

## Install

```sh
cargo build --release
./target/release/gamebus-setup
```

`gamebus-setup` is a small terminal tool: it reports what is and is not working,
installs the daemon, and fixes the usual problems (autostart disabled, naming
database missing, a stale D-Bus activation file) from the same screen. It offers
a user-level install into `~/.local` and a system one into `/usr/local`;
`/usr` proper is left to whatever packages your distribution installs.

The system target authenticates via `pkexec`. That path is **not yet tested on
real hardware**; the user-level install is the exercised one.

It also works without the terminal interface:

```sh
gamebus-setup status                        # what is working, as plain text
gamebus-setup plan install --target user    # exactly what an install would do
gamebus-setup apply install --target user --confirm
```

Note that "system-wide" means the system *prefix*. The unit stays a systemd
**user** unit: one daemon per login session, on the session bus.

### Doing it by hand

Nothing about the daemon requires the setup tool:

```sh
install -Dm755 target/release/gamebus-presenced ~/.local/bin/gamebus-presenced
install -Dm755 target/release/gamebus-presence  ~/.local/bin/gamebus-presence
install -Dm755 target/release/gamebus-setup     ~/.local/bin/gamebus-setup

# The files in data/ name /usr/bin; a ~/.local install must rewrite the path.
# The D-Bus activation file accepts no %h and no $HOME, which is why it has to
# be substituted rather than copied, and why the setup tool generates both.
sed "s|/usr/bin/gamebus-presenced|$HOME/.local/bin/gamebus-presenced|" \
    data/gamebus-presenced.service \
    > ~/.config/systemd/user/gamebus-presenced.service
mkdir -p ~/.local/share/dbus-1/services
sed "s|/usr/bin/gamebus-presenced|$HOME/.local/bin/gamebus-presenced|" \
    data/org.gamebus.Presence.v1.service \
    > ~/.local/share/dbus-1/services/org.gamebus.Presence.v1.service

gamebus-presence fetch-detectable      # game-name database, ~12 MB
systemctl --user daemon-reload
# Without this the activation file takes effect only after the next login.
busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
    org.freedesktop.DBus ReloadConfig
systemctl --user enable --now gamebus-presenced.service
```

`cargo install --path .` installs the binaries only: no unit, no activation
file, no naming database, so the daemon will not autostart and games will be
named after their executables. (`cargo install` from crates.io is not possible
at all: the Discord payload model comes from a git dependency.)

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
