# Contributing a page

This is the practical path for adding a game. The rules themselves, and the
reasoning behind them, are in `README.md`; this file is how you carry them out.

A contribution is a pull request that adds or edits one file under `games/`,
or one entry in `helpers.toml`. There is no API, no account, and nothing to
register.

## 0. Is it in scope?

The data set collects the few facts a launcher cannot work out on its own and
that no upstream project accepts. Before anything else, check that your fact is
one of them.

In scope:

- which store codename belongs to which game (an Epic app name, a GOG product
  id, the string a launcher actually has in hand)
- which executables ship beside many games and therefore never name one
  (`helpers.toml`)
- which rows in a closed data set are wrong, where the correction can be stated
  as a game identity

Out of scope:

- **Anything a launcher can already derive.** If the installed copy carries the
  answer, or the store's own metadata gives it up without a human, it does not
  need a page here.
- **Anything belonging in umu-database.** If the game needs a fix in Proton to
  run, that is umu's data set, and it takes contributions. Send it there.
- **DLC.** It never launches as its own process, so no resolver ever has to
  name it. A store id that belongs to DLC belongs on the base game's page or
  nowhere.
- **Catalog metadata.** Genre, cover art, release notes, price, ratings,
  descriptions. This is not a catalog, and a page carrying that would have to
  be maintained against sources we do not own.

If you are unsure, open the pull request and say what you are unsure about. The
scope line is the one thing a reviewer will always have an opinion on.

## 1. Check whether the game is already here

`games/` is one flat directory, on purpose: the question a contributor actually
has is "does a page for this game exist?", and a flat list answers it at a
glance.

    ls games/

The file name is the lowercase slug of the title, so the obvious guess is
usually right:

    ls games/ | grep -i control

But the file name is only a label. **The real answer is that any identifier the
page carries resolves to it**: the canonical id, each `<store>-<codename>`, the
Steam app id, the umu id, every executable it names, and anything it absorbed
through `merged_from`. So search by whatever you have in hand, not by the name:

    grep -ril 870780 games/          # a Steam app id
    grep -ril calluna games/         # a store codename
    grep -ril 1660194629 games/      # a GOG product id
    grep -ril projecthospital.exe games/

**If a page comes back, that page is the game.** Add your store entry to it.
Do not add a second page, even when the copy you have is from a different store
and looks like a different product. The build rejects the second page, and the
fix is always the same edit you would have made in the first place.

## 2. Write the page

One page describes one game across every store it appears on, because the claim
worth reviewing is that these store identities are the same game. Copy an
existing page and change it; `games/control.toml` and
`games/project-hospital.toml` are the two that exist.

```toml
title = "Control"
gamedb = "steam-870780"
note = """
Why this page exists, in a sentence or two. Optional, but it is where a \
reviewer looks first."""

[ids]
steam = 870780
source = "steam-sku"
seen = "2026-08-22"

[[stores.egs]]
codename = "Calluna"
seen = "2026-08-15"
source = "heroic-config"
confidence = "high"
note = "Anything a reviewer would otherwise have to ask about."
```

Unknown fields are an error, so the list below is the whole vocabulary.

### Top level

| Field | Required | What it is |
|---|---|---|
| `title` | yes | The game's own capitalization, as the store or the game itself writes it. The file name is derived from this, so get it right first. |
| `gamedb` | no | This page's canonical id. See section 3. Omit it whenever it is derivable. |
| `year` | no | Release year. Useful when two different games share a title, never required. |
| `note` | no | Free prose. Why this page exists, what was checked, what is still unknown. |
| `ids` | no | Identifiers in other people's namespaces, recorded as facts. |
| `exe` | no | Executable basenames that identify this game. Only for a page with no store identity at all. |
| `stores` | no | Store entries, keyed by store. |
| `variant_of` | no | The base game's canonical id, when this page is a separately sold product (a remaster, a definitive edition). |
| `merged_from` | no | Ids this page absorbed when two pages turned out to be one game. |

A page must name either a store entry or an executable. A page that names
neither identifies nothing, and the lint says so.

### `[ids]`

Identifiers that belong to somebody else's namespace. Recognized keys are
`steam` (an integer app id), `gog` (a product id, as a string of digits), and
`umu` (a full umu id, `umu-...`). `source` and `seen` are required and describe
the whole table.

- `source` is one of `steam-sku`, `store-id`, `umu-database`, `manual`.
- `seen` is the date **you observed it**, `YYYY-MM-DD`. Not the release date,
  not today's date copied from another page.
- `umu` appears **only when the game really is in umu-database**. A game umu
  declined has no umu id, and writing one claims a membership it does not have.
  The lint requires `source = "umu-database"` alongside it.
- **A fact is recorded once.** A GOG product id *is* the GOG codename, so it
  goes in the store entry, not in `ids.gog` as well. The lint rejects the
  repetition.

### `[[stores.<store>]]`

Store keys use umu's own spelling: `steam`, `gog`, `egs`, `ubisoft`,
`zoomplatform`, `humble`, `itchio`, `amazon`, `battlenet`, `ea`, `umu`. Store
entries are always arrays, even for a single entry, so a consumer never needs a
special case for the single-entry shape.

| Field | Required | What it is |
|---|---|---|
| `codename` | yes | The launcher-facing product identifier: an Epic app name, a GOG product id, a Steam app id. Exactly as the launcher writes it, including capitalization. |
| `seen` | yes | The date you observed it. |
| `source` | yes | How it was learned. |
| `confidence` | yes | How sure you are. |
| `exe` | no | Basename or path as observed, recorded only where it was actually seen for **this** store's copy. |
| `edition` | no | Which edition this product is, when a store sells more than one of the same game (`Standard`, `Deluxe`, `Game of the Year`). |
| `note` | no | Free prose. |

`source` is one of:

| Value | Where the value came from |
|---|---|
| `heroic-config` | Heroic's own configuration for the installed game |
| `heroic-library` | Heroic's library listing |
| `lutris` | Lutris, its local database or its API |
| `detectable` | Discord's detectable list |
| `gog-catalog` | GOG's catalog |
| `egdata` | egdata |
| `umu-database` | umu-database |
| `manual` | you worked it out by hand, and the `note` says how |

`source` and `confidence` are claiming two different things, and a reviewer
reads them separately. **`source` is where a reviewer can go to see the same
thing you saw.** `manual` is honest and perfectly acceptable, but it puts the
whole weight on your `note`, so write one. **`confidence` is how sure you are
that this codename is this game**, which is a separate question from where you
read it: a source can be authoritative and still leave you guessing which of
two games a string refers to.

Read the levels as:

- `high` - you saw the identifier in the installed copy's own record, and there
  is nothing left to guess about.
- `medium` - the identity is well supported but indirect: matched through a
  title, a directory name, or one source agreeing with another.
- `low` - a plausible inference you could not confirm. Say so in the `note`.

The point of all three fields is that **a reviewer should be able to judge a
page without trusting whoever wrote it.** A `low` entry with an honest note is
worth more than a `high` one that nobody can check.

### Editions, remasters, DLC

- **Editions of one game are store entries, not pages.** Standard and Deluxe on
  the same store are two products but one game, and presence should say the
  game's name. Two entries, one page, an `edition` label on each.
- **A remaster or definitive edition sold as its own product does get a page**,
  because it is separately installed and separately named on screen. It points
  home with `variant_of`, and the lint checks that the target exists.
- **DLC gets nothing.**

The dividing line is whether a launcher ever has to name the thing. If it can
appear on screen as what is running, it earns a page.

### `helpers.toml`

An entry there says an executable ships beside many games and therefore can
never identify one. `exe` is a lowercase basename (consumers match
case-insensitively), `reason` says why it can never name a game, `seen` is a
date, and `incident` records the concrete mislabel that proved it, where one
exists. The three entries there now are Unity's crash handlers, and the
`incident` on `unitycrashhandler64.exe` is what the field is for.

Add an executable here only when it genuinely ships with many games. An
executable that is simply generic but belongs to one game is not a helper.

## 3. Pick the canonical id

Every page has one canonical id in its `gamedb` field. It is **derived where an
authority already names the game**, and minted only where none does. The
precedence, first match wins:

| Form | When |
|---|---|
| `steam-870780` | the Steam app id, when the game is on Steam |
| `umu-870780` | the umu id, when the game really is in umu-database |
| `gog-1660194629`, `egs-Calluna` | a store codename, taking stores in the order gog, egs, ubisoft, ea, battlenet, amazon, humble, itchio, zoomplatform |
| `gamedb-k7m2p9qz` | minted here, when nothing above names the game |

**The prefix names the namespace the identifier belongs to, and nothing parses
the shape of what follows.** `gog-1660194629` is a GOG product id and a
perfectly good canonical id. A numeric tail does not mean Steam and needs no
special handling.

Three things follow from that, and they are where contributors usually trip:

1. **You may leave the field out** whenever it is derivable. The lint works out
   the same answer from `[ids]` and the store entries either way. It is written
   down for visibility, not because anything depends on reading it.
2. **A minted `gamedb-` id is only for a page with no store identity at all**,
   which in practice means a page identified by executable. The house
   convention is eight characters of `[a-z0-9]`, and the rule the lint enforces
   is that it contains at least one letter, so it can never be mistaken for an
   authority's number. There is no registry and nothing to allocate: pick one,
   and the same pass that validates everything else checks it is unique.
3. **An id is assigned once and then frozen.** A page that later learns a
   better identifier records it in `[ids]` and keeps the id it already had, so
   nothing that referred to it breaks. The lint reports the mismatch as a
   warning, and a warning here means "correct, leave it alone".

A gamedb id is **not** a umu id. Never put one in `GAMEID` or `UMU_ID`; cross
over through `[ids]`, which is what it is for.

## 4. Run the checks

From the repository root:

    .scripts/gamedb-lint.py gamedb

It validates both schemas, the id rules, and the one-game-one-page rule across
the whole set, and exits non-zero when anything fails. That exit status is what
a merge request is gated on, so running it yourself is the whole of "checking
before you open the PR".

    OK: 2 pages + helpers valid

`.scripts/gamedb-lint-selftest.sh` checks the lint itself against a fixture set
carrying one page per rule. Run it if you changed the lint; it is not something
a data contribution needs.

Formatting is `taplo fmt --check`, so nobody argues about spacing in review.
If you do not have taplo, copy the layout of an existing page and a reviewer
will sort out the rest.

## 5. What the failures mean

The lint prints one line per problem. In plain language:

**`'egs-Calluna' is claimed by both control-gog.toml and control.toml - the
same game cannot live on two pages, so consolidate them into one`**

The mistake this catches is the common one: somebody adds a game's GOG copy as
a new page, not knowing the Epic copy is already here. Both pages now claim the
same identifier, and from the moment that lands a lookup has two answers and
neither is wrong.

**The fix is to move the store entry onto the existing page and delete the new
one.** Not to change the id, not to rename the file, and never to add a second
page. The same message with an `exe:` prefix means both pages named the same
executable, which is the same problem arriving by a different route: either
they are the same game, or the executable is a shared helper and belongs in
`helpers.toml` rather than on either page.

**`gamedb is 'steam-999999', which none of this page's identifiers justifies`**

The written id does not follow from anything on the page. Usually a typo, or an
id copied from another page. Fix the id, or add the `[ids]` entry that makes it
true. Deleting the `gamedb` line is also a valid fix when the id is derivable.

**`minted id 'gamedb-12345678' must be gamedb-<uid> and must contain a letter`**

An all-digit minted id would be indistinguishable from an authority's number.
Change one character to a letter.

**`names neither a store entry nor an executable, so it identifies nothing`**

The page is a title and nothing a lookup could ever match. Add the store entry
or the `exe` list you meant to add.

**`file name is neither the title slug 'control' nor that slug plus a
qualifier`**

Rename the file to the lowercase slug of `title`. When two different games
share a title, the page that exists keeps its name and the newcomer takes a
qualifier of its own (`prey.toml` and `prey-2017.toml`). Never rename the
incumbent.

**`umu id present without an umu-database source`**

`ids.umu` claims the game is in umu-database, so `ids.source` has to say that
is where it came from. If the game is not in umu-database, remove the umu id.

**`ids.gog repeats the gog codename`**

The same fact written twice. Keep the store entry, drop the `ids` key.

**`merged_from 'steam-870780' is the live id of control.toml`** and
**`merged_from 'x' was already absorbed by y.toml`**

Absorbed ids stay reserved forever: never a live id somewhere else, never
absorbed twice. If you meant to merge two pages, the survivor lists what it
absorbed and the other page is deleted in the same change.

**`variant_of 'gamedb-control' names no page here`**

The base game has no page, or the id is wrong. Check it against the base game's
`gamedb` field.

**`warning: id 'gamedb-k7m2p9qz' predates 'steam-1234567'; kept, since ids are
never re-pointed`**

Not a failure and nothing to fix. The page learned a better identifier after it
was created, and keeping the old id is correct. Do not "fix" this by changing
the id; that retires an identifier something may still be using.

**`unknown field`** or **`store key not allowed`**

A typo in a field or store name. Both schemas reject anything they do not know
about, which is what keeps a page from quietly carrying data nothing reads.

## 6. Open the pull request

One game per pull request where you can manage it. Say where the identifier
came from if the `note` does not already, and say what you are unsure about.
Deciding that two store identities are the same game is a human judgement, and
that judgement is what review is for.
