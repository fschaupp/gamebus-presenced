---
date: 2026-08-04
type: decision
title: Manager and Activity D-Bus interface design
status: accepted
related-projects: [gamebus-presenced]
ai-first: true
tags: [decision, architecture, d-bus, interface]
---

## For future Claude

ADR-004: Decision on Manager and Activity D-Bus interface design for gamebus-presenced.

## Context

The gamebus-presenced service needs to expose presence information via D-Bus. We need to define:
- A Manager interface for service-level operations
- An Activity interface for per-activity information
- Signal emissions for presence changes

## Decision

### Manager Interface: `org.gamebus.Presence.v1.Manager`

**Methods:**
- `ListActivities() -> Vec<Activity>` - Returns all current activities

**Properties:**
- `HasActivity: bool` - Whether any activities are currently present
- `Version: u64` - Interface version number

**Signals:**
- `ActivityAdded(Activity)` - Emitted when a new activity is detected
- `ActivityRemoved(String)` - Emitted when an activity ends (activity ID)

### Activity Interface: `org.gamebus.Presence.v1.Activity`

**Properties (per-activity objects):**
- `Id: String` - Unique activity identifier
- `Pid: u64` - Process ID
- `Executable: String` - Executable path
- `Title: Option<String>` - Activity title (from Discord)
- `Details: Option<String>` - Activity details
- `State: Option<String>` - Activity state
- `SteamAppId: Option<u64>` - Steam application ID
- `StartTime: Option<u64>` - Process start time

**Object Paths:**
- Per-activity objects at `/org/gamebus/Presence/v1/activity/<id>`

## Rationale

- **ListActivities**: Provides snapshot of current state for new consumers
- **HasActivity**: Quick check without fetching full list
- **Version**: Allows consumers to detect interface changes
- **Per-activity objects**: Follows D-Bus best practices (like MPRIS)
- **Signals**: Enable real-time updates without polling

## Consequences

- Consumers can discover all activities via ListActivities
- Each activity has its own D-Bus object for fine-grained access
- Memory usage scales with number of activities
- Signals enable efficient event-driven consumption

## Implementation Notes

- Activity objects are created/destroyed dynamically as activities start/end
- Manager maintains the list of active activity object paths
- All properties are read-only from consumer perspective

## Related

- [[D-Bus]]
- [[MPRIS]] - Model for this interface design
- [[gamebus-presenced]]
- [[ADR-001 zbus v4 tokio runtime]]
- [[ADR-003 D-Bus service naming]]
