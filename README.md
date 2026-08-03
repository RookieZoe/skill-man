# Skill Man

Skill Man is a macOS desktop app for browsing and managing a local Library of AI Agent Skills. The MVP uses Tauri v2 with a UI-independent Rust core and a React/TypeScript Library Desk.

Issues #18 through #21 establish the first browse, activate, and Link Import vertical slice:

- SQLite startup and transactional schema migration with read-only lockout on failure or unsupported future schemas;
- typed Catalog queries across the Rust core and Tauri DTO boundary;
- a fixture-backed, three-column Library Desk for the Skill list, read-only detail, and Enable by Agent inspector;
- previewed Enable/Disable for configured Claude, Codex, and Custom Agents through a typed Tauri boundary, with entry-level Activation symlinks created and removed by the Rust core;
- path-overlap, stale-plan, occupied-entry, and Disable target-mismatch guards before filesystem writes;
- startup health checks for every desired Activation, with persisted drift states and previewed Repair/Conflict handling;
- Link Import source discovery, Preview/Apply, SQLite-only Library pointers, blocking Library Conflict, and result-to-Activation continuation;
- CI gates for formatting, lint, typechecking, tests, capability boundaries, and builds.

## Develop

Requirements: Node.js 22.20+, Rust 1.85+, Xcode, and an Apple Silicon Mac running macOS 13 or newer.

```sh
npm install
npm run dev
```

The browser-only Vite surface uses the same typed fixture as the component tests. It simulates the Link Import and Activation confirmation flows without touching the filesystem. The native Tauri surface materializes isolated fixture entities under the app-owned Library, seeds their metadata into SQLite on first run, and persists Link pointers and the Activation matrix there. Run the Tauri app to exercise real Link Import and Agent Activation writes:

```sh
npm run tauri dev
```

## Verify

```sh
npm run format:check
npm run check:capabilities
npm run lint
npm run typecheck
npm test
npm run build:web
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
```

The current tracer bullet seeds its initial catalog from fixtures, then treats SQLite as authoritative for imported Link records and native desired/observed Activation state across restarts. File/Git Import, Adopt, Preferences, and menu bar behavior belong to later implementation tickets.

Architecture and product language are defined in [the MVP implementation spec](docs/mvp-implementation-spec.md) and [CONTEXT.md](CONTEXT.md).
