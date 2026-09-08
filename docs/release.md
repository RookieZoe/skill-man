# Skill Man 发布手册

本手册落实 [ADR-0006](adr/0006-macos-distribution-and-updates.md)：Skill Man 只发布 Apple Silicon / macOS 13+ 版本。两条工作流都由维护者手动触发，只创建 Draft；核对后才发布。

## 无 Apple 凭据的社区 Release

此通道不需要 Apple 或 updater secrets。先公开仓库、检查提交历史中的敏感内容，确认 README、MIT LICENSE、截图和 `docs/releases/vX.Y.Z.md` 已提交。基础 Tauri 配置必须保留 `signingIdentity: "-"`、`createUpdaterArtifacts: false` 和 updater 公钥 sentinel，原生应用使用手动更新。设置中的“检查更新”仅查询公开 GitHub Release 元数据（包含预发布版本），发现新版后打开对应 Release 页面；不会下载或安装更新，也不使用 updater 公钥。

1. 在 Apple Silicon Mac 上运行 `npm run ci:local`，确认通过并提交发布改动。
2. 同步 main 后，在该 commit 创建并推送严格的 `vX.Y.Z` tag。版本字段必须与 tag 一致。
3. 从 main 手动触发 Community Release：

   ```sh
   gh workflow run prerelease.yml --ref main -f tag=v0.1.0 -f prerelease=false -f replace_existing=true
   ```

4. 工作流在 macOS runner 上跑完整验证、构建 Ad-hoc app 和 DMG、验证签名完整性和 arm64 架构，挂载 DMG 比对包内可执行文件，生成校验和，再创建 Draft Release。
5. 下载 Draft 的 DMG 和 SHA256SUMS，执行 `shasum -a 256 -c SHA256SUMS`，确认包内版本与签名正确。默认不会覆盖已有 Release。维护者明确要求重发同一 tag 时，可传 `replace_existing=true`；工作流只在构建和验证成功后将原 Release 转为 Draft，再替换两个附件，等待审核。
6. 核对发布说明中的未公证、手动更新和人工验收限制，预发布保持非 Latest；正式社区版使用 `prerelease=false` 构建，审核附件后执行 `gh release edit vX.Y.Z --draft=false --prerelease=false --latest`。只能有 DMG 和 SHA256SUMS 两个附件，不上传 latest.json 或 updater 包。
7. 未登录 GitHub 下载公开附件并再次校验。干净 macOS 用户或 VM 的浏览器下载、首次启动、手动放行与基本功能验收由维护者完成并记录；未完成时必须在 Release notes 明示，不能宣称已通过普通 Release 门禁。

用户首次打开可能需要在「系统设置 → 隐私与安全性」中选择「仍要打开」，见 [Apple 官方说明](https://support.apple.com/en-us/102445)。不建议关闭整个 Gatekeeper。公司管理的 Mac 可能禁止手动放行。

## 普通 Release：签名与公证

以下门禁适用于未来具有 Developer ID 签名、公证和 updater 的普通 Release，社区版（包括正式版）不替代这些证据。

## 一次性准备

1. 在首个公开测试版前把仓库设为 public，并确认根目录的 MIT `LICENSE` 已提交。
2. 加入 Apple Developer Program，创建 `Developer ID Application` 证书并把包含私钥的 `.p12` 导出为 base64。
3. 创建 App Store Connect API key，离线保存只能下载一次的 `.p8`，再把文件内容转成 base64。
4. 使用 `npm run tauri signer generate` 创建带密码的 updater 密钥对。把真实公钥写入 `src-tauri/tauri.conf.json`，替换 `UPDATER_PUBLIC_KEY_REQUIRED_FOR_RELEASE`；该 sentinel、空公钥或没有启用 updater 产物的 release 配置都会使工作流 fail closed。私钥和密码只进入受保护的 CI secrets，并各保留一份离线恢复副本。
5. 在 GitHub 创建名为 `release` 的 Environment：
   - 添加 required reviewer，并建议禁止发起者自审；
   - Deployment branches and tags 只允许 `v*` tag；
   - 把下表 secrets 全部配置为 Environment secrets，不要配置成普通仓库 secrets。

| Secret | 内容 |
|---|---|
| `APPLE_CERTIFICATE` | `.p12` 的单行 base64 |
| `APPLE_CERTIFICATE_PASSWORD` | 导出 `.p12` 时设置的密码 |
| `APPLE_API_ISSUER` | App Store Connect Issuer ID |
| `APPLE_API_KEY` | App Store Connect Key ID，不是 `.p8` 正文 |
| `APPLE_API_KEY_P8_BASE64` | `.p8` 文件的单行 base64 |
| `TAURI_SIGNING_PRIVATE_KEY` | Tauri updater 私钥内容 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | updater 私钥密码 |

临时 macOS keychain 的密码由 runner 每次随机生成，不需要长期 secret。发布工作流在 `always()` cleanup 中恢复默认 keychain，并删除临时 `.p12`、`.p8` 与 keychain。

## 创建 Draft Release

发布前先在 `main` 同步以下版本，且必须使用不带 prerelease/build 后缀的 `X.Y.Z`：

- `package.json`；
- `package-lock.json` 顶层与 `packages[""]`；
- `src-tauri/Cargo.toml`；
- `src-tauri/tauri.conf.json`。

提交并确认 `main` 的本地 CI 通过后，在该 commit 创建并推送严格的 `vX.Y.Z` tag：

```bash
git tag -s v0.1.0 -m "Skill Man v0.1.0"
git push origin v0.1.0
```

在该 tag 上手动运行 `gh workflow run release.yml --ref v0.1.0`。`.github/workflows/release.yml` 会按顺序执行：

1. 确认 tag 指向 `main` 上的 commit，并在构建前校验 tag 与四处版本完全一致；
2. 在不接触发布 secrets 的 `verify` job 跑完整格式、边界、lint、typecheck、前后端测试和构建门禁；
3. 等待 `release` Environment 人工批准，之后才读取 Apple 与 updater secrets；
4. 合并 `src-tauri/tauri.release.conf.json`，构建 Developer ID 签名、ASC 公证并 stapled 的 aarch64 DMG，以及已独立签名的 `.app.tar.gz` updater 包；
5. 执行 `hdiutil`、`codesign`、`spctl`、`stapler` 与 Mach-O 架构验证，把 App 复制到临时 Applications 目录并完成启动冒烟；
6. 通过 `scripts/release/prepare-release.mjs` 再次校验 tag/四处版本，并生成 `latest.json` 与 `SHA256SUMS`；
7. 创建 `draft=true`、`prerelease=false` 的 GitHub Draft Release，上传 DMG、updater 包与 `.sig`；Draft 不进入 latest，人工 Publish 时必须设为 Latest；
8. 再上传自定义 `latest.json` 与 checksums，并从 GitHub API read-back Draft 状态、完整 asset 集合和远端 metadata 字节。

任何一步失败都不会 Publish。若失败发生在 Draft 已创建之后，Draft 可能保留用于诊断；在修复并重跑前，维护者应确认它没有被发布，并在 GitHub UI 删除明确的失败 Draft。不要把未通过 Gate 的产物作为下载链接传播。

## 产物契约

每个 Draft 必须且只能包含本次支持范围内的五类 asset：

- 一个名称标明 `aarch64` 的 `.dmg`；
- 一个 `.app.tar.gz`；
- 与 updater 包同名的 `.app.tar.gz.sig`；
- `latest.json`；
- `SHA256SUMS`。

`prepare-release.mjs` 对产物树中的符号链接、缺失/重复/额外架构产物、空签名、空 release notes 和任一版本漂移都 fail closed。其输出不含时间戳，因此同一组输入会生成相同的 metadata 与 checksums。

`latest.json` 只包含 `darwin-aarch64` 通道。顶层 `download_size` 是 `.app.tar.gz` 的精确字节数，供更新确认界面在开始下载前展示；`notes` 与 Draft Release notes 使用同一份生成结果。固定入口是：

```text
https://github.com/RookieZoe/skill-man/releases/latest/download/latest.json
```

GitHub 的 `latest` 不包含 Draft 或 Prerelease。因此需要接入 updater 的 0.x 签名测试版必须发布为普通 Release，不能勾选 “This is a pre-release”；社区测试版不接入该通道。

## 人工 Publish Gate

工作流变绿只证明 CI 能验证的部分。维护者还必须在 Apple Silicon Mac 上完成以下步骤，并保存版本号、机器/macOS 版本和结果记录。

### 1. 检查 Draft

- Draft 的 tag、版本和 release notes 正确；
- `Prerelease` 为 false；
- 五类 asset 齐全；
- 本地执行 `shasum -a 256 -c SHA256SUMS` 通过；
- `latest.json` 的版本、notes、`download_size`、URL 与签名正确。

### 2. 干净首次启动

在从未运行过该版本的干净 macOS 用户或 VM 中，用浏览器下载 DMG，使文件带真实 quarantine 属性。挂载并复制到 `/Applications` 后首次启动：

- Gatekeeper 显示已识别开发者，不要求“仍要打开”等绕过；
- 应用正常进入首次启动流程；
- 建议在首次启动前断网一次，确认 stapled ticket 可供 Gatekeeper 离线验证。

CI runner 上从 DMG 复制到临时 Applications 目录后直接启动的 app 没有浏览器 quarantine，也不是干净用户环境，不能代替这项验收。

### 3. 人工 Publish

回到 GitHub Draft 页面，再次确认未勾选 Prerelease，点击 **Publish release**，并把该版本设为 Latest。不要用自动 Publish workflow 绕过这一步。

Publish 后，从未登录 GitHub 的环境读取固定入口，确认返回本次版本：

```bash
curl --fail --location \
  https://github.com/RookieZoe/skill-man/releases/latest/download/latest.json
```

### 4. 真实升级与回滚

从上一普通、签名版本启动 Skill Man，完成真实更新演练：

1. 检查更新后，提示展示新版本、release notes 和下载大小；
2. 第一次确认后才开始下载；
3. 下载和 updater 签名验证完成后，再次确认才安装并重启；
4. 重启后的应用版本正确，Library 与 Preferences 保持可用；
5. 从 GitHub Releases 手动下载安装上一签名版本，完成回滚演练。

首个公开版本没有“上一公开签名版本”，单独一次发布无法证明真实升级链。必须先用两份真实签名版本做受控演练，或把该 Gate 明确保留到下一次普通 Release；在完成前不得声称 #29 的真实升级 Gate 已通过。

## 本地检查

release CLI 的行为测试不需要任何 secret：

```bash
npm run test:release
```

常规 PR / `main` CI 只做无 bundle 构建，不引用 `release` Environment，也不会获得发布 secrets。静态检查或普通 CI 变绿不代表 Apple 公证、干净首次启动、人工 Publish 或真实升级已经完成。

## 官方参考

- [Tauri GitHub pipeline](https://v2.tauri.app/distribute/pipelines/github/)
- [Tauri macOS signing and notarization](https://v2.tauri.app/distribute/sign/macos/)
- [Tauri updater](https://v2.tauri.app/plugin/updater/)
- [Apple notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)
- [GitHub Environments](https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments)
- [GitHub latest release semantics](https://docs.github.com/en/rest/releases/releases#get-the-latest-release)
