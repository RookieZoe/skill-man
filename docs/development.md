# Development

Skill Man uses Tauri v2, a Rust core, and React/TypeScript. Product behavior and terminology are defined in [CONTEXT.md](../CONTEXT.md) and the [vNext implementation spec](vnext-implementation-spec.md). The [MVP spec](mvp-implementation-spec.md) is a historical baseline.

## Run

Requirements: an Apple Silicon Mac running macOS 13+, Xcode, Node.js 22.20+, and Rust 1.85+.

```sh
npm ci
npm run tauri dev
```

`npm run dev` starts the browser-only demo with sample data. The native app uses the configured Home and real filesystem data. Filesystem, Git, updater, and lifecycle operations stay behind typed Rust adapters.

## Build

Build a local Apple Silicon app and DMG without release signing secrets:

```sh
npm run tauri build -- --target aarch64-apple-darwin --bundles app,dmg
open "src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Skill Man.app"
```

Local bundles are for development and personal testing. Public distribution follows the community Pre-release or signed Release process in the [release manual](release.md).

`npm run build` builds only the web frontend. To check the native build without bundling:

```sh
npm run tauri build -- --no-bundle --target aarch64-apple-darwin
```

## Verify

```sh
npm run ci:local
```

Use `npm run ci:local:clean` to install locked dependencies first. The [local CI script](../scripts/ci-local.mjs) defines the complete gate, including formatting, capability and locale checks, lint, typechecking, frontend and Rust tests, release tooling tests, and builds.

Pushes and pull requests do not start ordinary hosted CI. GitHub Actions is retained as a manual emergency fallback. The separate, manually dispatched release workflows can use hosted macOS runners with maintainer authorization.
