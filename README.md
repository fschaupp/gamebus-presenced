# gamebus-presenced

One session-bus service that answers **"what is this machine playing, and what
is it doing in there?"** — by collecting the sources that each hold a fragment
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

This joins them, and publishes the result the way MPRIS publishes media —
read-only properties with change signals, no client library required:

```sh
busctl --user monitor org.gamebus.Presence.v1
```

## Status

**Working.** GameMode, Discord and Steam sources, pid correlation, a restart
cache, game-name enrichment, and a setup tool. See
[`docs/design/gamebus-presence.md`](docs/design/gamebus-presence.md) for the
architecture, the D-Bus surface, and the reasoning about why the existing
options (arRPC, the GameMode portal) do not cover this, and [`PLAN.md`](PLAN.md)
for what landed when.

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
real hardware** — the user-level install is the exercised one.

It also works without the terminal interface:

```sh
gamebus-setup status                        # what is working, as plain text
gamebus-setup plan install --target user    # exactly what an install would do
gamebus-setup apply install --target user --confirm
```

Note that "system-wide" means the system *prefix*. The unit stays a systemd
**user** unit — one daemon per login session, on the session bus.

### Doing it by hand

Nothing about the daemon requires the setup tool:

```sh
install -Dm755 target/release/gamebus-presenced ~/.local/bin/gamebus-presenced
install -Dm755 target/release/gamebus-presence  ~/.local/bin/gamebus-presence
install -Dm755 target/release/gamebus-setup     ~/.local/bin/gamebus-setup

# The files in data/ name /usr/bin; a ~/.local install must rewrite the path.
# The D-Bus activation file accepts no %h and no $HOME, which is why it has to
# be substituted rather than copied — and why the setup tool generates both.
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
file, no naming database — so the daemon will not autostart and games will be
named after their executables. (`cargo install` from crates.io is not possible
at all: the Discord payload model comes from a git dependency.)

## Prior art it stands on

- [rsRPC](https://github.com/SpikeHD/rsRPC) — MIT Rust implementation of the
  Discord IPC socket, the reasonable base for that source.
- [arRPC](https://github.com/OpenAsar/arrpc) — the original research into
  Discord's half-documented local RPC server.
- [GameMode](https://github.com/FeralInteractive/gamemode) — whose
  `GameRegistered` signal is the cleanest source of the four.
- [MPRIS](https://specifications.freedesktop.org/mpris/latest/) — the model for
  what a good desktop presence interface looks like.

## Licence

MIT OR Apache-2.0 (proposed).
