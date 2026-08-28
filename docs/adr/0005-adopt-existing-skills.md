# 存量 Skill 的 Adopt 流程

> `.skill-lock.json` 管理候选、installer/shared root 内 Local Source、Remote Install 分类与 Ownership Handoff 已由 [ADR-0013](0013-adopt-provenance-and-remote-source-parents.md) 取代；canonical entity 聚合、worktree/lock 来源归属、部分 Scan Report、候选选择与汇总由 [ADR-0017](0017-canonical-scan-aggregation-and-source-attribution.md) 取代；配置驱动的多 Root 扫描与 shared Activation Target 由 [ADR-0016](0016-agent-configurations-global-roots-and-shared-targets.md) 取代；项目级排除由 [ADR-0015](0015-project-level-enable-target-only.md) 取代。本 ADR 的其它 journal 与批量隔离规则继续有效。

Skill Man 以 **Adopt** 收编用户级 Agent 目录中的 Untracked Skill，同时把既有多级软链和共享目录布局迁移为 Library + 独立 Activation 模型。项目级目录依 ADR-0015 只是一次性 Enable 目标，不扫描、不 Adopt。

## 扫描与候选识别

Rescan 只消费已配置 Agent 的 canonical Global Skills Root union；Preset Detection、项目级目录、Library、system/builtin/cache 目录不进入扫描。Root scope 与共享 Target 以 ADR-0016 为准，调度和性能预算另行决定；Rescan 始终只读且绝不自动 Adopt。

每个目录条目解析完整 bounded symlink chain，并在一次 generation 内按最终文件系统对象身份形成 Canonical Skill Entity。同一实体经多个 Agent、Root、真实目录或多级软链可达时只形成一个候选，全部 Scan Appearance 逐条保留；canonical path 不是持久 Skill identity。目标已是 Library 实体或已登记的 Link 来源时，相关条目属于已有或待修复 Activation，不算 Untracked。

候选必须符合 Skill 定义（目录中含可读 `SKILL.md`）。dangling 条目没有最终实体，单列为 **Broken Untracked**，不允许 Adopt；用户只能重新定位目标、删除该目录条目或忽略，修复后再 Rescan。

## 真实目录与已有软链

稳定控制区外的 Untracked 最终实体是 Local Source，包括用户拥有的 Git 开发工作区。Adopt 只登记其真实 canonical 路径，原目录不移动、不复制、不改写；全局 Activation 直指该实体。实体若仍位于 Global Skills Root、installer-managed root 或 Home 中，用户须先选择稳定外部位置并完成可回滚迁出，再以 Link 认领。

已有软链不决定来源类型：最终实体的 ownership 位置、bounded worktree evidence 与 applicable external claim 共同分类。控制区外且无 applicable lock 的实体保持 Local；控制区内的受支持 Git evidence 或有效 Git lock 形成 Git Repository Source Candidate，并须 Fetch Latest 发现完整 Source Release。任何受管 Activation 都不保留 `link → link → real` 串联，遵循 ADR-0001。

已知 system/builtin/cache 永不收编。External Ownership Claim、worktree 与 lock 矛盾、来源无法解释或远端不可用使用 typed Conflict/Blocked/Deferred；不得用通用 warning 降级 Local。所有 Local Include、Conflict winner 与 Git source review 默认未选择。

## 共享 Root 与 appearance

共享 Global Skills Root 可以是 scan-only Root，也可以按 ADR-0016 成为多个 Agent 共用的 Agent Activation Target。物理 Root 只扫描一次，但 Scan Report 保留全部关联 Agent；一个共享 Target 只存在一条 Target-scoped Activation。

Adopt 只处理当前 Scan Report 已知的 appearances。部分 Root 失败时，保持实体原位的非破坏 Local Link 可以继续；迁移、删除、替换实体或释放 external ownership 必须等待完整 Scan Coverage。纳管完成后的 Enable 由用户选 Agent 并解析到唯一 Target，不按旧 appearance 自动勾选全部 Agent。

## Conflict 与批量交互

Scan Report 分层展示 Root coverage、Canonical Skill Entity、appearance、Git source group、Local candidate、Conflict Set 与 typed diagnostic。Scan incomplete 固定提示失败 Root 与受限操作；multiple appearances 是去重成功的信息，不是风险。Apply 前必须展示完整已知文件系统变更计划。

同一当前 Skill identity、但 Canonical Skill Entity 不同的 Local 候选形成 Conflict Set，不按内容或 mtime 合并。用户可显式选择一个 winner，其余保持 Untracked；同一实体多 identity 必须先统一名称后 Rescan。涉及 Git Source Member 的潜在同名关系保留全部来源事实并暂时阻断，最终 identity 由后继 namespace 决策确定。不得自动加后缀、改名或替换既有 Managed Skill。

## 事务、回滚与 Undo

每个 Skill 是独立事务：实体迁移或登记、SQLite 写入、旧条目处理与全部 Activation 创建必须全部成功；任一步失败就恢复该 Skill 的原始实体和目录条目。一个 Skill 失败不撤回批次中其他已成功项；批次结束展示成功/失败清单并允许重试失败项。

迁移使用 staging、补偿 journal 和临时备份处理跨目录及跨文件系统移动。成功批次在结果页关闭或应用重启前提供 **Undo 本批次**；Undo 前重新检查原路径未被外部占用。窗口结束后清理内容备份，仅在 SQLite 保留审计记录，不提供长期备份或历史任意回滚。

## Scope

Adopt 只管理用户级/全局 Skill。项目级 `.claude/skills`、`.agents/skills`、`.cursor/skills` 等目录不扫描、不纳管、不形成 Activation 或生命周期状态；它们只按 ADR-0015 接受用户逐次确认的 Enable。
