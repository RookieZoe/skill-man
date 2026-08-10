# Skill Man

Skill Man is a macOS desktop app for browsing and managing a local Library of AI Agent Skills. The MVP uses Tauri v2 with a UI-independent Rust core and a React/TypeScript Library Desk.

The MVP implementation covers the Library, Import, Activation, maintenance, Preferences, and application Update vertical slices:

- SQLite startup and transactional schema migration with read-only lockout on failure or unsupported future schemas;
- typed Catalog queries across the Rust core and Tauri DTO boundary;
- a fixture-backed, three-column Library Desk for the Skill list, read-only detail, and Enable by Agent inspector;
- previewed Enable/Disable for configured Claude, Codex, and Custom Agents through a typed Tauri boundary, with entry-level Activation symlinks created and removed by the Rust core;
- path-overlap, stale-plan, occupied-entry, and Disable target-mismatch guards before filesystem writes;
- startup health checks for every desired Activation, with persisted drift states and previewed Repair/Conflict handling;
- Link Import source discovery, Preview/Apply, SQLite-only Library pointers, blocking Library Conflict, and result-to-Activation continuation;
- folder/ZIP Install with two-stage discovery, 100-candidate truncation, multi-selection, Core-owned tree/link/size validation, and disk-space preflight;
- atomic stable-path file Install/reinstall with `tree-sha256-v1`, `file_sources` provenance, crash-recovery journals, and persisted Modified health;
- App Update checks with a persisted 24-hour cooldown, cancellable signed-archive download/discard, and separate Download / Install and Restart confirmations;
- a protected tag-to-Draft release workflow for Apple Silicon DMG, notarization, updater metadata, and checksums;
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

Create a local Apple Silicon `.app` and `.dmg` without release signing secrets:

```sh
npm run tauri build -- --target aarch64-apple-darwin --bundles app,dmg
open "src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Skill Man.app"
```

These local artifacts are for development and personal testing. Public distribution requires the Developer ID, notarization, Gatekeeper, and real-upgrade gates in [the release manual](docs/release.md).

## Verify

```sh
npm run format:check
npm run check:capabilities
npm run lint
npm run typecheck
npm test
npm run test:release
npm run build:web
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
```

The browser-only surface remains fixture-backed. Native mode treats SQLite and the app-owned Library as authoritative, and keeps filesystem, Git, updater, and lifecycle capabilities behind typed Rust adapters.

Architecture and product language are defined in [the MVP implementation spec](docs/mvp-implementation-spec.md), [CONTEXT.md](CONTEXT.md), and the [release manual](docs/release.md).
