---
date: 2026-08-22
type: idea
topic: publishing game icons and cover art on the gamebus D-Bus presence records
tags: [idea, gamebus-presenced, dbus, detectable-json, artwork, discord-cdn, lutris]
related-projects: [gamebus-presenced]
sources:
  - "https://discord.com/api/v10/applications/detectable (the detectable.json the tools already cache)"
  - "https://cdn.discordapp.com/app-icons/1402417032258916553/6ce4ec16083fc0b2a5c778dece82b3fc.png (verified live 2026-08-22)"
  - "https://lutris.net/api/games/<slug> (coverart field, IGDB-sourced)"
confidence: medium
ai-first: true
---

## For future Claude

An idea for [[wiki/projects/gamebus-presenced]], measured 2026-08-22: the
D-Bus presence records currently carry only **text** (title, source,
timestamps), but the **artwork is already in hand**. The `detectable.json`
copy the project's CLI tools download carries `icon_hash` for 19,752 of 23,900
entries and `cover_image_hash` for 16,758, and the Discord CDN URL built from
an entry's application id plus one of those hashes resolves to a real PNG
(verified live). So a richer presence - a game icon or cover next to the
title on any consumer, e.g. an e-paper panel or a desktop widget - needs
**no new data source**. What it does need is a set of design decisions that
are all still open; they are recorded below as TBD, deliberately, not as
answers.

## The finding (measured 2026-08-22, confidence: high)

The project already fetches and caches Discord's detectable-games file (see
`gamebus-presence fetch-detectable`, and the install step that best-effort
fetches it). Counted over that file:

| Field | Entries carrying it | Of total |
|---|---|---|
| `icon_hash` | 19,752 | 23,900 |
| `cover_image_hash` | 16,758 | 23,900 |

The CDN URL is **constructible**, not discovered:

```
https://cdn.discordapp.com/app-icons/<application id>/<hash>.png
```

Verified live on 2026-08-22 with Control's entry (application id
`1402417032258916553`, icon hash `6ce4ec16083fc0b2a5c778dece82b3fc`):
**HTTP 200, `image/png`, 8,095 bytes**. The cover hash requested on the same
path returned **25,519 bytes**. Both are small enough that caching a handful
of played games is trivial in size terms.

Confidence on the URL shape: **high** (measured, two hashes, one live day).
Confidence that it stays stable: **medium** - it is an undocumented CDN
convention, not a contract, so a consumer must degrade to text if a fetch
fails or 404s.

## Why this matters here

The identity work this project already does is the hard half. Once a game is
resolved to a Discord detectable entry (see
[[wiki/concepts/Game Identity Data Sources]] for which source answers which
identity question), the artwork comes along for free with the entry that
named it. No extra catalog, no extra scrape, no extra dependency - the same
file that supplies the title supplies the icon.

## Open questions - all TBD, none of these are decided

1. **Should the daemon ever fetch art?** The daemon is **network-free by
   design** and that is a property worth keeping. So a fetch would have to
   live in the **CLI tools or an install step**, exactly like
   `fetch-detectable` does today: something else populates a cache, and the
   daemon only ever reads local bytes (or publishes a URL and touches
   nothing). Status: TBD.
2. **What goes on the bus?** Three candidate shapes, none chosen:
   - a **URL** (smallest payload, pushes the fetch and the failure handling
     onto every consumer, and makes consumers network-dependent),
   - a **cached local path** (consumer just reads a file, but the path is
     only meaningful on the same machine and the same user's cache dir),
   - **bytes on the bus** (self-contained, works for a remote or sandboxed
     consumer, but puts image payloads into D-Bus properties/signals, which
     is a size and churn question).
   Status: TBD.
3. **Cache location and eviction.** Presumably under `$XDG_CACHE_HOME`
   alongside the detectable cache, but nothing is decided about naming,
   per-entry vs per-game keying, size cap, or when an entry is evicted.
   Status: TBD.
4. **Redistribution and licensing.** Discord CDN images are Discord-hosted
   game artwork; Lutris `coverart` images are IGDB-sourced. Whether either
   may be cached, re-served, or shipped in a package is **unresolved** and
   must be answered before anything is redistributed rather than merely
   fetched per user. Status: TBD.

## Fallback source

Lutris exposes a `coverart` field on `https://lutris.net/api/games/<slug>`
(IGDB images). It is a **possible fallback** for entries where
detectable.json carries no hash (roughly 4,100 entries with no `icon_hash`,
7,100 with no `cover_image_hash`). Same licensing question applies, and more
sharply, since IGDB has its own terms. Confidence: **medium** - the field
exists and is documented by use, but it has not been exercised by this
project.

## What would make this real

In rough dependency order, none of it started:

1. Decide the bus shape (question 2) - it drives everything else.
2. Decide where the fetch lives (question 1) and keep the daemon network-free.
3. Answer licensing (question 4) before any caching that outlives a session
   or any packaging.
4. Only then: cache design (question 3) and the consumer side.

## Related

- [[wiki/projects/gamebus-presenced]] - the daemon and CLI this would extend;
  its network-free daemon rule is the main constraint here.
- [[wiki/concepts/Game Identity Data Sources]] - the survey that turned this
  up; it maps which source answers which identity question, and detectable.json
  is the one that carries the artwork hashes.
- [[wiki/logs/2026-08-22 - gamebus-presenced umu Scope Gate]] - the session
  these measurements came out of.
- [[boards/gamebus-presenced]] - tracked as the backlog item "Icons and cover
  art over the D-Bus".

## Sources

- Discord detectable games file (the copy the tools cache; counts measured
  against it 2026-08-22): https://discord.com/api/v10/applications/detectable
- Discord CDN app icon, verified live 2026-08-22 (HTTP 200, image/png, 8,095
  bytes):
  https://cdn.discordapp.com/app-icons/1402417032258916553/6ce4ec16083fc0b2a5c778dece82b3fc.png
- Lutris game API (`coverart`, IGDB images): https://lutris.net/api/games/<slug>
