# 社区版应用更新与真实升级验收

维护者于 2026-09-08 确认：没有 Apple Developer 凭据时，社区版可以发布为正式 Latest Release，并使用独立签名的 Tauri 更新包提供 App Update。Apple 的开发者身份与公证、更新包的来源验证承担不同职责；不再把 Apple 凭据作为社区更新通道的前置条件。

本决策仅取代 ADR-0006 中“社区版只能 Pre-release、不能为 Latest、禁止 updater 产物、updater 必须绑定 Developer ID 通道”的限制。Apple 签名通道自身的证书、公证和验收门禁继续有效；密钥保管、强制验签、Draft 审阅与最小发布权限的约束保持有效。

## 状态与承接

- 已实现：v0.1.0 为正式社区版，Ad-hoc 签名，当前仍通过 Release 页面手动下载更新。维护者已确认干净 macOS 环境的社区版安装、首次启动及手动放行路径通过。
- 通道实施与证据：[#106](https://github.com/RookieZoe/skill-man/issues/106) 承接社区 Tauri 密钥、签名构建、Draft 附件回读及公开 Latest 校验。代码就绪不等于真实 Actions 构建、维护者密钥备份和公开附件验收完成；各项证据以票据记录为准。
- 待原生验收：[#107](https://github.com/RookieZoe/skill-man/issues/107) 依赖 #106，验证两个真实版本间的升级、重启和手动回滚。
- 暂缓：[#105](https://github.com/RookieZoe/skill-man/issues/105) 以 needs-info 跟踪 Developer ID 签名、公证及 Apple 凭据，不阻塞上述社区通道。

#31 的剩余工作由这些票据承接；关闭原票不表示这些后续门禁已经通过。

## 更新通道与交互

复用 #29 已有的 App Update 状态机和 Tauri adapter。只接收正式 GitHub Release，使用单一 Latest 通道，排除 Draft 和 Pre-release。保留既有后台检查冷却和用户偏好；用户确认后下载，完整验签后再次确认安装并重启，不进行后台自动下载或强制更新。

App 可以保持 Ad-hoc 签名，更新包必须使用长期保管的 Tauri 密钥签名。公钥嵌入应用，私钥及密码仅用于受保护的发布环境，并由维护者离线备份；私钥丢失不得关闭验签。工作流必须先生成、校验完整产物并创建 Draft，审阅后才公开 metadata 和更新包。

现有 v0.1.0 未嵌入真实公钥，不能自举这条更新链。用户需要先手动安装带公钥的基线版，再更新到严格递增的目标版本。

## Gatekeeper 与回滚边界

无 Apple 凭据时不承诺更新后免再次放行。允许 macOS 再次要求“仍要打开”，但应用与发布说明必须预先提示并提供操作说明、手动下载兜底；验收记录实际行为，不以关闭 Gatekeeper 或自动清除 quarantine 作为产品方案。管理员授权提示与 Gatekeeper 放行分别记录。

Manual App Rollback 限于两个数据格式兼容的真实社区版本之间，手动从官方 Release 下载并覆盖安装上一版。验收需确认升级和回退后的应用版本、Library、Home 绑定及设置可用；自动失败回滚、数据库降级和 Home 快照恢复不在范围内。

两个版本必须都包含真实更新公钥，使用真实构建和发布链完成验证。首装通过、静态测试和单次 CI 成功不能代替升级验收；记录 macOS、机型、目录权限及产物信息，结论仅覆盖实际测试环境。
