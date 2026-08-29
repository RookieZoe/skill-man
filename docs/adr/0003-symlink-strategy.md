# 符号链接策略与冲突规则

Skill Man 的 Activation 管理与冲突处置规则。词汇遵循 [CONTEXT.md](../../CONTEXT.md),Activation 结构遵循 [ADR-0001](0001-activation-points-to-entity.md)(直指实体,不串联)。

**Activation 生命周期.**
- **Global Enable** = 在 Agent Activation Target 建条目级符号链接，直指 Catalog 解析出的 Skill 最终实体；Git Source Member 使用 `<Home>/skills/git/<remote_id>/<skill_id>/`，其它 Install 使用各自 Home 实体，Link 使用 Local Source canonical 最终路径。**只建条目级,绝不把整个 agent skills 目录做成软链**(Claude Code #38051 整目录软链回归史)。共享 Target 只有一份 Activation 与一份操作控件，详见 [ADR-0019](0019-enable-surfaces-target-resolution-and-batch-semantics.md)。
- **Project Enable** 同样只建直指最终实体的条目级软链，但它位于本次 plan 解析出的项目内 skills 容器，不登记为 Activation，也没有项目级 Disable/健康检查。
- **Disable** = 删除 Activation。统一对所有 agent 删链,**不写 agent 原生开关**(Codex `config.toml` / Gemini `/skills disable` / Claude `skillOverrides`)——保持单一语义,原生开关记录为未来增强。
- **Remove** = 非 Git Install 删除实体(连带先移除其所有 Activation)，Link 仅删除 Catalog 引用、原地实体不动；Git Repository Source 依 [ADR-0018](0018-git-source-namespaces-and-immutable-members.md) 整组 Remove，Source Member 不可单独 Remove。

**健康检查(Claude Code #50052 的兜底).**
Claude Code 自动更新会静默删除 `~/.claude/skills` 下的软链(未修复确认)。Skill Man 在**每次启动时**校验全部受 Catalog 管理的全局 Activation 是否仍在、目标是否有效；缺失 entry 且目标有效时可修复，Source Member 消失导致的 dangling Activation 标为 Broken、保留到 Disable 或同路径成员恢复，不能重建到其它成员。项目级一次性软链不追踪、不检查。实时监听(agent 目录 fsnotify)不入 MVP。

**冲突规则(Conflict,两类).**
- **Library 内重名**：非 Git 与非 Git 同 Directory Identity 时继续阻止入库；Git Source Member 以及显式 Create Local Source Copy 可与同名 Managed Skill 共存，以 Git Repository Source/`skillPath` 区分。系统不自动加后缀或改写 Source Content 名称。
- **Global Activation 占用**：同一 Agent Activation Target 的 active Directory Identity 仍唯一。另一 Managed Skill 占用时只允许当前 Target 的原子 Switch / Cancel；Untracked 占用在单 Skill flow 沿用 Adopt / 明示移除后替换 / Cancel，在批量 flow 只允许逐 cell Replace / Skip。即使未纳管的一跳软链已精确直指目标，也删除并重建，不直接接管。
- **Project entry 占用**：精确直指同一实体的一跳软链是幂等 no-op；其它软链或真实目录只允许 Replace / Cancel，不提供 Adopt。真实目录先进入临时 operation backup，Preview 明示目录与文件数量，结果关闭前可 Undo。**绝不静默覆盖用户文件**。

**Broken 处置.**
Local Source 不可用或 Git Source Member 从当前 Source Release 消失时标红。**Activation 保留不动**——dangling 软链对各 Agent 无害；Local Source 恢复，或相同 `(remote_id, skillPath)` 重新出现并复用 `skill_id` 时自动恢复。否则用户显式 Disable，不自动改指同名成员。

**Library 布局.**
Library 是逻辑边界而非一个扁平名称目录。Git Source Member 使用 `<Home>/skills/git/<remote_id>/<skill_id>/`；其它 Install 继续使用其来源约定的 Home 实体；Link 只在 SQLite 记录 canonical 最终实体路径，不在 Home 建指针。SQLite 保存来源、Directory Identity、Activation desired/observed state 与健康事实。
