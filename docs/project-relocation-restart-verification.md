# 项目移动读取与原生重启恢复验收（#100）

2026-09-08，通过 CUA 操作真实 macOS/Tauri 窗口，在用户提供的 QA 目录完成移动读取，并在独立项目中完成强制退出后的启动恢复。

## 验收版本

- 代码提交：`ace626ee2114fcdf3d6b22662536390da4845dcb`。
- 本次重新执行 `npm run tauri build -- --bundles app --no-sign --target aarch64-apple-darwin`，退出码 0。
- `.app` 可执行文件 SHA-256：`80205c33eb7b74eeebdf207d05eb6c0a200af45d94b59980fd0d8fb91e251830`，与既有 #104 本地 CI / 原生验收版本一致。
- 本次未修改产品代码，未重新运行完整 CI。此前完整 CI 记录见 [项目批量分发验证](project-batch-verification.md)。这是 Apple Silicon 无签名应用验收，不代表签名 Release。

## 移动项目后读取：通过

1. 确认 `/Users/zoe/Desktop/skillman-qa/project-a` 与 `/Users/zoe/Documents/project-a-moved` 都是真实空目录。
2. 原生 Library 多选 `grill-me`、`grilling`，输入 `project-a` 路径，选择 Codex、omp。Preview 展示两份项目副本和四个 Agent 链接，披露 General 共用基础目录；此时项目仍为空。
3. 执行后原生结果页显示 `6 / 6 项已成功`。关闭结果页，完成 finalize。
4. 将整个 `project-a` 重命名为 `project-a-moved`，替换用户预建的空目标目录，没有嵌套一层目录。确认原路径不存在，项目根的 device/inode 不变。
5. 对比移动前后完整项目树：文件 SHA-256、权限、目录与原始软链文本全部一致。四条 Agent 链接均为 `../../.agents/skills/<name>`；所有软链解析目标均位于移动后的项目内。逐 Skill 对比基础副本、Codex 和 omp 三个入口下的完整内容，全部一致。
6. 使用独立 `sandbox-exec` 读取进程禁止访问 `/Users/zoe/SkillMan`。先确认读取真实来源 `SKILL.md` 抛出 PermissionError，再从移动后项目的三个入口读取并校验两份 Skill，全部通过。未移动或修改真实 Home / 来源。

| Skill | 三个入口一致的 SKILL.md SHA-256 |
| --- | --- |
| grill-me | `6189dfceb7304a6e5558f75d87e68fa3bc7fcf7ba120e44f21f8a61fe01eba54` |
| grilling | `fa5c1e5ee76b1c8f1ae56101f52c9e239de75d5c578adc61227b92d10b7e52ef` |

## 强制退出与重复启动：通过

1. 新建空目录 `/Users/zoe/Desktop/skillman-qa/project-b`。预置真实目录 `.codex/skills/grill-me`，其中 `original.txt` 内容为 `original-before-replacement` 加换行。
2. 使用同样两个 Skill、Codex 和 omp 原生分发。Preview 正确披露原真实目录包含 1 个文件、0 个文件夹；勾选该条目的替换确认后执行，结果为 `6 / 6` 成功。
3. 保持结果页打开，不关闭、不 Undo。真实文件系统验证两份副本、四条链接及原物备份完整；对应 operation `enable-167564-4` 的 journal 为 `committed`，仍待 finalize。
4. 核验 QA `.app` 进程路径后，对该进程发送 SIGKILL，模拟强制退出。重新启动同一 `.app`，走正式启动恢复入口。
5. 首次启动正常进入 Library，无恢复错误、无旧结果页或旧 Undo 入口。完整项目树对比确认成功产物内容、权限和链接未变；仅安全清理了原物备份目录及其中的文件，对应待恢复 journal 已清除，原目录没有被误恢复。
6. 正常退出并第二次启动，再次核对完整项目树与 journal：内容完全不变、journal 不重现。移动后的 `project-a-moved` 同时保持完整可读。

## 边界与保留产物

- 原生重启覆盖的是“批量操作已提交、结果页尚未 finalize”的确定性中断窗口；未在原生应用中注入拷贝暂存、备份后、链接身份持久化前或 Undo 中途故障。这些阶段的行为覆盖继续以现有自动测试为依据。
- 来源隔离针对本次读取进程；没有在另一台 Mac 上运行第三方 Agent，也不把文件读取成功等同于 Agent 发现、信任或实际执行。
- 保留 `project-a-moved`、`project-b` 作为用户可复查的 QA 产物。原 `project-a` 已整体移动。应用停留在正常 Library 页面；成功操作按既有行为更新最近项目记录。
- 此记录补齐移动读取和上述原生启动恢复验收，不自行代表 GitHub issue 已关闭。
