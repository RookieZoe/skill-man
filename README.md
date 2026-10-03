> [!IMPORTANT]
> **Archived / 已归档 — 2026-10-03**
>
> Skill Man is archived and is no longer maintained. We recommend [Magpie](https://github.com/yetone/magpie), which shares the same core approach to Skill management: keep Skills in one place and distribute them to the agents you use. Future development of Skill Man has stopped, and all open issues have been closed. The code, releases, and documentation below are retained for historical reference. Thank you for your support!
>
> Skill Man 已归档，停止维护。推荐使用 [Magpie](https://github.com/yetone/magpie)，它与本项目有着相同的核心 Skill 管理理念：集中管理 Skills，并分发到你使用的各个 Agent。Skill Man 不再继续开发，所有开放 issue 均已关闭；现有代码、发行版本及下方文档保留供历史参考。感谢大家的使用与支持！

# Skill Man

A macOS app for keeping your AI Agent Skills in one place and distributing them to the agents you use.

Browse your Library, read `SKILL.md`, and see which Skills are distributed to each Agent. Import from Git repositories, local folders, or ZIP files, and manage repository updates from one screen.

**Apple Silicon · macOS 13+ · English / 简体中文 · Light / Dark**

![Skill Man Library in light mode](docs/screenshots/library.png)

## What you can do

- Organize Skills by source and read their Markdown without leaving the app.
- Configure Agents such as Claude Code and Codex, or add your own skill directories.
- Preview global distribution changes before applying them, or distribute Skills to a project.
- Manage a Git repository's Skills together, inspect the tracked version, and preview updates.
- Find existing Skills on your Mac and review them before bringing them into your Library.
- Check for missing or modified Skills and inspect distribution link health.

Global distribution is tracked per target. Project distribution is a one-time operation; the project manages those files afterward. A distributed Skill is not proof that an Agent has loaded or executed it.

## Screenshots

### Library in dark mode

![Skill Man Library in dark mode](docs/screenshots/library-dark.png)

### Git repositories

![Git repository management in dark mode](docs/screenshots/repositories.png)

### Appearance and app updates

![Appearance and app updates in dark mode](docs/screenshots/settings-dark.png)

### Import from Git

![Git import in dark mode](docs/screenshots/import-dark.png)

### Agents

![Agent configuration in light mode](docs/screenshots/agents.png)

Screenshots show the native macOS app with an existing Library. Skills shown belong to their respective authors and are not bundled with Skill Man.

## Install

Download the Apple Silicon DMG from [GitHub Releases](https://github.com/RookieZoe/skill-man/releases), open it, and drag Skill Man to Applications.

Community releases use ad-hoc signing and are **not notarized by Apple**. If macOS blocks the first launch, verify the download source, then follow [Apple's Open Anyway instructions](https://support.apple.com/en-us/102445). Use **Check now** in Settings to find a newer release, then open its release page to download it. Each release includes SHA256SUMS and its remaining testing limitations.

## Develop

Built with Tauri v2, Rust, React, and TypeScript. Requires an Apple Silicon Mac, macOS 13+, Xcode, Node.js 22.20+, and Rust 1.85+.

```sh
npm ci
npm run tauri dev
```

For the browser demo, run `npm run dev`. Open `http://localhost:1420/?fixture=enable` for sample data; use the native app to work with real Skills.

```sh
npm run ci:local
```

Run the full verification gate locally before submitting changes. See [development commands](docs/development.md), the [domain model](CONTEXT.md), the [implementation spec](docs/vnext-implementation-spec.md), and the [release manual](docs/release.md) for details.

## Archive and license

This repository is read-only. Past discussions remain available in [Issues](https://github.com/RookieZoe/skill-man/issues?q=is%3Aissue+is%3Aclosed).

[MIT](LICENSE) © 2026 RookieZoe
