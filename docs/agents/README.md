# Agent instruction layout

The root [AGENTS.md](../../AGENTS.md) contains the project description, command exceptions, and task-based links. `CLAUDE.md` remains a symlink to that entrypoint. Read linked instructions only when relevant to the task.

```text
AGENTS.md
CLAUDE.md -> AGENTS.md
CONTEXT.md                         # Domain glossary and semantic contracts
docs/
├── agents/
│   ├── README.md                  # This layout and refactor notes
│   ├── domain.md                 # Glossary and ADR usage
│   ├── distribution.md           # State copy and compatible identifiers
│   ├── issue-tracker.md          # GitHub tracker conventions
│   ├── triage-labels.md          # Authoritative role-to-label mapping
│   └── wayfinding.md             # Maps, blockers, claiming, resolution
└── adr/                          # Architecture decisions; not duplicated here
```

## Refactor decisions

- Resolved the label conflict in favor of [triage-labels.md](triage-labels.md). Removed the root's "unmodified" restriction; the mapping and its customization guidance remain unchanged.
- Moved distribution instructions out of the root and consolidated the duplicate guidance from `domain.md` into [distribution.md](distribution.md).
- Kept task-specific tracker semantics and dependency details; moved wayfinding into its own file.
- Omitted a package-manager declaration because this project uses npm.
- Did not invent TypeScript, testing, API-design, or Git policies absent from the original instructions.

## Deletion review

Removed as redundant or unrelated:

- Basic `gh` create/comment/edit/close tutorials and repeated ticket-reading instructions: standard CLI knowledge, not project-specific constraints.
- Hypothetical multi-context trees, unrelated example ADR names, and ordering/billing examples: template content for a repository layout this project does not use.
- Instructions for triaging PRs when the request-surface flag is `yes`: inactive under the retained `no` policy.
- Template setup commentary around that PR flag and generic skill-dispatch references: not needed to carry out the retained repository rules.

No standalone "write clean code"-style rules were found. Replaced the vague missing-concept discussion with an explicit instruction to flag a terminology gap. No remaining deletion candidates require a policy decision.
