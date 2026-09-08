# 项目 Agent 条目确认覆盖验证（#103）

范围：沿用 #101–#102 的单 Skill 副本与多 Agent 相对链接流程，新增 Agent 同名条目的逐项确认覆盖。基础 Project Skill Copy 不接受覆盖确认，批量来源选择仍由 #104 交付。

## 行为与安全边界

- Preview 零写入，呈现原占用类型、路径、受影响 Agent 及真实目录的目录/文件数量。勾选不重新 Preview；Apply 的 `confirmedCellKeys` 只引用原 `planToken` 中的冲突 cell，默认跳过。不存在真实目录 Replace all 或 Adopt 跳转。
- Apply 重验原物身份、内容和权限、目录解析链及容器；无法安全快照的占用仅阻止该 Agent cell，其余副本与 Agent 可继续。安全快照预算为 128 MiB，拒绝特殊文件，不截断原内容。
- 项目副本 ready 后才以同一物理父目录内的排他 rename 备份原物，不跨卷拷贝原目录。原始链接按 link text 保存，不跟随外部目标。备份名称为本次 operation 的隐藏条目，由持久 journal 跟踪。
- Undo 先处理依赖，再处理本次副本。恢复原物之前重验备份内容及新链接身份；安全项继续，无法恢复的备份和依赖副本保留。结果 DTO 的 `recoveryRequired` 与中英文提示明确区分普通保留和原物待恢复。
- 启动维护使用冻结 journal，覆盖备份后、链接身份未持久化、提交、Undo 和部分清理等中断状态；不查询来源最新版本或依赖项目 Activation。无法证明安全时保留数据并暴露恢复需求。
- finalize 仅清理已经验证的 committed 备份；清理授权先持久化，重启可继续清理。Undo 中未恢复原物不会被重新视为可丢弃的 committed 备份。成功 finalize 关闭原操作 Undo 窗口，全局行为保持兼容。

## 自动验证

- `EnableApi` 连接真实 Core、隔离临时 Home 与 macOS 文件系统：目录、文件、外部和断裂软链覆盖/Undo，未确认保留，同 inode 同长度内容变化 stale，已有副本复用及 NoOp，无法快照的占用隔离，全局 finalize 关闭 Undo 窗口。
- 正式 `MaintenanceService.begin_startup` / `HealthApi` 重入：备份后、未捕获链接、提交、Undo 前/移除链接后/恢复原物后、外部改动及 finalize 部分清理；观察原物、副本和链接，多次重启不丢数据。
- `ProjectEnableSheet` + typed `CatalogClient`：中英文逐项确认绑定同一 Preview、未选项跳过、基础副本无覆盖入口、逐项部分 Undo 与原物恢复提示。
- 针对性：30 个项目集成测试、11 个 ProjectEnableSheet 测试通过。完整本地 CI 与原生验收结果在下方记录。

## 完整本地 CI

`npm run ci:local` 最终通过：37 个前端测试文件、290 个前端测试、698 个 Rust 测试通过；格式、capability、locale、lint、typecheck、release scripts、Web 构建和 Apple Silicon Tauri 原生构建均通过。未使用 GitHub-hosted Actions。

双轴 code-review 复查：Standards 与 Spec 均无剩余发现。审查中修复并回归了全局 finalize 关闭 Undo 窗口、不可快照占用按项隔离、typed 原物恢复状态，以及部分 Undo 后 operation 保留。

## 原生验收

2026-09-08，通过本次 Apple Silicon `.app` 完成 CUA 原生验收，使用现有健康 Skill `build-iterated-agentic-loop`、Codex 与 omp 配置。项目路径为独立临时目录 `skillman-103-native-h8hqgq64`；未改动来源、Agent 配置或全局 Activation。

- 在临时 `.codex/skills/build-iterated-agentic-loop` 放置原真实目录、`original.txt` 和 `nested/keep.txt`。中文 Preview 正确显示“真实目录将被备份：2 个文件，1 个文件夹”，确认框初始未勾选；实际截图检查长路径换行和确认控件可用。
- 逐项勾选后 Apply 为 `3 / 3 项已成功`：项目副本、Codex 覆盖链接和 omp 新链接。两条 link text 均为 `../../.agents/skills/build-iterated-agentic-loop`；三个入口的 `SKILL.md` SHA-256 相同。隐藏备份中两份原文件内容完整。
- 点击“撤销本次操作”后回到文件夹步骤。真实文件系统验证 Codex 恢复为真实目录，两份原文件内容逐字一致，本次副本和 omp 链接已移除，原物备份无残留。
- 验收 `.app` 与最终 CI 原生可执行文件 SHA-256 一致：`faaba8c1ebd82ae50a7d4beb8655ce8eff313aac35026c9a3dd417a232d94ac7`。结果页已关闭，应用返回技能库；临时项目作为 QA 产物保留，成功操作按既有规则记录 MRU。

中英文交互与原生真实目录覆盖/Undo 已验证；各中断及部分恢复场景通过隔离文件系统的正式启动维护入口自动验证。本记录不代表签名 Release、推送或 issue 关闭。
