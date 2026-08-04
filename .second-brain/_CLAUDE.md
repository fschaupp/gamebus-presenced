# Claude Operating Manual - Florian's Second Brain Vault

> Read this file before doing anything in this vault.
> This is the single source of truth for how Claude operates here.

---

## Section 0 - AI-First Vault Rule (read first, applies to every note)

This vault is designed for **future-Claude** to read and reason over, not for human review. The owner rarely reads notes directly - they call Claude to retrieve, synthesize, and connect dots across years of accumulated knowledge.

**Every note Claude writes to this vault must follow these rules:**

1. **Self-contained context** - Each note must explain itself. Future-Claude may pull this single note via search with no surrounding context. Don't rely on backlinks alone for meaning.
2. **"For future Claude" preamble** - Every note begins with a 2-3 sentence summary in plain English under a `## For future Claude` header so Claude can decide relevance in 10 seconds before parsing the structured data.
3. **Rich, consistent frontmatter** - Filterable metadata (`type`, `date`, `topic`, `tags`, `related-people`, `related-projects`, `sources`, `confidence`). Different note types may have different schemas, but every note has machine-readable frontmatter.
4. **Recency markers per claim** - When stating external facts, attach the date: "Mem0 raised $24M (as of 2026-04)" so future-Claude knows what to verify before trusting.
5. **Sources preserved verbatim** - Every external claim has its source URL inline so it can be re-verified or refreshed.
6. **Cross-links are mandatory** - Every person, project, idea, decision, or concept referenced uses `[[wikilinks]]` so the graph is traversable.
7. **Confidence levels** - Where applicable, mark claims as `stated | high | medium | speculation` so future-Claude knows what to trust vs verify.

This rule applies to all `/obsidian-*` and `/research*` commands, all scheduled agents, and any direct vault writes.

---

## Section 0.5 - Verify Live State Before Acting

Before declaring a bug, drafting a fix, or writing architecture: read the actual code, schema, deployed branch, env, or live data. Speculation from stale context burns hours and produces drafts that contradict reality.

Specific cues:
- Read the schema or types before declaring a bug (real field names live in the code, not in memory)
- `git fetch origin` and read the deployed branch, not local `main`
- Grep the live file before any anchor-based patch
- Fetch live time, dates, and rates (never infer from training data)
- Verify env vars in the running process before blaming code
- Mock tests miss schema drift: read one real payload before declaring "done"

This is a general operating principle, not vault-specific. Keep it in `_CLAUDE.md` so every Claude session in this vault inherits it.

---

## Vault Identity

- **Owner:** Florian
- **Primary purpose:** Technical knowledge base - gamebus-presenced project, software engineering, research, and development work
- **Last updated:** 2026-08-04

---

## Folder Map

| Folder | Purpose |
|---|---|
| `wiki/daily/` | One note per day. Named `YYYY-MM-DD.md` |
| `wiki/projects/` | Active and archived projects |
| `wiki/tasks/` | Standalone task notes (linked from boards) |
| `boards/` | Kanban boards |
| `wiki/entities/` | One note per person, company, or tool |
| `wiki/logs/` | Technical work logs - dated, project-tagged |
| `Research/` | Research outputs (web, deep, x-pulse, x-reads, YouTube, NotebookLM) |
| `wiki/concepts/` | Ideas, frameworks, methodologies, synthesis |
| `wiki/reviews/` | Weekly and monthly reviews |
| `wiki/decisions/` | Architectural decision records (ADRs) |
| `wiki/meetings/` | Meeting notes |
| `wiki/agenda/` | Calendar snapshots |
| `raw/` | Immutable raw sources (articles, transcripts, pdfs, videos) |
| `Templates/` | Note templates |

---

## Key Files

- **Dashboard:** `index.md` - main navigation and catalog
- **Operations log:** `log.md` - pointer to `Logs/YYYY-MM-DD.md` files
- **AI-first rules:** `.opencode/references/ai-first-rules.md` (external reference)

---

## Active Context

> Update this section at the start of each major project or focus period.

**Current top priority:** gamebus-presenced - D-Bus presence daemon for Linux desktop
**Current project:** gamebus-presenced (S0+S1+S2 complete, S3 next)
**Key technologies:** D-Bus, Discord IPC, GameMode, Rust, zbus, tokio, rsrpc

---

## Auto-Save Rules

Claude should auto-save the following **without asking**:
- Decisions made in conversation → relevant project note + daily note
- New people mentioned → wiki/entities/ (create stub if needed)
- Tasks assigned or committed to → kanban board + wiki/tasks/ note
- Dev work done → wiki/logs/ + project note + daily note
- Mentions/recognition from colleagues → Mentions Log + person's note
- Completed tasks → move on kanban to ✅ Done

Claude should **ask before saving**:
- Anything touching Finances/ or personal financial data
- Private/ or Journal/ (private notes)
- Anything that involves deleting or archiving an existing note

---

## Naming Conventions

| Type | Pattern | Example |
|---|---|---|
| Daily note | `YYYY-MM-DD.md` | `2026-08-04.md` |
| Dev log | `YYYY-MM-DD - Description.md` | `2026-08-04 - Discord IPC Listener.md` |
| Entity | Full name (flat) | `Florian.md`, `GameMode.md` |
| Concept | Descriptive title | `D-Bus Presence Protocol.md` |
| Project | Proper name | `gamebus-presenced.md` |
| Source | `YYYY-MM-DD - Source Title.md` | `2026-08-04 - Discord IPC Spec.md` |
| Decision | `ADR-YYYY-MM-DD - Title.md` | `ADR-2026-08-04 - D-Bus Interface Design.md` |
| Archive prefix | `_archived_` | `_archived_old-idea.md` |

---

## Frontmatter Requirements

Every note must have at minimum:
```yaml
---
date: YYYY-MM-DD
type: <note-type>
tags: [note-type]
ai-first: true
---
```

Note types: `daily` | `project` | `task` | `person` | `devlog` | `decision` | `concept` | `idea` | `review` | `source` | `meeting` | `agenda-snapshot` | `recurring-task` | `architecture-overview` | `architecture-module` | `brainstorm` | `synthesis` | `emerge` | `connect` | `challenge` | `distillation` | `podcast` | `adr`

---

## Kanban Convention

Columns in boards: `📥 Backlog` · `📋 This Week` · `🔨 In Progress` · `⏳ Waiting On` · `✅ Done`

Priority: 🔴 critical · 🟡 important · 🟢 low

Item format:
```
- [ ] 🔴 **Title** · @{YYYY-MM-DD}
	Description. [[Related Project]] [[Person]]
```

Completed:
```
- [x] ~~🔴 **Title**~~ ✅ Date
```

---

## Propagation Rules

| Event | Also update |
|---|---|
| New project | Board (Backlog) + today's daily note |
| Task done | Board (Done, strikethrough) + project note + daily note |
| Dev session | wiki/logs/ + project note (Recent Activity) + daily note |
| Person interaction | Daily note + their wiki/entities/ note |
| Decision made | Project note (Key Decisions) + daily note |
| Mention/recognition | Mentions Log + person's note + daily note |
| Deal update | Deal file + Side Biz board + daily note |

---

## Projects Currently Active

> Keep this list current. Claude uses it to route context correctly.

- `[[wiki/projects/gamebus-presenced]]` - D-Bus presence daemon for Linux desktop (S0+S1+S2 complete, S3 next)

---

## Do Not Touch

- `Templates/` - Never modify templates during normal vault operations
- `raw/` - Immutable raw sources. Read only, never modify.

---

## Vault Style

This vault uses **wiki-style** folder structure as defined in `.opencode/references/folder-map.md`.

---

*This file was generated by the obsidian-second-brain skill.*
*Regenerate with: "Claude, update my _CLAUDE.md"*
