# skill-man

## Agent skills

### Issue tracker

Issues are tracked as GitHub issues on this repo (via the `gh` CLI). See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage labels, unmodified: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.

### Skill distribution terminology

Use **已分发 / 未分发 / 部分分发** for Skill distribution states, with English **Distributed / Not distributed / Partially distributed**. Follow the Distribution terminology section in `CONTEXT.md` for target-level aggregation, Library filtering, and project scope. Distribution does not mean an Agent has loaded or executed the Skill; link health is separate.

Use `DistributionState` for frontend display states. Keep existing `Enable/Disable`, `Activation`, `desired`, and `enabled/disabled` API/storage identifiers compatible. Do not reintroduce the old enabled/disabled UI status labels or rename persistence contracts as part of copy changes.
