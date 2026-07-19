# Skill Man macOS 技术栈选型调研:SwiftUI vs Tauri v2 vs Electron

- 调研日期:2026-07-20(文中所有版本号、价格、体积、stars 数字均为当日核实)
- 调研方法:仅以一手资料为依据(官方文档、官方 GitHub 仓库/Release、Apple 官方文档 JSON API、官方规范),不采信二手博客结论
- 项目背景:macOS 桌面应用「Skill Man」,统一管理本机 AI agent 的 skills;核心交互为文件系统操作(读目录、解析 SKILL.md、创建/删除符号链接)、主窗口(列表+详情+开关)+ 菜单栏快捷入口;**不走 App Store,直接分发**;维护者会 TypeScript/JavaScript 与 Rust/Go,无 macOS 开发经验但愿意学 Swift

---

## TL;DR

**推荐排序:① Tauri v2 → ② SwiftUI → ③ Electron**

一句话理由:维护者同时具备 TypeScript 与 Rust 能力,Tauri v2 恰好把两者都用上(Web 前端 + Rust 后端),同时具备官方 tray、官方强制签名 updater、CLI 内置公证、小体积(官方宣称可低至 600KB);SwiftUI 原生体验最佳且 `MenuBarExtra` 就是为菜单栏工具而生,但需要从零学 Swift/Xcode 且 TS/Rust 技能闲置;Electron 技术栈匹配度也高,但运行时体积大两个数量级(darwin-arm64 压缩包约 116MB)、原生观感最弱,在本场景无不可替代的优势。

---

## 对比矩阵(A–G × 三栈)

| 维度 | SwiftUI(Swift 原生) | Tauri v2(Rust + Web) | Electron(Chromium + Node) |
|---|---|---|---|
| **A. 技术栈匹配** | 需新学 Swift + Xcode + SwiftUI 心智模型;TS/Rust 技能基本闲置 | **双技能命中**:前端用 TS,后端用 Rust;学习点是 Tauri 概念(command/IPC/WebView) | TS 全栈命中;学习点是主进程/渲染进程/preload 模型 |
| **B. 窗口+菜单栏** | `MenuBarExtra`(macOS 13+),Apple 官方 Scene,原生程度最高;`LSUIElement` 可隐藏 Dock 图标 | 官方 `TrayIconBuilder`/`@tauri-apps/api/tray`,`tray-icon` feature;tray 模式 + 手动控制窗口显隐 | 官方 `Tray` API,成熟稳定;macOS 建议 Template Image;窗口显隐自行管理 |
| **C. 文件系统/符号链接** | `FileManager.createSymbolicLink` / `destinationOfSymbolicLink`;FSEvents 官方 API 监听目录 | Rust `std::os::unix::fs::symlink`、`read_link`、`symlink_metadata`;官方 `fs` 插件(带 scope 权限);目录监听用 `notify` crate(v2 官方插件列表中已无 fs-watch) | Node `fs.symlink`(macOS 上 `type` 参数被忽略)、`fs.watch`(macOS 目录走 FSEvents,`recursive` 自 Node 18.13/19.1 起支持 macOS) |
| **C2. 沙盒限制(直接分发)** | 无需沙盒(App Sandbox 仅 Mac App Store 强制);公证需 Hardened Runtime | 同左,无需沙盒;Tauri `hardenedRuntime` 配置默认 `true`,直接满足公证前置 | 同左,无需沙盒;公证同样要求 Hardened Runtime |
| **D. 分发链路** | `notarytool` + Xcode 导出;自动更新用 Sparkle(de-facto 标准,Sparkle 2 支持沙盒 app,EdDSA 签名) | CLI 内置签名+公证(环境变量驱动);官方 updater 插件**强制签名**,静态 JSON / GitHub Releases 皆可托管 | 官方教程推荐 Forge(`@electron/osx-sign` + `@electron/notarize`);核心 `autoUpdater` 基于 Squirrel.Mac(macOS 必须签名);常用 `electron-updater`(electron-userland 社区项目) |
| **D2. Apple Developer Program 费用** | $99/年(三栈相同,直接分发必须) | $99/年 | $99/年 |
| **E. 体积/内存** | 最小(系统框架,无捆绑运行时);官方无统一数字 | 官方宣称使用系统 Web 渲染器时 app "can be little as 600KB";内存官方无数据 | 官方 Release 的 darwin-arm64 zip ≈116MB、x64 ≈118MB(仅运行时,压缩态);Chromium 多进程模型内存开销最高 |
| **F. 原生观感/平台集成** | **最强**:原生控件、快捷键、拖放、Finder 扩展都可直接做 | 中等:WKWebView 渲染,观感接近但细节(焦点、键盘、拖放)需打磨 | 较弱:Chromium 自绘 UI,需刻意模仿 macOS 风格 |
| **G. 生态/长期风险** | Apple 一方支持,无倒闭风险;风险是 API 随 macOS 年度演进 | GitHub 109k stars;v2 稳定版 2024-10-02 发布,经 Radically Open Security 独立审计;托管于 The Commons Conservancy;插件 API 可能在 minor 版本破坏 | GitHub 122k stars;工作组制治理,跟随 Chromium 快速发版(v43,Chromium 150);体量与历史最久,风险低 |

---

## 一、SwiftUI(Swift 原生)

### A. 技术栈匹配

- 需要学习 Swift 语言、Xcode 工程体系、SwiftUI 声明式 UI。当前 Swift 稳定版为 **6.3.3**([swift.org 首页](https://www.swift.org/),2026-07-20 核实)。
- 对"无 macOS 经验但愿意学 Swift"的维护者:Swift 语言本身对 Rust/TS 开发者友好(可选类型、协议、async/await),真正的成本在 Xcode 工作流、签名配置、SwiftUI 的状态管理与布局心智模型。
- TS 与 Rust 技能在本方案中基本闲置。

### B. 窗口 + 菜单栏双形态

- `MenuBarExtra` 是 Apple 官方 SwiftUI Scene:**"A scene that renders itself as a persistent control in the system menu bar"**,可用性经 Apple 文档 JSON 核实为 **macOS 13.0+**,macOS 独占([MenuBarExtra 文档](https://developer.apple.com/documentation/swiftui/menubarextra) / [文档 JSON](https://developer.apple.com/tutorials/data/documentation/swiftui/menubarextra.json),2026-07-20 核实)。
- `.menuBarExtraStyle(.window)` 以 popover 式浮窗渲染内容,官方推荐用于"more complex or data rich menu bar extras"——正好契合"菜单栏快捷入口 + 主窗口列表详情"的双形态。
- 纯菜单栏 app 可设 `LSUIElement = true` 隐藏 Dock 图标(同一文档)。
- 这是三栈中唯一"菜单栏形态"为平台一方 API 直接建模的方案,原生程度最高。

### C. 文件系统与符号链接

- 符号链接:`FileManager` 提供 `createSymbolicLink(at:withDestinationURL:)`("Creates a symbolic link at the specified URL that points to an item at the given URL")、`createSymbolicLink(atPath:withDestinationPath:)`、`destinationOfSymbolicLink(atPath:)`("Returns the path of the item pointed to by a symbolic link")([FileManager 文档 JSON](https://developer.apple.com/tutorials/data/documentation/foundation/filemanager.json),2026-07-20 核实)。判断"路径本身是 symlink"需用 `attributesOfItem(atPath:)` 检查类型,因为 `fileExists(atPath:)` 会跟随链接。
- 目录监听:FSEvents 是 Core Services 官方 API——"Get notifications when the contents of a directory hierarchy change",支持按 event ID 增量回放([File System Events 文档 JSON](https://developer.apple.com/tutorials/data/documentation/coreservices/file_system_events.json),2026-07-20 核实);也可用 `DispatchSource` 文件事件做轻量监听。
- 沙盒:Apple 官方文档明确 **"To distribute a macOS app through the Mac App Store, you must enable the App Sandbox capability."**([App Sandbox 文档 JSON](https://developer.apple.com/tutorials/data/documentation/security/app_sandbox.json),2026-07-20 核实)。反过来说,**Developer ID 直接分发不强制沙盒**,对 skills 目录的自由读写 + 符号链接操作没有沙盒 entitlement 障碍。

### D. 分发链路

- 证书:`Developer ID Application` 证书(直接分发),费用为 Apple Developer Program **$99/年**([Apple Developer Program](https://developer.apple.com/programs/),2026-07-20 核实;企业内部分发的 Enterprise Program 为 $299/年,与本场景无关)。
- 公证:Apple 官方定义——"The Apple notary service is an automated system that scans your software for malicious content, checks for code-signing issues, and returns the results to you quickly";自 macOS 10.15 起,2019-06-01 之后构建的 Developer ID 软件必须公证;`altool` 自 2023-11-01 起停止接受上传,现行工具为 `notarytool` + `stapler`,且 **Hardened Runtime 是公证前置条件**([Notarizing macOS software before distribution 文档 JSON](https://developer.apple.com/tutorials/data/documentation/security/notarizing_macos_software_before_distribution.json),2026-07-20 核实)。Xcode 导出流程(Archive → Distribute App → Developer ID → Upload)可一步完成。
- 自动更新:**Sparkle** 是 macOS 生态事实标准——"an easy-to-use software update framework for macOS applications",支持 EdDSA 签名与 Apple Code Signing 双重校验、delta 更新、"also supports sandboxed applications";Sparkle 2 支持 macOS 10.13+([sparkle-project.org](https://sparkle-project.org/),2026-07-20 核实)。当前版本 **2.9.4**(2026-07-03 发布),GitHub **9.4k stars**([sparkle-project/Sparkle](https://github.com/sparkle-project/Sparkle),2026-07-20 核实)。
- 不签名/不公证的后果:Gatekeeper 默认拦截;Apple 文档说明未公证软件的票据缺失会让 Gatekeeper 在首次启动时告警/拦截,未公证的隔离插件需在系统设置中手动批准(同上公证文档)。

### E. 体积与内存

- 无捆绑运行时(Swift/SwiftUI 框架随系统),三栈中体积与内存最小。Apple 官方未给出"Hello World"统一数字,此处不编造;结论是定性最强项。

### F. 原生观感与平台集成

- 原生控件、系统快捷键、拖放、Finder 集成(如 Finder Sync Extension、Services)都走一方 API,是"看起来像 macOS 应用"的最短路径。

### G. 社区生态与长期风险

- 一方技术,无项目倒闭风险;SwiftUI 自 2019 年起为 Apple 主推 UI 框架。
- 风险:API 可用性绑定 macOS 版本(如 `MenuBarExtra` 要求 macOS 13+);SwiftUI 复杂列表/文本场景历史上偶有需要回退 AppKit 的情况;锁定 Apple 平台(本项目只面向 macOS,影响可忽略)。

---

## 二、Tauri v2(Rust 后端 + Web 前端,macOS 上 WKWebView)

### A. 技术栈匹配

- **唯一同时命中维护者两项技能**:前端用熟悉的 TS/任意 Web 框架,后端用熟练的 Rust。学习成本集中在 Tauri 自身概念(command、IPC、capability/permission、WebView 窗口)。
- 当前稳定版 **tauri v2.11.5**(2026-07-01 发布,[tauri-apps/tauri Releases](https://github.com/tauri-apps/tauri),2026-07-20 核实)。

### B. 窗口 + 菜单栏双形态

- 官方 tray 支持:在 `Cargo.toml` 开启 `tauri = { version = "2.0.0", features = ["tray-icon"] }`;Rust 侧 `TrayIconBuilder`、JS 侧 `@tauri-apps/api/tray` 的 `TrayIcon.new(options)`;支持 Click/DoubleClick/Enter/Move/Leave 事件;`show_menu_on_left_click(false)`(JS:`menuOnLeftClick: false`)可关闭左键弹菜单,官方示例正是 macOS 菜单栏 app 的经典模式:左键点击 → 取消最小化、显示并聚焦主窗口([Tauri v2 System Tray 文档](https://v2.tauri.app/learn/system-tray/),2026-07-20 核实)。
- 与 SwiftUI 的差距:菜单栏入口是 NSStatusItem + 自行控制的普通窗口,而非 `MenuBarExtra` 那种 popover 一体的原生形态;要做到"popover 观感"需要自己调窗口样式(无边框、失焦关闭等)。

### C. 文件系统与符号链接

- Rust 标准库:`std::os::unix::fs::symlink(original, link)`("Creates a new symbolic link on the filesystem",Rust 1.1.0 起稳定)、`std::fs::read_link`(读链接目标,等效 `readlink(2)`)、`std::fs::symlink_metadata`(`lstat` 语义,不跟随链接,可判断"是否为 symlink")([std::os::unix::fs::symlink 文档](https://doc.rust-lang.org/std/os/unix/fs/fn.symlink.html),2026-07-20 核实)。前端侧有官方 `fs` 插件("Access the file system",支持 Windows/macOS/Linux)并带 scope 权限控制([plugins-workspace](https://github.com/tauri-apps/plugins-workspace),2026-07-20 核实)。
- 目录监听:v2 官方插件列表(30 个)中**已无 fs-watch 插件**(同上仓库列表核实);实践中在 Rust 侧用 `notify` crate(底层即 FSEvents)监听并通过 event 推到前端。这是与 v1 相比的一个生态变化点,需在工程上自行接线。
- 沙盒:与所有直接分发方案一样,不强制 App Sandbox(依据见 SwiftUI 一节的 Apple 文档引用);Tauri 的 capability/permission 是其自身安全模型,与 macOS 沙盒无关。

### D. 分发链路

- 签名:直接分发用 `Developer ID Application` 证书;签名身份可配置于 `tauri.conf.json > bundle > macOS > signingIdentity` 或 `APPLE_SIGNING_IDENTITY`;CI 用 `APPLE_CERTIFICATE`(base64 .p12)+ `APPLE_CERTIFICATE_PASSWORD`;`signingIdentity: "-"` 为 ad-hoc 签名,文档明确其"does not prevent MacOS from requiring users to whitelist the installation"([Tauri v2 macOS 签名文档](https://v2.tauri.app/distribute/sign/macos/),2026-07-20 核实)。
- 公证:**Tauri CLI 内置公证**,设置 `APPLE_API_ISSUER` / `APPLE_API_KEY`(_PATH)(App Store Connect API)或 `APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID` 后随打包自动完成,文档明确"Notarization is required when using a Developer ID Application certificate"(同上)。
- Hardened Runtime:`bundle > macOS > hardenedRuntime` **默认为 `true`**([Tauri 配置参考](https://v2.tauri.app/reference/config/),2026-07-20 核实)——默认即满足 Apple 公证前置条件,省心。
- 自动更新:官方 updater 插件;**签名强制不可关闭**("needs a signature to verify that the update is from a trusted source. This cannot be disabled"),`tauri signer generate` 生成密钥对,私钥经 `TAURI_SIGNING_PRIVATE_KEY` 在构建时签名,丢失私钥即无法向存量安装推送更新;分发可用静态 JSON(可托管于 GitHub Releases/S3)或动态服务器(204=无更新);**macOS 更新产物是整个 `.app` 的 `.tar.gz`,即整包替换,无官方 delta**([Tauri updater 插件文档](https://v2.tauri.app/plugin/updater/),2026-07-20 核实)。好在包体小,整包更新代价低。
- 费用:$99/年(见 SwiftUI 一节,三栈相同)。

### E. 体积与内存

- 官方首页:"By using the OS's native web renderer, the size of a Tauri app can be little as **600KB**."([tauri.app](https://tauri.app/),2026-07-20 核实);另有专门的体积优化文档(`opt-level`、`removeUnusedCommands` 等,该功能要求 `tauri@2.4+`)([App Size 文档](https://v2.tauri.app/concept/size/),2026-07-20 核实)。
- 内存:官方文档未给数字;定性上 WKWebView 为系统共享组件,优于每窗口一个 Chromium renderer 的方案。对比见 Electron 一节的官方 Release 体积数字。

### F. 原生观感与平台集成

- UI 由 WKWebView 渲染,可以做到"接近原生",但控件细节(焦点环、滚动弹性、文本渲染)与真原生有差距;拖放、快捷键由 Web 层 + Tauri API 组合实现;Finder 深度集成无现成路径(需自己写原生扩展或放弃)。

### G. 社区生态与长期风险

- GitHub **109k stars**;组织为 "The Tauri Programme within The Commons Conservancy",经费走 Open Collective;CrabNebula 为官方合作伙伴(2024 年投入 2,870+ 工时)([tauri-apps/tauri](https://github.com/tauri-apps/tauri) 与 [Tauri 2.0 公告](https://v2.tauri.app/blog/tauri-20/),2026-07-20 核实)。
- v2 稳定版发布于 **2024-10-02**;"The major changes and architecture of v2 was independently audited by Radically Open Security"(NLNet/NGI 资助)(同公告)。
- 风险:公告自己承认插件 API 可能在 minor 版本中破坏("hope to stabilize the core functionality... plugin APIs potentially breaking in minor versions");fs-watch 这类插件在 v2 中的缺位即是实例;WebView 差异带来的长尾 UI bug 需要耐心。

---

## 三、Electron(Chromium + Node.js)

### A. 技术栈匹配

- TS/JS 全栈命中,生态最熟;需掌握 Electron 特有的进程模型:**一个主进程(Node.js,持有全部 API)+ 每个 `BrowserWindow` 一个 renderer 进程(默认无 Node 访问,需 preload + `contextBridge` 做 IPC)+ `UtilityProcess`**([Process Model 文档](https://www.electronjs.org/docs/latest/tutorial/process-model),2026-07-20 核实)。
- 当前稳定版 **electron v43.1.1**(2026-07-14 发布,Chromium 150.0.7871.114)([electron/electron Releases](https://github.com/electron/electron/releases) 与 [GitHub API](https://api.github.com/repos/electron/electron/releases/latest),2026-07-20 核实)。

### B. 窗口 + 菜单栏双形态

- 官方 `Tray` 类:"Add icons and context menus to the system's notification area",主进程使用,事件/方法齐全;macOS 建议传入 **Template Image**(16x16@72dpi、32x32@2x 144dpi,打包时文件名须保留 `Template` 后缀否则反色失效);`setTitle`/`setPressedImage`/拖放事件为 macOS 独占;注意 `mouse-up` 在设置了 context menu 时不会触发(macOS 平台限制);Electron 内置类不可被子类化([Tray API 文档](https://www.electronjs.org/docs/latest/api/tray),2026-07-20 核实)。
- 与 Tauri 一样,菜单栏入口 = status item + 自控窗口,无 popover 一体形态;成熟度和文档完备度三者中最详尽。

### C. 文件系统与符号链接

- Node `fs.symlink(target, path[, type])`:`type` 仅 Windows 有效,macOS/Linux 上被忽略;相对 target 相对于链接所在目录解析([Node fs 文档](https://nodejs.org/api/fs.html),2026-07-20 核实)。
- `fs.watch`:macOS 上**文件走 kqueue、目录走 FSEvents**;`recursive: true` 自 Node v19.1.0 / v18.13.0 起支持 macOS(经 FSEvents 实现);注意 inode 语义——文件删除重建后旧 watcher 不会报新文件事件,filename 回调参数不保证非空(同上)。
- 沙盒:直接分发同样无需 App Sandbox(依据同前);Electron 的 contextIsolation 等是其自身安全模型。

### D. 分发链路

- 官方代码签名教程:流程为"先签名、再上传 Apple 公证";前置条件为加入 Apple Developer Program($99/年)+ 安装 Xcode + 生成证书;**官方首选工具为 Electron Forge**(内部使用 `@electron/packager`、`@electron/osx-sign`、`@electron/notarize`),非 Forge 用户可用 `@electron/packager` 传 `osxSign`/`osxNotarize` 配置;electron-builder 未出现在该教程的 macOS 部分([Code Signing 教程](https://www.electronjs.org/docs/latest/tutorial/code-signing),2026-07-20 核实)。
- 自动更新(核心 `autoUpdater`):macOS 基于 **Squirrel.Mac**,"Your application must be signed for automatic updates on macOS";Linux 无内置支持;服务器需满足 Squirrel.Mac 的 Server Support 要求([autoUpdater 文档](https://www.electronjs.org/docs/latest/api/auto-updater),2026-07-20 核实)。
- 实践中更常用 **electron-builder / electron-updater**:README 宣称 auto update "out of the box",发布目标含 GitHub Releases、Amazon S3、DigitalOcean Spaces;注意其归属 **electron-userland(社区组织)而非 Electron 核心**([electron-userland/electron-builder](https://github.com/electron-userland/electron-builder),2026-07-20 核实)。
- 公证工具链(@electron/notarize)底层同样走 Apple notary service,前置要求(Hardened Runtime、Developer ID、secure timestamp)与所有栈一致([Apple 公证文档 JSON](https://developer.apple.com/tutorials/data/documentation/security/notarizing_macos_software_before_distribution.json))。

### E. 体积与内存

- 体积有官方一手数据:electron **v43.1.1** 的 `electron-v43.1.1-darwin-arm64.zip` 为 122,054,683 字节(**≈116MB**,压缩态),`darwin-x64.zip` 为 123,952,132 字节(≈118MB)([GitHub Releases API](https://api.github.com/repos/electron/electron/releases/latest),2026-07-20 核实)。这仅是运行时;打成安装包后量级不变,比 Tauri 官方宣称的 600KB 大约两个数量级。
- 内存:官方 process-model 文档明确 Electron 继承 Chromium 多进程架构(每个窗口一个 renderer),稳定性换开销([Process Model](https://www.electronjs.org/docs/latest/tutorial/process-model));官方未给内存数字,定性为三栈最高。

### F. 原生观感与平台集成

- UI 由 Chromium 自绘,默认观感最不像 macOS 原生应用;需自行处理 vibrancy、控件风格、系统快捷键细节;Finder 集成无现成路径。

### G. 社区生态与长期风险

- GitHub **122k stars**(三栈最高);治理为工作组制(API、Releases、Security、Upgrades 等 9 个 Working Group + Administrative 组协调冲突),MIT 许可([electron/governance](https://github.com/electron/governance),2026-07-20 核实)。
- 跟随 Chromium 高频发版(当前 v43 / Chromium 150),安全更新快;生态与历史最久(VS Code、Slack 等先例),长期存续风险低。
- 风险:版本节奏快意味着升级维护成本常态化;包体与内存对用户可感知;核心 autoUpdater 能力朴素(Squirrel.Mac),好用的更新体验依赖社区 electron-updater。

---

## 风险与开放问题

1. **macOS 15+ 的绕过路径收窄**:未公证软件在 macOS 15 Sequoia 之后无法再通过"右键 → 打开"绕过 Gatekeeper,只能去 系统设置 → 隐私与安全性 手动放行(Apple 公证文档说明了 Gatekeeper 对未公证软件的拦截与手动批准路径)。**结论:直接分发必须做 Developer ID 签名 + 公证,$99/年不可省**,三栈相同。
2. **Tauri 插件生态的 minor 版本破坏**:Tauri 2.0 官方公告明示插件 API 可能在 minor 版本破坏;且 v2 官方插件列表中已不见 v1 时代的 fs-watch。目录监听需在 Rust 侧用 `notify` 自行接线并向 App 内推 event——工程量小但属于"官方文档之外的组装"。
3. **Tauri updater 无 delta**:macOS 更新产物是整包 `.app.tar.gz`,靠包体小对冲;若未来加入大量资源文件需重新评估。Electron 侧 Squirrel.Mac/electron-updater、Swift 侧 Sparkle 均有 delta 能力(Sparkle 官方列明 delta updates)。
4. **electron-updater 的归属**:**它是 electron-userland 社区项目,不是 Electron 核心模块**;Electron 官方签名教程 macOS 部分也未覆盖 electron-builder。选择 Electron 方案意味着关键分发环节押在社区项目上(尽管该项目是事实标准)。
5. **菜单栏 popover 形态**:Tauri/Electron 的 tray 都是 NSStatusItem + 自控窗口,做到 `MenuBarExtra`(.window 样式)那种系统 popover 观感需要额外打磨(无边框窗、失焦隐藏、箭头朝向)。若该体验被列为产品核心卖点,SwiftUI 权重应上调。
6. **开放问题(需在原型阶段验证)**:Tauri/Electron 在菜单栏模式下窗口失焦行为、拖放文件到窗口列表的可靠性;SwiftUI 列表在数百个 skill 条目下的性能;三者对 `~/.claude/skills` 等用户目录的读写在非沙盒下均无权限障碍,但 macOS 对 `~/Documents` 等目录有 TCC 提示,需实测目标目录是否在 TCC 管控范围内。

## 结论与推荐

**排序:第一 Tauri v2,第二 SwiftUI,第三 Electron。**

- **第一:Tauri v2**。对"TS + Rust 双熟练"的维护者是唯一全技能命中的方案;官方 tray、强制签名的 updater、CLI 内置公证、`hardenedRuntime` 默认开启,分发链路完整;体积(官方宣称可低至 600KB)与内存表现适合常驻菜单栏工具。主要代价是菜单栏 popover 观感需手工打磨、目录监听需自行接 `notify`。
- **第二:SwiftUI**。`MenuBarExtra`(macOS 13+)+ Sparkle 的组合在"菜单栏工具"这一形态上体验上限最高、最省心;但要求维护者投入学习 Swift/Xcode,且现有 TS/Rust 技能基本用不上。
- **第三:Electron**。技术匹配度不差、Tray/公证链路最老练,但 ≈116MB 压缩运行时 + Chromium 多进程内存开销对一个轻量常驻工具是硬伤;核心 autoUpdater 能力朴素,好用的更新依赖社区 electron-updater。在本场景没有前两者不可替代的优势。

**什么条件下结论会改变:**

1. 若"菜单栏 popover 的原生观感 + Finder 深度集成"上升为产品第一优先级 → **SwiftUI 升为第一**(MenuBarExtra 是一方 API,Sparkle 是最成熟更新方案)。
2. 若团队明确"只用 TypeScript、不碰 Rust/Swift"或需要最快复用 npm 生态(如直接复用现有 TS 版 skill 解析逻辑)→ **Electron 升为第一**,接受体积与内存代价。
3. 若未来需要支持 Windows/Linux(例如把 Skill Man 推广给用 Windows 的团队成员)→ **SwiftUI 出局**,Tauri 第一、Electron 第二。
4. 若目标用户的 macOS 版本可能低于 13 → SwiftUI 的 `MenuBarExtra` 不可用,只能回退 AppKit `NSStatusItem`,SwiftUI 优势下降。

---

### 附:主要一手来源清单(均于 2026-07-20 核实)

- Tauri v2:[System Tray](https://v2.tauri.app/learn/system-tray/) · [Updater 插件](https://v2.tauri.app/plugin/updater/) · [macOS 签名](https://v2.tauri.app/distribute/sign/macos/) · [配置参考](https://v2.tauri.app/reference/config/) · [App Size](https://v2.tauri.app/concept/size/) · [Tauri 2.0 公告](https://v2.tauri.app/blog/tauri-20/) · [tauri.app 首页](https://tauri.app/) · [tauri-apps/tauri](https://github.com/tauri-apps/tauri) · [plugins-workspace](https://github.com/tauri-apps/plugins-workspace)
- Electron:[Tray](https://www.electronjs.org/docs/latest/api/tray) · [autoUpdater](https://www.electronjs.org/docs/latest/api/auto-updater) · [Code Signing 教程](https://www.electronjs.org/docs/latest/tutorial/code-signing) · [Process Model](https://www.electronjs.org/docs/latest/tutorial/process-model) · [electron/electron](https://github.com/electron/electron) · [Releases API](https://api.github.com/repos/electron/electron/releases/latest) · [electron/governance](https://github.com/electron/governance) · [electron-userland/electron-builder](https://github.com/electron-userland/electron-builder)
- Apple / Swift:[MenuBarExtra](https://developer.apple.com/documentation/swiftui/menubarextra)([JSON](https://developer.apple.com/tutorials/data/documentation/swiftui/menubarextra.json)) · [Notarizing macOS software(JSON)](https://developer.apple.com/tutorials/data/documentation/security/notarizing_macos_software_before_distribution.json) · [App Sandbox(JSON)](https://developer.apple.com/tutorials/data/documentation/security/app_sandbox.json) · [FileManager(JSON)](https://developer.apple.com/tutorials/data/documentation/foundation/filemanager.json) · [File System Events(JSON)](https://developer.apple.com/tutorials/data/documentation/coreservices/file_system_events.json) · [Apple Developer Program](https://developer.apple.com/programs/) · [swift.org](https://www.swift.org/)
- 运行时 API:[Rust std::os::unix::fs::symlink](https://doc.rust-lang.org/std/os/unix/fs/fn.symlink.html) · [Node.js fs](https://nodejs.org/api/fs.html)
- 更新框架:[Sparkle](https://sparkle-project.org/) · [sparkle-project/Sparkle](https://github.com/sparkle-project/Sparkle)
