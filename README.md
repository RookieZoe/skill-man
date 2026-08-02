# Skill Man

Skill Man is a macOS desktop app for browsing and managing a local Library of AI Agent Skills. The MVP uses Tauri v2 with a UI-independent Rust core and a React/TypeScript Library Desk.

Issue #18 establishes the first read-only vertical slice:

- SQLite startup and transactional schema migration with read-only lockout on failure or unsupported future schemas;
- typed Catalog queries across the Rust core and Tauri DTO boundary;
- a fixture-backed, three-column Library Desk for the Skill list, read-only detail, and Enable by Agent inspector;
- CI gates for formatting, lint, typechecking, tests, capability boundaries, and builds.

## Develop

Requirements: Node.js 22.20+, Rust 1.85+, Xcode, and an Apple Silicon Mac running macOS 13 or newer.

```sh
npm install
npm run tauri dev
```

The browser-only Vite surface uses the same typed fixture as the component tests:

```sh
npm run dev
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

The current UI is deliberately read only. Import, Adopt, Activation writes, health repair, Preferences, and menu bar behavior belong to later implementation tickets.

Architecture and product language are defined in [the MVP implementation spec](docs/mvp-implementation-spec.md) and [CONTEXT.md](CONTEXT.md).
