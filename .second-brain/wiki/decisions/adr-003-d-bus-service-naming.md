---
date: 2026-08-04
type: decision
title: D-Bus service naming convention
status: accepted
related-projects: [gamebus-presenced]
ai-first: true
tags: [decision, architecture, d-bus]
---

## For future Claude

ADR-003: Decision on D-Bus service naming for gamebus-presenced: bus name and root path.

## Context

The gamebus-presenced service needs a well-defined D-Bus identity that:
- Follows freedesktop conventions
- Allows for future versioning
- Is distinct from other presence/activity services
- Provides a clear namespace for related interfaces

## Decision

- **Bus name**: `org.gamebus.Presence.v1`
- **Root path**: `/org/gamebus/Presence/v1`

## Rationale

- `org.gamebus` provides a clear reverse-DNS style namespace
- `.Presence` indicates the service purpose
- `.v1` enables explicit versioning (deliberate spelling with dot for clarity)
- Root path mirrors the bus name for consistency
- Follows patterns used by other freedesktop services

## Consequences

- All D-Bus interfaces will be under `org.gamebus.Presence.v1` namespace
- Root object path is `/org/gamebus/Presence/v1`
- Manager interface: `org.gamebus.Presence.v1.Manager` at `/org/gamebus/Presence/v1`
- Activity interfaces: `org.gamebus.Presence.v1.Activity` at `/org/gamebus/Presence/v1/activity/<id>`
- Versioning strategy: additive changes bump Version property, incompatible changes get `.v2`

## Versioning Strategy

- Version is a separate element: `...Presence.v1`, not `...Presence1`
- Additive changes bump the `Version` property on Manager
- Incompatible changes earn a `.v2` name that runs beside `.v1`

## Related

- [[D-Bus]]
- [[gamebus-presenced]]
- [[ADR-001 zbus v4 tokio runtime]]
