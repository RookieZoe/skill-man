# Skill Man

Skill Man is a macOS desktop app for managing and distributing AI Agent Skills, built with Tauri v2, Rust, and React/TypeScript.

## Commands

- Typecheck: `npm run typecheck`.
- Web build only: `npm run build` (does not build the desktop app).
- Native build check: `npm run tauri build -- --no-bundle --target aarch64-apple-darwin`.
- Full verification: `npm run ci:local` on an Apple Silicon Mac. Use local CI; GitHub-hosted Actions are a manual emergency fallback, not a required gate.

## Task-specific instructions

Read only the guidance relevant to the task before working in that area:

- Domain behavior, architecture, and terminology: [Domain docs](docs/agents/domain.md).
- Skill distribution UI, localization, or API mappings: [Distribution](docs/agents/distribution.md).
- Issues and PRDs: [Issue tracker](docs/agents/issue-tracker.md).
- Triage: [Triage labels](docs/agents/triage-labels.md), the authoritative role-to-label mapping.
- Roadmap tickets and dependency ordering: [Wayfinding](docs/agents/wayfinding.md).
