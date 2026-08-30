---
date: 2026-08-04
type: decision
title: Use simplified Activity type without lifetime parameters
status: accepted
related-projects: [gamebus-presenced]
ai-first: true
tags: [decision, architecture, rust, d-bus]
---

## For future Claude

ADR-002: Decision to use simplified Activity type without lifetime parameters to avoid zvariant::Value Clone issue in gamebus-presenced.

## Context

When working with zvariant::Value for D-Bus serialization, types with lifetime parameters cannot be cloned into zvariant::Value due to Rust's ownership model. The Activity type needs to:
- Be serializable to D-Bus via zvariant
- Support cloning for caching and reuse
- Handle complex game presence data

## Decision

Use a **simplified Activity type without lifetime parameters** that owns all its data.

## Rationale

- Avoids the zvariant::Value Clone issue entirely
- Simpler type signature and usage
- Still supports all required presence data fields
- Can be easily converted to/from D-Bus types
- Works seamlessly with zbus interface generation

## Consequences

- Activity struct will use owned String, u64, etc. instead of borrowed references
- Slightly higher memory usage from owning data instead of borrowing
- No lifetime complexity in type definitions
- Cleaner serialization/deserialization code

## Example Type Structure

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Activity {
    pub pid: u64,
    pub executable: String,
    pub title: Option<String>,
    pub details: Option<String>,
    pub state: Option<String>,
    pub steam_app_id: Option<u64>,
    pub start_time: Option<u64>,
    // ... other fields
}
```

## Related

- [[zvariant]]
- [[zbus]]
- [[gamebus-presenced]]

## Drift Note (2026-08-06)

The illustrative field list above is pre-S1; the real property set (`Sources`,
`Kind`, `Name`, `AppIds`, `Extra`, `ProcessId`, …) is recorded in
[[wiki/decisions/adr-004-manager-and-activity-interfaces]]'s drift note. The
decision itself - owned data, no lifetimes - stands.
