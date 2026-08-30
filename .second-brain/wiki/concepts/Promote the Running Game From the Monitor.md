---
date: 2026-08-30
type: idea
tags: [idea]
ai-first: true
status: captured
related-projects: [gamebus-presenced]
---

## For future Claude

One-line: a keybinding on gamebus-setup's monitor tab that promotes the
currently running game (the bus activity under the cursor) into the
identity stash / gamebus-gamedb, as the manual override for launches the
daemon deliberately does not record.

Born from the ICARUS case (2026-08-30): a Steam launch whose appid Discord's
detectable.json already maps is not stashed - by design, there is no gap.
But the design also proved the reverse door is missing: when the owner WANTS
a curated game in the set anyway, the only path is waiting for a real miss.
A `p`-style key on the monitor tab would write the entry by hand, from the
elected state the bus already carries (name, appid, exe) - the same
"promoted by hand" opt-in `umu_promoted` already models for umu candidacy.
Provenance question at graduation: a hand promotion is `source: manual`, and
the entry must be honest that the daemon observed it live rather than
learned it from a launcher.

Graduates into [[wiki/projects/gamebus-presenced]] when picked up; design
context in [[wiki/concepts/2026-08-24 - Identity Misses Design]] and
[[wiki/concepts/Elected State Beats the Event Stream]].
