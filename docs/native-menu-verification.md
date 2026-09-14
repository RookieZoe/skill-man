# 原生菜单：实现与验收（#113 / #112）

基础范围来自 [#113](https://github.com/RookieZoe/skill-man/issues/113) 和 [#114 resolution](https://github.com/RookieZoe/skill-man/issues/114#issuecomment-5658886613)。2026-09-14 用户在实现中确认以下调整，优先于原六项菜单描述：

- 关于使用 macOS About panel，显示应用 logo、当前运行版本、作者和项目地址。
- App Update 使用系统原生窗口，独立完成检查、下载确认和安装确认；发布说明为简洁文本。
- 检查更新下方增加问题反馈，打开仓库的 `issues/new/choose` 页面。
- 启动与重新打开时将主窗口置前。
- 检查更新、问题反馈不带省略号；About 条目只显示名称，版本移入 About panel，使系统菜单按较短内容自动收窄。

菜单现有七项操作：打开 Skill Man、关于 Skill Man、语言、配色、检查更新、问题反馈、退出 Skill Man。保留三处分隔线。浏览菜单不查询 Catalog、不执行 Rescan、不检查网络、不更改 Home。

## App-level 状态与更新

Locale 继续使用 ADR-0011 的 LocaleService。设置写入与 snapshot 发布按同一锁排序；React 按 generation 接收状态，订阅完成后读取初值，迟到查询不能覆盖较新选择。

AppearanceApi 提供 `get_appearance_snapshot`、`set_appearance_selection`、`migrate_appearance_legacy` 和 `appearance://changed`。system/light/dark 保存在 Home 外的 `skill-man-state/appearance.json`。落盘成功后才改变 snapshot；原生外观和菜单在主线程读取最新 authority。

Legacy 配色在主窗口首次挂载时从 `localStorage["skill-man.appearance"]` 提交。原生值存在时优先，包括用户从菜单明确选择的 system。只有迁移成功才清理 localStorage；读取或迁移失败保留旧值供下次重试。主窗口挂载前原生无法读取 Webview localStorage，此时尚未迁移的菜单使用 system。浏览菜单不会为了迁移打开主窗口。

NativeAppUpdate 统一接收托盘及设置页的手动检查与自动检查，复用 AppUpdateApi 和签名更新 adapter。无需主 Webview 的接收端，无需打开 Library。手动请求不受自动检查开关或冷却期限制；Home 不可写或冷却记录失败，不丢弃已经获得的手动检查结果。

原生进度窗使用 AppKit NSWindow、NSTextField 和 NSProgressIndicator，所有 Objective-C 对象只在主线程持有；检查结果和确认使用独立的 NSAlert app-modal 对话框，不依附隐藏的主窗口或进度窗。关闭进度窗取消当前意图；下载中的关闭也中止传输。取消之后的新点击进入新检查，取消之前的重复点击随本次操作一起取消。自动检查期间手动点击会提升为可见进度。进度窗标题和正文跟随 locale event 更新。

发现更新后先确认下载；签名校验成功后另行确认安装与重启。失败或不支持 App Update 时提供正式 Releases 的手动下载入口。元数据检查期限为 30 秒，归档下载为 10 分钟。原生发布说明展示最多 8 行、420 字符的纯文本摘要（截断时附省略号）。浏览器预览保留 React 展示，不承接原生菜单请求。

## 自动验证

公开边界覆盖真实临时目录持久化重开、legacy 优先级、保存失败保持状态、重复选择、迟到 snapshot、七项菜单顺序与分隔线、native/session 合并及取消后重试、HomeUnavailable 主动检查及冷却写入失败。AppUpdateApi 行为测试保留下载、签名失败、取消、独立安装确认等状态约束。React 测试验证原生路由不创建 Web 更新弹层，以及既有浏览器展示回归。

2026-09-14 最终 `npm run ci:local`（Apple Silicon）通过，退出码 0：格式、locales/capabilities、lint、TypeScript、Rust clippy、Rust 全套测试、前端 41 个文件 / 310 项测试及原生 aarch64 构建均通过。本地运行日志：`/tmp/skillman-113-native-modal-ci.log`。不依赖 GitHub Actions。

## 原生证据与验收边界

测试环境：macOS 26.6.2 (25G83)，arm64；本地构建的 Skill Man 0.1.3 `.app`，未创建发行版。

本轮已通过真实 UI 验证 About panel 显示 logo、版本、作者 RookieZoe 和项目地址。本地截图位于 `.scratch/native-menu-113/native-about-final.png`，不作为发布宣传截图。

新的原生更新入口已通过设置页的真实手动检查验证：检查结果以独立 macOS 系统对话框显示“Skill Man 已是最新版本”，点击“完成”回到原来的主窗口；截图为 `.scratch/native-menu-113/native-update-current-final.png`。检查进度窗由实际运行路径创建；其关闭与重复请求次序另由 session 回归覆盖。未在本轮安装新版本。

最初使用 Tauri/rfd 异步消息对话框时，插件自动选择已有窗口作 sheet parent，隐藏进度窗后可能看不到结果。现改为主线程 `NSAlert.runModal`，真实检查结果已重新验证。

前一轮同任务原生冒烟已验证语言/配色切换不关闭设置、English/Dark 重启持久化、关闭主窗口后驻留、重新打开保留选择、⌘Q 退出。验证后已恢复跟随系统语言、跟随系统配色和自动检查开启。旧 `update-checking.png` / `update-current.png` 来自更改为系统原生对话框之前的实现，不作为新原生更新窗口证据。

#112 继续承担完整组合验收，包括 VoiceOver、多屏/缩放、屏幕边缘、托盘实际菜单宽度、Dock 两种偏好、故障注入与正式更新通道证据。现有 CUA 原生 app surface 未暴露状态栏图标，不能将实际托盘左/右键、子菜单和屏幕边缘测试记为通过。真实下载/安装不通过伪造正式发布来验证。

## Standards review

最终复核通过，无剩余可操作问题。NSAlert 在主线程创建、运行和释放；取消区分关闭前后的请求；临时诊断代码已移除。

## Spec review

最终复核通过，无剩余可操作问题。自动检查被手动请求提升为可见进度；取消后新的请求保留；下载和安装仍分别确认。原生验收范围以以上实际证据为准。

## About 文字修整（2026-09-14）

用户截图中，作者和完整 URL 使用默认大字号且左对齐，与居中的应用名称/版本不协调，网址折为两行，版本重复显示。现在两个菜单入口共用标准 About panel 的 attributed credits：11 pt 系统字体、次要文字颜色、居中段落、5 pt 段后间距；网址显示为 `github.com/RookieZoe/skill-man`，链接目标保留 HTTPS 完整地址。显式清空 build version 字段，运行版本只显示一次。Logo 继续取运行 App 的原生图标。

已通过原生 App 菜单打开新面板并截图验证：网址从 2 行变为 1 行，版本从 2 次变为 1 次。原截图为 578×464 px，CUA 截图为 284×209 px，两者采集比例不同，不直接用像素差计算字号变化。新截图为 `.scratch/native-menu-113/native-about-typography.jpg`。AX 可访问树识别项目地址为 link；未将浏览器导航 readback 记为通过。

checked 1 / changed 2 / left 1：App 菜单入口已实测，App 菜单和托盘两个入口已改用同一实现，托盘图标入口仍待原生复查。

本轮 `npm run ci:local` 通过（退出码 0，日志 `/tmp/skillman-about-polish-ci.log`），`.app` 构建成功。Standards 与 Spec 两路静态复核均无剩余问题。

## 原生 Logo 修复（2026-09-14）

用户随后截图显示 About 与更新结果为文件夹图标。前轮仅验证打包 App，依赖运行 App 的默认图标，未覆盖开发/未打包运行。现将现有 `icons/icon.icns` 嵌入可执行文件，显式设置 About 的 `ApplicationIcon` 和共享 `NSAlert.icon`，不依赖 App Bundle；原文字版式保持不变。所有更新结果、错误、下载/安装确认以及复用该弹窗的菜单错误提示均覆盖。

新增 `native_branding` 原生回归程序，在真正的进程主线程创建 Cocoa 对象，比较 About 选项及 NSAlert 中的实际图像数据与嵌入 Logo。修复前两处均失败（`About=false, alert=false`），修复后通过；日志为 `/tmp/skillman-branding-red.log` 和 `/tmp/skillman-branding-green.log`。CUA 无法将未打包 `target/debug/skill-man` 识别为 App，因此本轮未取得该开发进程的新窗口截图，不以旧打包截图替代此项证据。Standards 与 Spec 复核均无可操作问题。

本轮 `npm run ci:local` 通过（退出码 0，日志 `/tmp/skillman-branding-ci.log`），包括上述原生回归、完整测试及 aarch64 原生构建。


## 规格同步与原生冒烟补测（2026-09-14）

已同步 #108、#113、#112 及 #114 的 canonical resolution：七项菜单、问题反馈、About 内容与排版、独立原生 App Update、启动/重新打开置前及无省略号；保留原验收要求，不因自动化限制删减门禁。

测试代码为 main `4b0bcb2`，本轮重新构建 Skill Man 0.1.3 App Bundle 成功（日志 `/tmp/skillman-native-smoke-build.log`）。机器连接 DELL U2720QM（主屏，60 Hz）与 M27P20P（144 Hz），两者报告的逻辑分辨率均为 2560×1440，渲染像素 5120×2880，非镜像。显示器清单仅证明环境，不能替代跨屏交互证据。

| 项目 | 本轮结果 | 证据与限制 |
| --- | --- | --- |
| Dock 偏好开启 | 部分通过 | 从 off 切至 on；⌘Q 后确认测试进程退出，重启设置仍为 on；关闭主窗口后进程 65956 继续驻留，CUA 重新访问 App 可恢复同一设置页。未验证 Dock 图标本身与通过 Dock 点击重新打开。 |
| Dock 偏好关闭 | 部分通过 | 从 on 切回 off；⌘Q 后进程退出，重启仍为 off；关闭主窗口后进程 65989 继续驻留，CUA 重新访问 App 可恢复设置页。最终保持原 off 偏好，截图 `.scratch/native-menu-113/dock-off-restored.jpg`。未验证 Dock 图标消失。 |
| 托盘左/右键 | 未验证 | App AX 仅暴露主窗口及 App/Edit/Window/Help 菜单，不含状态栏入口；CUA 访问 SystemUIServer 超时，键盘尝试未产生状态变化。 |
| 托盘子菜单、Esc、外部点击 | 未验证 | 缺少可操作的托盘入口，不能用应用菜单或源码推断通过。 |
| 多屏与屏幕边缘菜单 | 未验证 | 双屏已连接；工具未提供可操作的两屏状态栏或全桌面坐标视图，未完成跨屏触发、子菜单可达性与边缘定位。 |

CUA 访问 Dock 同样超时。已请求用户协助实测工具无法覆盖的托盘及双屏项目，尚未收到结果。#113 / #112 继续保持打开，组合验收复选框不勾选；本轮没有发现新的产品缺陷，也没有足够证据认定上述未验证项通过。
