# 技术栈:Tauri v2(Rust + React/TS Web 前端)

Skill Man 采用 **Tauri v2** 构建:Rust 后端(文件系统/符号链接/目录监听)+ React + TypeScript Web 前端;构建打包用 Tauri CLI(内置 Developer ID 签名、notarytool 公证、`hardenedRuntime` 默认开启);自动更新用 Tauri 官方 updater 插件(强制签名)。

**为什么不是 SwiftUI 原生:** SwiftUI 的 `MenuBarExtra`(macOS 13+)+ Sparkle 组合在菜单栏形态上体验上限最高,但维护者无 macOS 开发经验需从零学 Swift/Xcode,且现有 TypeScript 与 Rust 技能基本闲置。Tauri 是唯一同时命中这两项技能的方案。

**为什么不是 Electron:** 运行时压缩包 ≈116MB(v43.1.1,比 Tauri 官方宣称的 600KB 大两个数量级),Chromium 多进程内存开销最高;对一个轻量常驻菜单栏工具是硬伤。且好用的自动更新(electron-updater)押在 electron-userland 社区项目而非 Electron 核心。

**已接受的代价:** ① 菜单栏 popover 观感需手工打磨(Tauri tray 是 NSStatusItem + 自控窗口,非 `MenuBarExtra` 一体形态);② 目录监听无官方插件,需在 Rust 侧接 `notify` crate 自行推 event;③ updater 在 macOS 上为整包 `.app.tar.gz` 更新,无 delta(靠包体小对冲);④ Tauri 插件 API 可能在 minor 版本破坏。

详据见 [docs/research/2026-07-20-macos-tech-stack.md](../research/2026-07-20-macos-tech-stack.md)。
