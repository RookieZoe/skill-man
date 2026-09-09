# Skill Man 发布手册

本手册落实 [ADR-0006](adr/0006-macos-distribution-and-updates.md) 与 [ADR-0026](adr/0026-community-app-updates.md)。仅支持 Apple Silicon / macOS 13+。社区通道使用 Ad-hoc App 签名和独立的 Tauri 更新包签名；Developer ID 通道另需 Apple 凭据。两个工作流均手动触发、只创建 Draft，维护者核对后才 Publish。

v0.1.0 已发布产物没有 updater 公钥，继续通过 Release 页面手动下载。仓库新增的社区签名通道不改变该历史产物；首次接入必须手动安装带公钥的较新基线版。#106 的真实 Actions 构建、发布附件校验和维护者备份确认，以及 #107 的升级验收，必须分别记录，不能由代码或本地 CI 推断完成。

## 社区签名更新通道

### 密钥与受保护环境

- 长期 Tauri updater 公钥仅写入基础 `src-tauri/tauri.conf.json`，供前端构建和原生 adapter 使用同一来源。`tauri.release.conf.json` 只开启更新包产物，不覆盖公钥；本地普通开发构建不要求私钥。
- 加密私钥与密码存入 `community-release` Environment 的 `TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`，不使用普通仓库 Secrets，不提交或输出私钥。
- Environment 必须配置维护者 required reviewer，Deployment branches and tags 只允许 `v*` **tag**。单维护者仓库可由维护者本人审核；保留这项现实限制，不声称阻止自审。禁止绕过审核。
- 维护者另把加密私钥、公钥和密码保存到受控离线加密介质或密码管理器，检查可恢复性并确认。仅有本机副本、Secrets 名称或 CI 成功不算备份完成。私钥丢失不得关闭验签；轮换需先用旧密钥发布可信过渡版本。
- 无需 Apple Developer ID、公证或 Apple secrets。Apple 通道仍由 #105 独立跟踪。

### 构建与 Draft 审阅

1. 在 Apple Silicon Mac 运行 `npm run ci:local` 并提交；日常验证不运行 GitHub Actions。正式发布时，先同步所有版本字段、准备 `docs/releases/vX.Y.Z.md`，再推送 main 与相同 commit 的严格 `vX.Y.Z` tag。首次带公钥基线版本必须高于已公开的 v0.1.0，不覆盖历史 tag 或公开 Release。
2. 在该 **tag** 上明确触发发布构建：

   ```sh
   gh workflow run prerelease.yml --ref vX.Y.Z -f tag=vX.Y.Z
   ```

   文件名沿用 `prerelease.yml`，工作流名为 Community Release。它校验 dispatch ref、实际 checkout、tag、main ancestry 和各版本字段；先在不接触 Secrets 的 job 验证，再等待 `community-release` 人工审核后构建。
3. 构建 Ad-hoc App、DMG、签名 `.app.tar.gz` 与 `.sig`。验证 arm64、版本和 codesign，比较 DMG／更新包内 App 与原构建内容。`prepare-release.mjs` 用基础配置中的公钥验证真实签名，再生成 `latest.json` 和 SHA256SUMS。
4. 创建 `draft=true`、`prerelease=false`、尚非 Latest 的 Release，一次上传五件附件。工作流不覆盖已有 Release；失败 Draft 需维护者先检查再明确删除重跑。不会自动 Publish。
5. 工作流下载所有 Draft 附件，通过 `verify-release.mjs` 核对完整集合、Release notes、版本、URL、包大小、签名和校验和；仅比较 metadata 字节不足以替代附件回读。
6. 维护者审核五件附件、App 内容和发布说明，并完成适用的干净环境验收。社区发布说明须包含下节用户说明及未完成的原生验收项。审核完成后才发布为正式 Latest：

   ```sh
   gh release edit vX.Y.Z --draft=false --prerelease=false --latest
   node scripts/release/verify-release.mjs --repo-root . --repository RookieZoe/skill-man --tag vX.Y.Z --public
   ```

   在对应 tag checkout 运行校验；`--public` 不携带 GitHub token，读取 Latest、下载全部附件并验证固定 metadata 入口。校验失败时该发布不能视为更新通道就绪，需先停止推广并调查。Draft 审阅前的本地校验可以使用下载目录和 GitHub REST Release JSON：

   ```sh
   node scripts/release/verify-release.mjs --repo-root . --repository RookieZoe/skill-man --tag vX.Y.Z --assets-dir /path/to/assets --release-json /path/to/release.json
   ```

### 用户说明与恢复

只接收正式 Latest，排除 Draft 和 Pre-release。应用保留每日检查冷却与偏好，下载前确认，完整验签后再次确认安装重启；不自动下载或强制安装。取消或验签失败不会进入安装。

社区 App 尚未经 Apple 公证。首次安装或更新后，macOS 可能要求在「系统设置 → 隐私与安全性」中选择「仍要打开」，随后重新打开应用；见 [Apple 说明](https://support.apple.com/en-us/102445)。不关闭 Gatekeeper，也不自动移除 quarantine。受管理的 Mac 可能禁止放行，不能承诺所有 macOS 免提示。

下载、验签或安装失败时保留应用内错误及[正式 Release 手动下载入口](https://github.com/RookieZoe/skill-man/releases/latest)。先按说明恢复启动；若需要 Manual App Rollback，只能手动安装与现有数据格式兼容的上一版，不降级数据库或恢复 Home 数据快照。

### 两个真实版本的验收方法（#107）

先发布并手动安装带同一公钥的基线版，再按相同步骤发布版本严格递增、数据兼容的目标版。例如未来可用 v0.1.1 → v0.1.2；这是版本安排示例，不表示这两个版本已发布或兼容性已验收。

使用真实 GitHub Latest 链完成下载确认、取消、错误签名、二次安装确认、重启和兼容版本手动回滚。错误产物只用于受控测试，不污染公开 Latest。保留浏览器下载 quarantine，记录机型、macOS、安装目录权限、两个 commit／产物校验和、Library／Home 绑定／设置前后状态，以及是否出现管理员授权或再次放行。实际结果由 #107 承接，v0.1.0 → 带公钥基线版的手动安装不算自动更新通过。

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

`prepare-release.mjs` 对产物树中的符号链接、缺失/重复/额外架构产物、无效或不匹配签名、空 release notes 和任一版本漂移都 fail closed。三个输入产物会以 `Skill-Man-vX.Y.Z-aarch64` 为前缀复制到输出目录；上传使用返回的路径，避免 GitHub 规范化空格导致文件名漂移。复制不改变已签名的 archive 字节。其输出不含时间戳，因此同一组输入会生成相同的 metadata 与 checksums。

`latest.json` 只包含 `darwin-aarch64` 通道。顶层 `download_size` 是 `.app.tar.gz` 的精确字节数，供更新确认界面在开始下载前展示；`notes` 与 Draft Release notes 使用同一份生成结果。固定入口是：

```text
https://github.com/RookieZoe/skill-man/releases/latest/download/latest.json
```

GitHub 的 `latest` 不包含 Draft 或 Prerelease。因此需要接入 updater 的 0.x 签名测试版必须发布为普通 Release，不能勾选 “This is a pre-release”；社区正式版使用同一 Latest 通道。

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
