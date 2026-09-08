# Skill distribution

Read this guidance when changing distribution behavior, UI states, localization, or related documentation.

The semantic authority is [Distribution terminology in CONTEXT.md](../../CONTEXT.md#distribution-terminology), including target-level aggregation, Library filtering, and project scope.

- Use **已分发 / 未分发 / 部分分发** and **Distributed / Not distributed / Partially distributed** for Skill distribution states. Do not reintroduce enabled/disabled UI status labels for these states.
- Distribution does not mean an Agent has loaded or executed the Skill. Link health is separate.
- Use `DistributionState` for frontend display states.
- Preserve existing `Enable/Disable`, `Activation`, `desired`, `enabledAgentCount`, and `enabled/disabled` API/storage identifiers. Copy changes must not rename persistence contracts.
- Check distribution terminology consistently across code, locale catalogs, and documentation.
