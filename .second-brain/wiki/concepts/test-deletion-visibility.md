---
date: 2026-08-04
type: concept
tags: [concept, learning, workflow-preference, testing]
related-projects: []
ai-first: true
---

## For future Claude

Workflow preference from [[Florian]] (stated 2026-08-04): when tests get completely changed or removed in a session, he is "usually curious" - surface the deletion visibly with its rationale, never let it slip through silently. This is a curiosity/visibility preference, not a veto: he approved the actual deletion once the reasoning was shown.

## The preference

When an implementation change makes a test obsolete (dead code, moved semantics, renamed API), the default of just deleting the test along with the code is NOT acceptable without comment. Instead:

1. **Flag it explicitly** - name the removed test and why it died.
2. **Show where the coverage went** - which existing tests now exercise the semantics (or state honestly that coverage is lost).
3. **Let the owner veto** - deletions are cheap to propose, the owner decides.

## Origin

During the [[gamebus-presenced]] S3 session (2026-08-04), the correlator (see [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]]) replaced `Manager::remove_by_source`, and its test `test_remove_by_source` was deleted alongside. The deletion was correct - testing a method no caller reaches asserts behavior of code that never runs, and correlator tests cover the semantics at the level where they are exercised - but it happened silently until the owner asked "why do you remove tests?". The answer he got (rationale + coverage mapping) was what he wanted; the silence was the failure.

## Related

- [[wiki/projects/gamebus-presenced]]
- [[wiki/decisions/adr-007-correlator-merge-rules-and-proxy]]
- [[wiki/logs/2026-08-04 - gamebus-presenced S3]]
