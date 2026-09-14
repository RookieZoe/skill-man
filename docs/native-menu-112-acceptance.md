# #112 原生菜单组合验收

## 范围与证据来源

规格为 #108 / #114 的七项菜单决策，实施 #113 已关闭。基线 `7c4400d`（产品实现 `4b0bcb2`），Skill Man 0.1.3，macOS 26.6.2 / Apple Silicon。本轮补充验收测试，不修改产品行为，不发版。既有原生截图、构建及用户手工确认详见 [原生菜单验证历史](native-menu-verification.md)。

证据分别标记为真实原生对象测试、隔离行为测试、真实 App 操作、用户手工确认、静态路径检查。后两者不能替代未执行的自动化；内存或 API 测试不作为 VoiceOver、鼠标、屏幕定位或实际网络观测的证据。

## 本轮可重复验证

- `cargo test --manifest-path src-tauri/Cargo.toml --test native_menu`：在进程主线程构建真实 Tauri/AppKit 菜单，覆盖三种语言选择 × 三种配色选择共九组。断言七项操作及分隔位置、更新/反馈文案、子菜单各三项且仅当前选择勾选。系统语言测试源固定为 en-US，另覆盖显式 en/zh-Hans。上下文无窗口、Home、Catalog、更新服务或生产插件；菜单构建前后两份临时偏好文件字节不变。该测试验证构建/读取菜单，不执行弹出菜单，不证明浏览菜单时所有进程 I/O 为零。初次日志 `/tmp/skillman-112-native-menu.log`；补强退出文案、分隔项及子菜单顺序断言后复测日志 `/tmp/skillman-112-native-menu-final.log`。
- `cargo test --manifest-path src-tauri/Cargo.toml --lib appearance_api`：五项真实临时 App-state 测试，验证持久化重开、合法/未知 legacy、原生选择优先、临时写路径故障、失败后重试、重复及并发选择。未改动用户偏好目录。日志 `/tmp/skillman-112-appearance.log`。
- `cargo test --manifest-path src-tauri/Cargo.toml --lib native_app_update`：四项 session 测试，覆盖重复请求合并、自动检查期间手动意图、关闭后的新重试和关闭前的重复点击取消。日志 `/tmp/skillman-112-session.log`。
- `cargo test --manifest-path src-tauri/Cargo.toml --test app_update_flow`：十三项隔离 API 测试，覆盖主动检查、冷却、网络失败、下载取消、签名未验证不可安装、下载/安装独立阶段、HomeUnavailable 及 Home 偏好写入失败。使用受控更新 adapter，不冒充正式网络下载/安装。日志 `/tmp/skillman-112-update.log`。
- `npx vitest run src/features/appearance/AppearanceProvider.test.tsx src/features/locale/LocaleProvider.test.tsx src/app/AppUpdateProvider.test.tsx`：三个文件十五项通过，验证失败保持状态及 legacy、迟到 snapshot 不覆盖新选择、输入不丢、原生更新不依赖 Web 弹层。日志 `/tmp/skillman-112-focused-web.log`。`npm run typecheck` 通过。

## 验收矩阵

编号对应 #112 原 Acceptance criteria 的顺序。复合项只有所有子条件都有证据时才整体勾选。

| 编号 | 结果 | 已有证据与剩余范围 |
| --- | --- | --- |
| 1 菜单结构与反馈 | 部分通过 | 真实原生对象九组菜单通过；用户确认托盘交互。创建 issue 页面实际浏览器导航尚缺 readback。 |
| 2 原生交互与辅助功能 | 部分通过 | 用户确认左/右键、子菜单、Esc、外部点击；方向键/Enter、VoiceOver 未验证。 |
| 3 Dock 与生命周期 | 部分通过 | 两种偏好、驻留、重启保持及退出已有 App/用户证据；最小化恢复及启动置前全部组合未单独记录。 |
| 4 About | 部分通过 | 真实 About 内容/排版截图、未打包 Cocoa 图标测试；关于打开不唤起主窗口/不联网尚缺独立观察。 |
| 5 Locale | 部分通过 | 隔离 authority/provider 与真实菜单选择通过，既有双语重启证据；运行时双向同步全部 UI 组合未齐。 |
| 6 Appearance | 部分通过 | 三选项菜单、临时目录持久化及原生重启证据；系统配色实际改变后的响应未验证。 |
| 7 隔离 legacy | 通过 | 真临时 App-state 五项测试及 Web legacy 清理/失败保留测试。 |
| 8 保存失败与 Home 状态 | 部分通过 | API 保存失败保留状态及 HomeUnavailable 更新通过；真实 CheckMenuItem 自动勾选还原、原生错误提示和各 Home 状态组合未完整验证。 |
| 9 原生 App Update | 部分通过 | 既有真实最新版本结果、Logo、隔离 API/session；各窗口状态的失败/不支持原生呈现未完整验证。 |
| 10 正式检查与受控失败 | 通过 | #113 已有真实正式通道“已是最新版本”检查及原生截图；本轮十三项 API 与四项 session 隔离验证，不执行升级/回滚。 |
| 11 多屏/缩放/边缘 | 部分通过 | 用户确认双屏冒烟；不同缩放组合未验证，未据环境清单推断通过。 |
| 12 浏览菜单无副作用 | 部分通过 | 新增真实菜单构建没有 Home/Catalog/更新服务且偏好不变；静态路径没有浏览回调。实际弹出/浏览期间的日志或调用观测仍缺，不能整体勾选。 |
| 13 验证文档 | 通过 | 本文与历史文档区分环境、代码版本、实际结果、截图与未验证项目。 |
| 14 缺陷复测与最终门禁 | 检查通过，验收未齐 | 本轮未发现产品缺陷。完整本地 CI 通过，新增测试的复核发现已修正并复测；Standards 与 Spec 无剩余发现。整体原生组合验收仍受以下限制。 |

## 自动化边界与结论

CUA 已在此前实测中无法访问 SystemUIServer / Dock，App AX 没有托盘入口，工具不提供全桌面跨屏或 VoiceOver 控制。这是当前验证能力限制，不是产品测试失败。用户询问这些项目能否自动完成时，已说明上述限制；没有将该询问当作通过确认。

#112 保持打开。剩余原生辅助功能、键盘、实际系统配色、不同缩放、浏览器反馈导航、窗口/Home 组合和菜单浏览调用观测需要补证。可以在完整桌面操作/观察能力可用时自动执行部分项目；当前不得以模拟输入、源码或原生对象构建替代。父规格 #108 不自动关闭。


## 最终验证与复核

`npm run ci:local` 退出码 0（`/tmp/skillman-112-ci.log`），包含前端 41 文件 / 310 项、Rust 全套测试及 aarch64 原生构建。复核指出初版测试仅验证 Predefined 类型，未区分 Quit 与分隔项；最终补充退出双语文案、空分隔项及子菜单 ID 顺序断言，原生测试再次通过。最终测试文件的格式与定向 clippy 另行检查；两路复核均无剩余可操作问题。未重复运行与断言补强无关的全套测试。
