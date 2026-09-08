# 项目批量分发验证（#104）

范围：Library 多选通过同一 typed request 与 Core operation 分发多个 Skill，沿用 #103 的逐 Agent 条目覆盖确认。

## 行为与安全边界

- Directory Identity 使用既有 NFC + Unicode casefold。同名且无项目副本时，选择绑定副本 cell；所有依赖 Agent 使用同一来源。未选择或非法多个 winner 跳过该 identity，其他 Skill 继续。已有有效副本保留内容和权限，使用一个可执行的复用 action，不要求来源选择。
- Preview 零写入。来源选择重新 Preview 并清除旧覆盖确认；覆盖确认只绑定当前 token。Retry 重新解析来源、目录和占用。结果逐 Skill / Agent 标识成功、复用、跳过、失败、未执行，不创建项目 Activation。
- 真实共享容器按解析后的物理路径去重。对尚不存在且仅大小写/Unicode 拼写等价、无法证明同一物理容器的路径，保守阻止歧义链接，不能伪报共享成功。未来产物的祖先/子孙重叠也按规范化路径组件阻止。
- 一个 operation journal 保存按 copy cell 标识的多副本记录，链接持久记录其副本依赖；保留旧单副本 journal 的启动恢复兼容性。每份副本独立暂存及提交，普通失败不回滚其他成功项。全局 WriteGate / Recovery Required 仍停止后续写入。
- Undo 先处理链接，再按每份副本自己的依赖结果处理副本。改动过、原有或仍有未撤销依赖的副本保留，安全组继续。启动恢复读取冻结 journal，不重新获取来源；原物备份继续使用通用覆盖恢复协议。
- MRU 只由成功写入更新；纯复用、跳过和失败不生成成功历史。

## 自动验证

`EnableApi` 使用隔离临时 Home 和真实 macOS 文件系统，覆盖多副本单 operation、大小写及 Unicode 等价来源、非法多个 winner、已有副本内容/权限保留、共享容器、跨副本重叠、混合成功/失败/跳过/未执行、部分 Undo 和正式启动恢复重入。已有单 Skill、覆盖/原物恢复、全局 Enable 回归继续运行。

`ProjectEnableSheet` + typed `CatalogClient` 覆盖批量入口、零 Agent、中英文来源选择、切换来源清除覆盖确认、部分结果及部分 Undo。Library 多选入口继续通过 `LibrarySelection` 行为测试。

## 完整本地 CI 与审查

`npm run ci:local` 最终通过：37 个前端测试文件、292 个前端测试、708 个 Rust 测试；格式、capability、locale、lint、typecheck、release scripts、Web 构建及 Apple Silicon Tauri 原生构建均通过。项目针对性套件包含 40 个 Rust 测试和 13 个 ProjectEnableSheet 测试。未使用 GitHub-hosted Actions。

双轴 code-review 最终无剩余发现。审查中通过失败测试复现并修复了已有副本被不健康同名来源遮蔽，以及未来目录的大小写等价重叠绕过；同时拒绝把无法证明物理同一的不同目录拼写合并并伪报成功。

## 原生验收

2026-09-08，使用本次 Apple Silicon 无签名 `.app`，在独立临时项目 `skillman-104-native-so0qm3c0` 完成 CUA 原生验收：

- 从 Library 通过键盘多选 `grill-me` 与 `grilling`，打开“向项目启用 2 个技能”，选择 Codex、omp。Preview 显示两份副本及四个 Agent 动作，披露 General 对基础目录的共享读取。真实文件系统确认此时没有 `.agents` 写入，原目录内容未变。
- 临时 Codex 的 `grill-me` 位置预置真实目录及 `original.txt`。Preview 正确显示 1 个文件、0 个目录；确认后同一批次 `6 / 6` 成功。四条链接均为 `../../.agents/skills/<name>`，各 Agent 的 `SKILL.md` 与对应副本内容相同；原物备份完整。
- 一次“撤销本次操作”返回文件夹步骤。真实文件系统确认两份副本和四条新链接全部移除，Codex 原真实目录及文件内容恢复，隐藏备份无残留。
- 已检查原生预览与结果截图，长路径能换行，六项结果按 Skill / Agent 清晰区分。验收后关闭操作页，应用返回技能库。临时目录作为 QA 产物保留，成功操作按既有行为更新 MRU。
- 验收 `.app` 与 CI 原生可执行文件 SHA-256 一致：`80205c33eb7b74eeebdf207d05eb6c0a200af45d94b59980fd0d8fb91e251830`。

中英文同名来源交互与各中断恢复路径通过自动行为测试验证。此记录不代表签名 Release、推送或 issue 关闭。

后续已补充 [项目移动读取与原生重启恢复验收](project-relocation-restart-verification.md)：整体移动后隔离原 Home 仍可读取，以及批量提交后强制退出、连续两次启动的恢复与 finalize 均通过；具体中断窗口及验收边界见该记录。
