# macOS 签名、公证、发布与应用更新策略

Skill Man 通过 **GitHub Releases** 直接分发签名、公证后的 macOS 应用，并使用 Tauri v2 官方 updater 完成应用内更新。首个公开测试版前开源，采用 **MIT** 许可证。永久 Bundle Identifier 为 `io.github.rookiezoe.skillman`。

## 平台与产物

首版只支持 **Apple Silicon（aarch64）与 macOS 13+**。首次安装产物是签名、公证并 stapled 的 `.dmg`；updater 产物是 Tauri 生成并独立签名的 `.app.tar.gz` 与 `latest.json`。Intel、macOS 11/12、universal binary 不进入本次 MVP。

GitHub Releases 是唯一权威发布源。所有 updater 可接收的 0.x 公开测试版都发布为普通 GitHub Release，而不是 GitHub Prerelease；0.x 版本号、release notes 与应用内文案负责表达“测试版”。这样单一更新通道可使用 `releases/latest/download/latest.json`。稳定版后再增加 Homebrew Cask；官网、自建 CDN 与 Stable/Beta 双通道不进入 MVP。

## Apple 身份与 Release Gate

当前维护者尚未加入 Apple Developer Program。开发阶段不因此受阻：本地使用未签名或 ad-hoc 构建，普通 CI 只运行测试和非发布构建，且不得把这些产物作为公开下载发布。

**首个公开测试版的 Release Gate** 是：

1. Apple Developer Program 会员已激活（当前官方费用为 $99/年）；
2. 创建 `Developer ID Application` 证书并导出带私钥的 `.p12`；
3. 创建 App Store Connect API Key；
4. 把证书、密码、Issuer ID、Key ID 与 `.p8` 配置到受保护的 GitHub release Environment Secrets；
5. 完成一次签名、公证、staple、Gatekeeper 验证与 updater 安装演练。

Release Gate 未全部通过时，release workflow 必须失败并且不能产生公开 Release。公开分发不提供未签名/未公证的降级路径。Apple 官方要求见 [Developer Program](https://developer.apple.com/programs/)、[Developer ID certificates](https://developer.apple.com/help/account/certificates/create-developer-id-certificates/) 与 [Notarizing macOS software](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)。

## 签名、公证与密钥

应用使用 Developer ID Application 证书签名，Hardened Runtime 保持开启，由 Tauri CLI 调用 Apple 公证服务并 staple ticket。CI 公证采用 **App Store Connect API Key**，不使用绑定个人 Apple ID 的 app-specific password。

Tauri updater 的加密私钥与 Apple 证书分开管理：加密私钥及密码存放于 GitHub Actions Secrets，另在密码管理器或离线加密介质保留恢复副本；公钥提交仓库并嵌入应用配置。密钥轮换须在旧私钥仍可用时先发布包含新公钥的过渡版本。私钥丢失不得以关闭签名验证降级。

PR workflow 不接触任何发布 Secrets。Release workflow 使用最小 GitHub 权限和受保护 Environment；第三方 GitHub Actions 固定到完整 commit SHA。

## 发布流程

发布者先在 `main` 更新 Cargo、package 与 Tauri 配置中的版本，再创建 `vX.Y.Z` tag。Tag 触发 release workflow，依次：

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

MVP 只有一个应用更新通道。应用启动后按 24 小时冷却后台检查；失败或离线不打扰用户。有新版时展示版本号、release notes 和下载大小，只有用户确认后才下载；下载完成后再次确认重启安装。设置中提供“检查更新”。不做后台自动下载、强制更新或 delta 更新。

本 ADR 只定义 Skill Man App 自身更新；Library 中 Skill 内容的更新遵循 ADR-0004。
