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

## Implementation Drift Note (2026-08-04)

This ADR was written during S0 (interface design before any source existed). The S1 implementation drifted in two details (2026-08-06 addendum: two more below):
- `ListActivities` returns `ao` (object paths), not `Vec<Activity>` - corrected during S0→S1 transition.
- The property list evolved: the final Activity interface has `Sources`, `Kind`, `AppIds` (`a{ss}`), `Extra` (`a{sv}`), and uses different names for some fields (e.g., `ProcessId` not `Pid`, `Name` not `Title`).

The **decision** (Manager + per-activity objects, signals, read-only properties) stands. See [[wiki/logs/2026-08-04 - gamebus-presenced S1]] for the actual implemented interface.

### Drift addendum (2026-08-06 reconcile)

Two further drifts the 2026-08-04 note missed:
- Path case: objects live under `/Activity/<id>` (capital), not
  `/activity/<id>`.
- Signal payloads: `ActivityAdded`/`ActivityRemoved` both carry the **object
  path** (`o`), not `Activity`/id-string as written above
  (src/dbus/manager.rs).
The `Version: u64` here is CORRECT - the S0 dev log's "version string" is the
wrong side (src/dbus/manager.rs `pub fn version(&self) -> u64`).
