# macOS 签名、公证、发布与应用更新策略

Skill Man 通过 **GitHub Releases** 分发 macOS 应用，采用 **MIT** 许可证。永久 Bundle Identifier 为 `io.github.rookiezoe.skillman`。

## 社区正式版通道（ADR-0026 修订）

无 Apple Developer 凭据时，社区版可用 Ad-hoc 签名发布正式 Latest。更新包使用独立的 Tauri 密钥签名；App 的 Ad-hoc 签名不等于 Developer ID 签名或公证。

- 只发布 Apple Silicon / macOS 13+ DMG、签名 `.app.tar.gz`、`.sig`、`latest.json` 和 SHA256SUMS。
- 只消费正式 Latest；Draft 和 Pre-release 不进入更新通道。
- 公钥在基础 Tauri 配置中，前后端可用性使用同一来源。缺失公钥的历史版本继续手动更新，不伪装成 updater 可用。
- 私钥及密码只进入受保护的 `community-release` Environment，要求维护者审核和发布 tag 限制；由维护者保留离线恢复副本。与 Apple credentials 独立。
- 明确触发 tag 发布构建，验证实际 checkout、main ancestry 和严格版本一致性；验签并回读五件附件后保留 Draft，维护者审核再 Publish。
- 日常 CI 使用 `npm run ci:local`；GitHub 托管构建用于明确触发的真实发布。
- v0.1.0 没有公钥，不能自举更新。先手动安装带公钥的基线版，再验证两个严格递增、数据兼容版本之间的更新。
- 允许 macOS 更新后再次要求「仍要打开」，必须预先提示、给出操作说明及手动下载兜底；不关闭 Gatekeeper 或自动清除 quarantine。

完整社区合同见 [ADR-0026](0026-community-app-updates.md)，操作步骤见[发布手册](../release.md)。#106 的通道与产物证据、#107 的真实升级和 Manual App Rollback 证据分别记录。以下 Apple 身份、公证与免手动放行门禁仅适用于 Developer ID 通道。

## 平台与产物

支持范围只包括 **Apple Silicon（aarch64）与 macOS 13+**。普通 Release 的首次安装产物是签名、公证并 stapled 的 `.dmg`；updater 产物是 Tauri 生成并独立签名的 `.app.tar.gz` 与 `latest.json`。Intel、macOS 11/12、universal binary 不进入本次 MVP。

GitHub Releases 是唯一权威发布源。所有 updater 可接收的 0.x 公开测试版都发布为普通 GitHub Release，而不是 GitHub Prerelease；0.x 版本号、release notes 与应用内文案负责表达“测试版”。这样单一更新通道可使用 `releases/latest/download/latest.json`。稳定版后再增加 Homebrew Cask；官网、自建 CDN 与 Stable/Beta 双通道不进入 MVP。

## Apple 身份与 Release Gate

当前维护者尚未加入 Apple Developer Program。开发阶段不因此受阻：本地使用未签名或 ad-hoc 构建，普通 CI 只运行测试和非发布构建；公开社区正式版须经过上述专用通道。

**Developer ID 通道首个 Release 的 Release Gate** 是：

1. Apple Developer Program 会员已激活（当前官方费用为 $99/年）；
2. 创建 `Developer ID Application` 证书并导出带私钥的 `.p12`；
3. 创建 App Store Connect API Key；
4. 把证书、密码、Issuer ID、Key ID 与 `.p8` 配置到受保护的 GitHub release Environment Secrets；
5. 完成一次签名、公证、staple、Gatekeeper 验证与 updater 安装演练。

Release Gate 未全部通过时，release workflow 必须失败并且不能产生公开 Release。Developer ID 通道不提供未签名/未公证的降级路径；社区版使用上述独立通道。Apple 官方要求见 [Developer Program](https://developer.apple.com/programs/)、[Developer ID certificates](https://developer.apple.com/help/account/certificates/create-developer-id-certificates/) 与 [Notarizing macOS software](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)。

## 签名、公证与密钥

应用使用 Developer ID Application 证书签名，Hardened Runtime 保持开启，由 Tauri CLI 调用 Apple 公证服务并 staple ticket。CI 公证采用 **App Store Connect API Key**，不使用绑定个人 Apple ID 的 app-specific password。

Tauri updater 的加密私钥与 Apple 证书分开管理：加密私钥及密码存放于 GitHub Actions Secrets，另在密码管理器或离线加密介质保留恢复副本；公钥提交仓库并嵌入应用配置。密钥轮换须在旧私钥仍可用时先发布包含新公钥的过渡版本。私钥丢失不得以关闭签名验证降级。

PR workflow 不接触任何发布 Secrets。Release workflow 使用最小 GitHub 权限和受保护 Environment；第三方 GitHub Actions 固定到完整 commit SHA。

## 发布流程

发布者先在 `main` 更新 Cargo、package 与 Tauri 配置中的版本，再创建 `vX.Y.Z` tag。在该 tag 上手动触发 release workflow，依次：

1. 校验 tag 与各处版本完全一致；
2. 运行测试和静态检查；
3. 构建 `aarch64-apple-darwin`；
4. 使用 Developer ID 签名并启用 Hardened Runtime；
5. 提交 Apple 公证、等待结果并 staple；
6. 执行 `codesign`、`spctl` 与安装启动冒烟验证；
7. 生成 updater 签名、`latest.json`、SHA-256 checksums 与 release notes；
8. 创建包含 `.dmg`、`.app.tar.gz`、签名、metadata 与 checksums 的 **Draft Release**。

维护者检查 Draft 产物后手动 Publish。Publish 是唯一公开闸门；tag 推送不会自动把未审阅产物暴露给用户。

## 应用更新体验

配置真实公钥的普通 Release 只有一个应用更新通道。应用启动后按 24 小时冷却后台检查；失败或离线不打扰用户。有新版时展示版本号、release notes 和下载大小，只有用户确认后才下载；下载完成后再次确认重启安装。设置中提供“检查更新”。不做后台自动下载、强制更新或 delta 更新。

本 ADR 只定义 Skill Man App 自身更新；Library 中 Skill 内容的更新遵循 ADR-0004。
