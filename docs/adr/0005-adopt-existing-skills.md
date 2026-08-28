# 存量 Skill 的 Adopt 流程

> `.skill-lock.json` 管理候选、installer/shared root 内 Local Source、Remote Install 分类与 Ownership Handoff 已由 [ADR-0013](0013-adopt-provenance-and-remote-source-parents.md) 取代；候选默认选择、风险交互与 Preview 证据由 [vNext 实施 Spec](../vnext-implementation-spec.md) 取代；配置驱动的多 Root 扫描与 shared Activation Target 由 [ADR-0016](0016-agent-configurations-global-roots-and-shared-targets.md) 取代；项目级排除由 [ADR-0015](0015-project-level-enable-target-only.md) 取代。本 ADR 的其它 Conflict、journal 与批量隔离规则继续有效。

Skill Man 以 **Adopt** 收编用户级 Agent 目录中的 Untracked Skill，同时把既有多级软链和共享目录布局迁移为 Library + 独立 Activation 模型。项目级 Skill 不属于本次 MVP；它需要尚未建立的 Project / Profile 领域模型。

## 扫描与候选识别

扫描所有已配置 Agent 的用户级 skills 目录，以及内置适配器声明的共享和 legacy 目录（如 `~/.agents/skills`、`~/.codex/skills`）。Library 自身、Codex `.system` 等已知系统缓存和内建目录明确排除。首次启动执行完整扫描；以后每次启动把只读轻量扫描与 Activation 健康检查合并，发现新 Untracked 时显示徽标但不打断用户；另提供手动 Rescan，绝不自动 Adopt。

扫描解析每个目录条目的最终 canonical target，并按实体分组。同一实体经多个 Agent 目录或多级软链可达时，只形成一个 Adopt 候选，同时列出全部出现位置。目标已是 Library 实体或已登记的 Link 来源时，相关条目属于已有或待修复 Activation，不算 Untracked。

候选必须符合 Skill 定义（目录中含可读 `SKILL.md`）。dangling 条目没有最终实体，单列为 **Broken Untracked**，不允许 Adopt；用户只能重新定位目标、删除该目录条目或忽略，修复后再 Rescan。

## 真实目录与已有软链

Agent 目录中的 Untracked 真实目录默认移入 `<Library>/skills/<name>`，再把它原先出现的所有 Agent 位置替换为直指该 Library 实体的 Activation。它成为 Library-owned 的 Adopt 实体；若该目录实际是开发工作区，用户应先把它移到 Agent 目录之外，再使用 Link Import，Adopt 不在后台猜测或改写开发路径。

已有软链若最终实体位于所有 Agent 目录之外，则把最终实体登记为 Link 来源，在 Library 建对应指针，并把所有已发现链路压平为直指最终实体的 Activation。若最终实体仍位于某个 Agent 目录，则按真实目录规则移入 Library。任何情况下都不保留 `link → link → real` 的串联结构，遵循 ADR-0001。

已知系统缓存和内建目录永不收编。其他疑似由外部 installer 管理的个人层条目照常展示，但标记风险、批量默认不勾选，只有用户显式确认后才移动。外部工具后来重建同名目录时，按 ADR-0003 的 Activation Conflict 处理。

## 共享目录迁移

`~/.agents/skills` 等同时被多个 Agent 读取的目录只作为 legacy/shared 扫描源，不作为受管 Activation 目标。否则一个目录条目会联动多个 Agent，无法实现独立 Enable / Disable。

Adopt 共享条目时，先把实体移入 Library 或登记为 Link，再移除共享条目，并在每个 Agent 的私有兼容目录创建独立 Activation。计划预览默认勾选所有已检测、且适配器声明会读取该共享目录的 Agent，以保持迁移前的有效可见性；用户可在 Apply 前取消任意 Agent。未检测到的 Agent 不创建目录。

## Conflict 与批量交互

扫描结果用可筛选表格展示 Skill、最终实体、出现位置、真实目录/软链、目标 Agent、风险与 Conflict。安全候选默认勾选；疑似外部托管、Broken 和 Conflict 默认不选。Apply 前必须展示完整文件系统变更计划。

同名但 canonical target 不同的候选保持为独立实体并标记 Conflict，不按内容或 mtime 合并。用户只能选择其中一个作为 canonical Skill；其余需要改目录名后 Rescan，或继续保持 Untracked。不得自动加后缀或改名。

## 事务、回滚与 Undo

每个 Skill 是独立事务：实体迁移或登记、SQLite 写入、旧条目处理与全部 Activation 创建必须全部成功；任一步失败就恢复该 Skill 的原始实体和目录条目。一个 Skill 失败不撤回批次中其他已成功项；批次结束展示成功/失败清单并允许重试失败项。

迁移使用 staging、补偿 journal 和临时备份处理跨目录及跨文件系统移动。成功批次在结果页关闭或应用重启前提供 **Undo 本批次**；Undo 前重新检查原路径未被外部占用。窗口结束后清理内容备份，仅在 SQLite 保留审计记录，不提供长期备份或历史任意回滚。

## Scope

MVP 只管理用户级/全局 Skill。项目级 `.claude/skills`、`.agents/skills`、`.cursor/skills` 等需要 Project / Profile、项目级 Activation 和层级冲突模型，明确留在本次方案 spec 之外。
