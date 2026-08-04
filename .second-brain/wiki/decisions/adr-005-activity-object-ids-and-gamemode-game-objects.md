---
date: 2026-08-04
type: decision
title: Activity object IDs use pid_<pid> (underscore), and GameMode game details come from per-game objects
status: accepted
related-projects: [gamebus-presenced]
ai-first: true
tags: [decision, architecture, d-bus, gamemode]
---

## For future Claude

ADR-005: Two S1 implementation constraints discovered live on the bus: (1) activity object IDs use `pid_<pid>` with an underscore because hyphens are illegal in D-Bus object path elements; (2) GameMode's `GameRegistered`/`ListGames` carry a per-game object path, not the executable - details come from the `com.feralinteractive.GameMode.Game` object's properties.

## Context

During S1 implementation for [[wiki/projects/gamebus-presenced]], two assumptions broke against the live system (as of 2026-08-04, verified with busctl against gamemoded):

1. The planned `pid-12345` activity ID scheme was rejected by zbus at runtime: `invalid value: character '-', expected an alphanumeric character, '_' or '/'`. The D-Bus object path spec allows only `[A-Z][a-z][0-9]_` per element.
2. The design doc claimed "GameMode hands over (pid, executable) directly". Empirically, `GameRegistered`/`GameUnregistered` signals and `ListGames` carry `(pid, object_path)` where the path is a per-game object `/com/feralinteractive/GameMode/Games/<pid>` - not an executable path.

## Decision

1. **Activity object IDs use `pid_<pid>`** (underscore). Object paths are `/org/gamebus/Presence/v1/Activity/pid_<pid>`. The prefix stays self-describing on the bus while satisfying the D-Bus element charset. The correlator ([[wiki/decisions/adr-004-manager-and-activity-interfaces]], S3) may revisit the scheme when merging multi-source records.

2. **Game details resolve from the per-game object.** On `GameRegistered` or a `ListGames` seed entry, read the `Executable` and `Timestamp` properties from `com.feralinteractive.GameMode.Game` at the handed-over path. Fallbacks: `/proc/<pid>/exe` for the executable (already the design's documented alternative), local clock for the timestamp. The `Timestamp` property feeds the activity's `Since` field - more accurate than the daemon's receive time.

## Consequences

- All activity IDs must match `[A-Za-z0-9_]+` forever; any future scheme (uuid, hash) must avoid hyphens.
- `Activity::from_gamemode(pid, executable, since)` takes resolved values; the watcher owns the GameMode-specific resolution.
- The registration->property-read race (game exits between signal and read) resolves to empty executable + local-clock `since`; the record is still published and dies with the subsequent `GameUnregistered` (observed live with short-lived processes).
- Design doc corrected in the correlator section.

## Alternatives Considered

- Bare pid (`12345`) as ID: legal but less self-describing; rejected by owner preference for a prefixed form.
- `/proc/<pid>/exe` as primary executable source: works, but the Game object property is the official API and also provides `Timestamp`.
- Hyphenated IDs with path escaping: D-Bus has no escaping mechanism for path elements; impossible.

## Related

- [[wiki/decisions/adr-003-d-bus-service-naming]]
- [[wiki/decisions/adr-004-manager-and-activity-interfaces]]
- [[wiki/projects/gamebus-presenced]]
- [[wiki/logs/2026-08-04 - gamebus-presenced S1]]
