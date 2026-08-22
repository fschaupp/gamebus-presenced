# gamebus-gamedb

This is not a game database in the catalog sense. It collects the handful
of facts a game launcher cannot work out on its own and that no
upstream project will accept: which store codename belongs to which game,
which executables ship beside many games and therefore never name one, and
which rows in closed datasets are wrong. Anything a launcher can already
derive, or that belongs in umu-database, the Lutris API, or Discord's
detectable list, does not belong here.

Why it exists: umu-database only takes games that require a fix in Proton,
Discord's detectable list has no contribution path at all, and the Lutris API
is read-only with no field for a store codename. The knowledge in between has
nowhere else to live.

## Layout

    games/<slug>.toml   one page per GAME, stores nested inside
    helpers.toml        executables that can never identify a game
    schema/             JSON Schema for both, applied to the parsed TOML

## Ids

Every page carries a canonical id in its `gamedb` field, written above the
note. It is **derived where an authority already names the game**, and minted
only where none does:

    steam-870780      a Steam app id, when the game is on Steam
    umu-870780        the umu id, when the game really is in umu-database
    gog-1660194629    a store codename, taking stores in the order gog, egs,
    egs-Calluna       ubisoft, ea, battlenet, amazon, humble, itchio,
                      zoomplatform
    gamedb-k7m2p9qz   minted here, when nothing above names the game

In that order of precedence. The field may be **omitted whenever it is
derivable** - the lint works the same id out from `[ids]` and the store entries
either way, and rejects one that none of the page's own identifiers justifies,
so it is stored for visibility and checked for truth rather than trusted. An id
the page justifies but that precedence would no longer pick is kept and
reported as a warning, because ids are frozen on assignment (below).

**The prefix names the namespace the identifier belongs to.** A consumer reads
the prefix and never the shape of what follows: `gog-1660194629` is a GOG
product id, not a number to guess about. That is the whole safety story, and it
is why a numeric store id needs no special handling.

A `gamedb-` id is minted only for a page with no store identity at all - a
correction to somebody else's wrong row, identified by executable. It is eight
characters of `[a-z0-9]` by convention and must contain at least one letter,
which keeps it visibly distinct from an authority's number; the letter is the
part the lint enforces. It is checked unique across the whole data set before a
merge. There is no registry to consult and nothing to allocate centrally: the
check is the same pass that validates everything else.

**One game, one page - and the build enforces it.** Every identifier a page
carries is a lookup key that resolves to that page: its canonical id, each
store's `<store>-<codename>`, the Steam app id, the umu id, every executable it
names, and anything it absorbed through `merged_from`. No two pages may claim
the same one.

This is what catches the common contribution mistake. Somebody adds a game's
GOG copy as a new page, not knowing the Epic copy is already here; the two
pages both claim `egs-Calluna`, or both claim `steam-870780`, or both name the
same executable, and the merge request fails:

    'egs-Calluna' is claimed by both control-gog.toml and control.toml -
    the same game cannot live on two pages, so consolidate them into one

The build breaking is the point. Consolidating is a human decision and a small
one - move the new store entry onto the existing page - but it has to happen
before the data set carries the same game twice, because from then on a lookup
has two answers and neither is wrong.

**Merging two pages.** Sooner or later two pages turn out to be one game - a
page identified only by an executable meets the page that had its store entry
all along. The survivor lists what it absorbed:

```toml
gamedb = "gamedb-k7m2p9qz"
merged_from = ["egs-LateBloomerApp"]
```

Absorbed ids stay reserved forever: never reused for another game, and carried
into the built artifacts as aliases, so a lookup that learned the old id still
resolves. The merge itself is a manual judgement - deciding two things are the
same game is exactly what a reviewer is for - and the only mechanical part is
that the ids do not get lost.

**An id is never re-pointed, and never recomputed.** Precedence decides an id
once, when the page is created; after that it is frozen. A page that later
learns a better identifier records it in `[ids]` and keeps the id it already
had, so nothing that referred to it breaks. The file name is free to change
too, because the id lives in the file and not in the path.

A gamedb id is **not** a umu id. Do not put one in `GAMEID` or `UMU_ID`; cross
over through `[ids]` instead, which is what it is for:

```toml
title = "Control"
gamedb = "steam-870780"

[ids]
steam = 870780
source = "steam-sku"
seen = "2026-08-22"

[[stores.egs]]
codename = "Calluna"
seen = "2026-08-15"
source = "heroic-config"
confidence = "high"
```

`ids.umu` appears **only when the game really is in umu-database**. A game umu
declined has no umu id, and writing one would claim a membership it does not
have. An id belongs in `[ids]` only when no store entry already carries it - a
GOG product id *is* the GOG codename, and a fact is recorded once.

## File names

The file name is a human label, not the key: lowercase, the title's slug, with
a qualifier when two games share a title (`prey.toml` and `prey-2017.toml`).
Games are not shelved by year or by anything else - a contributor asking "does
a page for this game exist already?" knows the name and not much else, and a
flat directory answers that at a glance.

## Pages

A page describes one game across every store it appears on, because the claim
worth reviewing is that these store identities are the same game.

Store keys use umu's own spelling: steam, gog, egs, ubisoft, zoomplatform,
humble, itchio, amazon, battlenet, ea, umu. Store entries are always arrays,
even when a game has exactly one entry for a store, so consumers never need a
special case for the single-entry shape.

Every claim carries how it was learned (`source`), when (`seen`), and how sure
we are (`confidence`). A reviewer should be able to judge a page without
trusting whoever wrote it.

## Same name, editions, DLC

**Two different games with the same title.** The page that exists keeps its
file name and its id; the newcomer takes a distinguishing file name of its own.
Their ids differ anyway, because they come from different app ids.

**Editions of one game are store entries, not pages.** Standard and Deluxe on
the same store are two products but one game, and presence should say the
game's name. They sit on one page as two entries with an `edition` label:

```toml
[[stores.egs]]
codename = "Calluna"
edition = "Standard"

[[stores.egs]]
codename = "SomeOtherAppName"
edition = "Deluxe"
```

**A remaster or definitive edition sold as its own product** does get its own
page, because it is separately installed, separately identified, and usually
separately named on screen. It points home with `variant_of`, and the lint
checks the target exists:

```toml
title = "Control Ultimate Edition"
variant_of = "gamedb-control"
```

The dividing line is whether a launcher ever has to name the thing. If it can
appear on screen as what is running, it earns a page; if it is only another way
to buy the same executable, it is an edition.

**DLC gets nothing.** It never launches as its own process, so no resolver ever
has to name it. A store id that belongs to DLC belongs to the base game's page
or nowhere.

## License

The data here - the game pages and `helpers.toml` - is under the **Open
Database License v1.0** (`LICENSE`, retrieved from
https://opendatacommons.org/licenses/odbl/1-0/ on 2026-08-22; check it against
the canonical text before relying on it). The lint, the schemas and the build
scripts are code and stay under their own project's license.

What that means in practice, in the license's own words:

> If you publicly use any adapted version of this database, or works produced
> from an adapted database, you must also offer that adapted database under the
> ODbL.

Use it, ship it inside whatever you like, extend it. Nothing here asks you to
send changes back to this repository, to ask permission, or to tell anyone you
used it. The one thing the license does is keep an enriched copy from
disappearing into a product: if you publish something built on a version you
extended, that extended version is public too. Your own code is unaffected -
share-alike attaches to the database, not to the application around it.

The attribution the ODbL asks for is a notice, not a credit screen.

None of this is a claim to own facts. A Steam app id belongs to Steam and a
store codename to its store; what is licensed here is the collection - the
selection, the verification, and the notes explaining why an entry says what it
says.

## Contributing

Open a pull request that adds or edits one page. There is no API and no
account to register for. Every pull request runs the lint below, so a mistake
is caught before a human reads it.

## Checking the data

    .scripts/gamedb-lint.py [directory]

validates the whole set: both schemas, the id rules, and the one-game-one-page
rule. It exits non-zero when anything fails, which is what a merge request is
gated on. `.scripts/gamedb-lint-selftest.sh` checks the lint itself against a
fixture set carrying one page per rule.

Both live outside this directory on purpose. When this becomes its own
repository the checks are downloaded as a pinned release rather than vendored,
so the data set stays data only. The Python is a stand-in until the Rust lint
exists.

## Built artifacts

The condensed forms are built from these files and published as release
assets. They are never committed here, because a binary blob per merge would
bloat the repository permanently.

    identities.json       scripts and web pages
    identities.parquet    bulk and analysis, queryable over HTTP with DuckDB
    identities.sqlite     in-process point lookup, indexed
    shared-helpers.txt    the flat list, one basename per line
    manifest.toml         source commit and a checksum per artifact

Builds are deterministic: sorted, fixed field order, no build timestamp inside
the data.
