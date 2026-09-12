# 社区 App Update 原生验收（#107）

2026-09-12，通过 CUA 操作 UTM 中的真实 macOS/Tauri 窗口，完成正式版 **v0.1.2 → v0.1.3 → v0.1.2** 升级、重启和 Manual App Rollback。持久技能样本、Home 绑定和设置保持可用。另用隔离的原生验收构建验证损坏更新被拒绝，并以原包下载成功作为对照。

本文是维护者操作与终端输出的整理记录，不是自动化测试输出或附带截图的报告。结论限于以下环境，不外推所有 macOS 13+、Intel、受管理设备或所有 Library 数据组合。GitHub issue 的关闭另行处理。

## 环境与正式产物

- UTM Apple 虚拟化 macOS VM，`VirtualMac2,1`、`arm64`、macOS **26.6.2 / 25G83**。
- 新建标准账号 `skillman107`，uid 502；`/Applications` 为 `root:admin`、0775。安装及回滚使用 VM 管理员认证。
- 首次启动前，账号中没有 Skill Man 状态目录和 Home。未操作宿主机的正式 App 或真实 Home。
- 两版均为 Ad-hoc App 签名、真实 Tauri updater 公钥与更新包签名。没有关闭 Gatekeeper，也没有通过删除 quarantine 获得通过结果。

| 版本   | commit                                     | 真实构建运行                                                                   | 官方 Release                                                         |
| ------ | ------------------------------------------ | ------------------------------------------------------------------------------ | -------------------------------------------------------------------- |
| v0.1.2 | `0095c1988334331b9f3b447e253d8ffc2befad67` | [34378404629](https://github.com/RookieZoe/skill-man/actions/runs/34378404629) | [v0.1.2](https://github.com/RookieZoe/skill-man/releases/tag/v0.1.2) |
| v0.1.3 | `2a2fb249477da14f049a2fb5da1d2d9f3f9833dc` | [34384355160](https://github.com/RookieZoe/skill-man/actions/runs/34384355160) | [v0.1.3](https://github.com/RookieZoe/skill-man/releases/tag/v0.1.3) |

| 产物                                                                                                                          | SHA256                                                             |
| ----------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| [v0.1.2 DMG](https://github.com/RookieZoe/skill-man/releases/download/v0.1.2/Skill-Man-v0.1.2-aarch64.dmg)                    | `0c72f6dfe3d80d2115f0c96e066103df521293a50bd5b97bedbd84d03c6085a5` |
| [v0.1.3 DMG](https://github.com/RookieZoe/skill-man/releases/download/v0.1.3/Skill-Man-v0.1.3-aarch64.dmg)                    | `255cb90628193484418d2b229b7615e90d9fa3d8edffc93ddaf50dd26ffba9c6` |
| [v0.1.3 updater archive](https://github.com/RookieZoe/skill-man/releases/download/v0.1.3/Skill-Man-v0.1.3-aarch64.app.tar.gz) | `e90dfa57b10253ee7e32936d453d623dfc234a032170e52c74cf9faa1515c715` |

v0.1.3 发布后，匿名 `verify-release.mjs --public` 返回 `assets: 5`、`signatureVerified: true`、`anonymous: true`。正式 Latest 为 v0.1.3；本次没有发布测试版 v0.1.4。产物审查另已确认 DMG 与 updater App 内容一致、arm64、版本、公钥和签名正确。产品变更的完整 `npm run ci:local` 已通过；本次新增验收文档未修改产品代码。

## 干净账号首装与确认边界

1. 在 VM Safari 匿名打开官方 v0.1.2 Release，下载 DMG。终端 SHA256 与上表一致，`xattr` 显示 `0083`、`Safari` quarantine 来源。
2. Finder 安装至 `/Applications`，需要管理员认证。首次启动遭 Gatekeeper 拦截，按发布说明进入“系统设置 → 隐私与安全性 → 仍要打开”，管理员认证后启动成功。
3. 初始化默认 Home：`/Users/skillman107/Library/Application Support/skill-man`。初始 Library 为 0；选择简体中文、深色；登录时启动关闭，Dock 显示、应用更新检查和技能更新检查开启。
4. v0.1.2 经真实 GitHub Latest 发现 v0.1.3，显示当前 0.1.2、目标 0.1.3、发布说明和 9.7 MB 下载大小。“暂不”返回可用 Library；另一次下载中的“取消下载”也返回 Library，没有安装或重启。
5. 完整下载后显示“下载已验证”，仍等待“安装并重新启动”二次确认。点击后才出现管理员认证并执行安装重启。

正式 GitHub 流程的首次确认前未下载结论来自 UI 与真实调用流程观察，没有采集该阶段的网络抓包。下文隔离原生构建补充了服务端请求记录：确认前只有清单请求，确认后才出现包请求。没有使用 browser fixture 或 mock 作为原生通过依据。

## 持久技能的升级与回滚

第一次升级发生于空 Library，不用它证明已有技能的保持。后续使用真实 Local Source，持久位置为 `/Users/skillman107/Documents/acceptance107persistent/SKILL.md`，通过原生 Import 纳管。内容为：

```text
---
name: acceptance107persistent
description: Native update persistence check
---
# Acceptance 107
Preserve this content across upgrade and rollback.
```

样本以 LF 换行并保留末尾换行，SHA256 为 `5922d92c235f558b9f75ded8fa8ce08da0861a40b496bade8e45b7c2d1a56d0e`。

升级前将 Home Binding、locale 文件和样本 SHA256 清单保存到 VM 的 `Documents/evidence107`。原生 Library 显示技能名称、描述、标题及正文。未配置 Agent 或 Activation，不声称覆盖其所有组合。

| 检查                                                   | 升级后                                                 | 回滚后                               |
| ------------------------------------------------------ | ------------------------------------------------------ | ------------------------------------ |
| 安装版本，`defaults read … CFBundleShortVersionString` | 0.1.3                                                  | 0.1.2                                |
| 真实进程路径，`pgrep -fl skill-man`                    | `/Applications/Skill Man.app/Contents/MacOS/skill-man` | 同左                                 |
| Home Binding 与升级前副本 `diff`                       | 无差异                                                 | 无差异                               |
| locale 与升级前副本 `diff`                             | 无差异                                                 | 无差异                               |
| 持久技能 `shasum -a 256 -c`                            | OK                                                     | OK                                   |
| Library 技能正文                                       | 正常显示                                               | 正常显示                             |
| 简体中文、深色和原有开关                               | 保持                                                   | 保持                                 |
| 管理员认证                                             | 安装时出现                                             | Finder 替换及“仍要打开”时出现        |
| Gatekeeper                                             | 自动重启后未再次出现                                   | 首次启动再次出现，按说明放行后可启动 |

Manual App Rollback 使用同一份已由 Safari 下载并核对的官方 v0.1.2 DMG，通过 Finder“拷贝 / 粘贴项目 → 替换”覆盖较新 App；没有恢复 Home 快照、数据库降级或手动改写业务数据。回退后再次检查更新，仍显示当前 0.1.2、可更新 0.1.3、9.7 MB，符合预期。

v0.1.3 原生设置中更新说明和手动下载链接已与设置正文保持内缩；回滚到 v0.1.2 后旧版贴边样式重新出现，与本次样式修复范围相符。

## 损坏更新的原生拒绝与对照

使用同一 v0.1.3 commit 本地构建独立 `Skill Man Acceptance.app`，identifier 为 `io.github.rookiezoe.skillman.acceptance107`。仅覆盖产品名称、identifier、updater 测试地址及本地 HTTP transport 开关；正式公钥、产品代码和 Tauri 下载验签实现未变。该构建只在 VM 中运行，不替换 `/Applications` 的官方 App，不用于证明官方首装或正式升级链。

测试服务绑定宿主机 VM 网段 `192.168.64.1:18707`；临时构建允许该本地 HTTP endpoint，签名校验保留。正式构建仍使用原 HTTPS GitHub endpoint。测试服务结束后已停止。

复现构建配置（仅用于隔离验收）：

```json
{
  "productName": "Skill Man Acceptance",
  "identifier": "io.github.rookiezoe.skillman.acceptance107",
  "plugins": {
    "updater": {
      "endpoints": ["http://192.168.64.1:18707/latest.json"],
      "dangerousInsecureTransportProtocol": true
    }
  }
}
```

构建命令为 `npm run tauri build -- --target aarch64-apple-darwin --bundles app --config <临时配置>`，退出码 0。通过本地服务传入 VM 的验收 App ZIP SHA256 为 `92b491404643d63ab941b19e79a3488d7a0be8d642b38e0226981cb621da0d1d`，VM 下载后匹配。该测试构建通过 curl 传入，不计作 Safari quarantine 首装证据。

受控清单从正式 v0.1.3 `latest.json` 派生：version 改为仅测试用 0.1.4，notes 标明不是公开发布；包地址改成本地服务。保留原签名和大小。把正式 archive 的中间字节按位异或 `1`，得到损坏包 SHA256：`7c1ae86a0933b6ec4c5996f125d5f5ee6f88a96a1bff5b481c3053847b593579`。

观察到的服务端请求顺序（服务端本地时间，2026-09-12）：

| 时间     | 请求                             | 当时操作                                  |
| -------- | -------------------------------- | ----------------------------------------- |
| 17:27:01 | `GET /latest.json`，200          | 检查更新，等待首次确认；无包请求          |
| 17:28:18 | `GET /corrupted.app.tar.gz`，200 | 点击下载损坏包                            |
| 17:31:47 | `GET /latest.json`，200          | 正常包对照检查                            |
| 17:32:12 | `GET /corrupted.app.tar.gz`，200 | 同一 URL 此时提供原始未损坏包，仅验证下载 |

损坏包下载后，原生界面显示“应用更新失败，请稍后重试”，保留下载重试和手动下载说明，没有进入“安装并重新启动”确认，没有安装或重启。关闭提示后技能正文仍可读取。Home/locale 对照无差异，技能哈希 OK，官方 App 和验收 App 二进制均与失败前 SHA256 一致。

为排除网络或清单配置问题，保留损坏包证据后，仅把同一包 URL 的内容换回原始正式 archive，清单、签名和大小不变。原生下载成功进入“下载已验证”二次确认。选择“稍后”，未安装测试目标；退出验收 App，重新打开官方基线。该对照与真实下载路径共同支持损坏包在验签阶段被拒绝；UI 错误文案是通用失败文案，未采集底层异常原文，不声称 UI 展示了签名诊断。

## 中断与失败记录

- 早期样本放在 `/tmp`。VM 在操作过程中意外停止，原因未确定；重新启动后该目录及临时基线文件不存在，链接技能详情不可读。因此这一尝试不计作数据保持通过。恢复临时样本也不计作保持证据。上述正式持久链改用 Documents，重新建立基线并完整执行。
- 早期回滚用终端 `ditto` 复制 App，Gatekeeper 放行后进程仍位于只读 App Translocation 挂载。下载验签成功，但安装失败且没有管理员提示。`pgrep` 与 `mount` 确认了只读路径；按发布说明重新用 Finder 安装后进程回到 `/Applications`，同一更新成功。此失败不代替损坏签名验收，也不支持推荐命令行复制作为安装方式。
- 手动下载兜底与 Gatekeeper 说明在正式版及失败界面保留。已实际按“仍要打开”完成首装和回滚启动；未禁用系统安全检查。

## 证据保留与检查

VM 中 `Documents/evidence107` 保留 `home-before.json`、`locale-before.json`、`sample-before.sha256` 和 `binaries-before-negative.sha256`；持久技能留在 Documents。官方 v0.1.2 已重新打开并正常显示技能正文和深色配色；VM 暂未删除。

宿主机本次会话的临时记录为 `/tmp/skill-man-107-native-progress.md`；受控产物目录由 `/tmp/skillman107-negative-path` 指向，保留配置、清单、原始来源说明、损坏包副本及验收 ZIP。这些临时文件可能被系统清理；本文保留关键结果、版本、哈希和复现方法，不依赖临时路径作为永久证据链接。

本记录覆盖 #107 的真实升级、重启、兼容回滚和受控损坏包拒绝场景。正式链网络边界采用 UI 观察，包请求时序的额外证据来自隔离原生构建；两者没有混写为同一次运行。产品发布仍不具备 Apple Developer ID / notarization。
