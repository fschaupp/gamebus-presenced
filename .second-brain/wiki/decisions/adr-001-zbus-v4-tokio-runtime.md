---
date: 2026-08-04
type: decision
title: Use zbus v4 for D-Bus bindings with tokio runtime
status: accepted
related-projects: [gamebus-presenced]
ai-first: true
tags: [decision, architecture, d-bus, rust]
---

## For future Claude

ADR-001: Decision to use zbus v4 with tokio runtime for D-Bus bindings in gamebus-presenced project.

## Context

The gamebus-presenced project requires D-Bus bindings to publish presence information on the Linux session bus. We need to choose a D-Bus library that:
- Provides async support for non-blocking I/O
- Has good Rust ecosystem integration
- Supports both client and server-side D-Bus interfaces
- Is actively maintained

## Decision

Use **zbus v4** with **tokio runtime** for all D-Bus bindings.

## Rationale

- zbus is the most mature and widely-used Rust D-Bus library
- v4 provides stable async support with tokio compatibility
- Excellent documentation and community adoption
- Supports both session and system bus
- Provides macros for easy interface definition
- Compatible with zvariant for complex type handling

## Consequences

- All D-Bus code will use zbus::Connection with tokio runtime
- Interface definitions will use zbus macros
- Error handling follows zbus error types
- Dependencies: zbus = "4", tokio = { version = "1", features = ["full"] }

## Alternatives Considered

- dbus-rs: Older, less ergonomic API, blocking by default
- dbus-tokio: Separate crate, less integrated
- Custom zbus v3: Lacks some async features we need

## Related

- [[zbus]]
- [[tokio]]
- [[gamebus-presenced]]
