# 项目级 Skill 目录仅作 Enable 目标

状态：Accepted

> 项目条目直指来源实体等分发规则由 [ADR-0025](0025-project-local-skill-copies.md) 的已确认设计取代；其中 #101 已实现单 Skill、零附加 Agent 的副本交付与复用，附加 Agent 与批量分发待 #102–#104。本文其余项目自管边界继续有效。

Skill Man 支持把 Managed Skill Enable 到项目文件夹内解析后的 Agent skills 目录（如 `.claude/skills`、`.omp/skills`、`.agents/skills`），但项目级目录不进入扫描、Adopt、Import、健康检查与生命周期管理：Skill Man 不注册 Project 实体，不记录项目级链接，不做优先级管控与反向 Adopt。这取代 [ADR-0005](0005-adopt-existing-skills.md)「项目级 Skill 需要 Project/Profile 模型、留待后续」的排除条款——结论不是延期，而是显式不做。决议过程见 [决策:Project 领域模型与项目级 Activation](https://github.com/RookieZoe/skill-man/issues/70)，操作面与批量语义见 [ADR-0019](0019-enable-surfaces-target-resolution-and-batch-semantics.md)。

## 背景与理由

[2026-08-28 主流 Agent 调研](https://github.com/RookieZoe/skill-man/blob/4fd51c73a9a6a9d2d9e83be91d7e3cd2b449bdb6/docs/research/2026-08-28-agent-project-skill-dirs.md)表明：项目级与全局的优先级规则因 Agent 而异且互斥（Zed 项目覆盖全局、Claude Code personal 覆盖 project、Gemini workspace 最高、Codex 同名并存、opencode 扫描序敏感），Skill Man 发明统一层级必然与某家相悖；Gemini/Zed/Claude 的信任门无法从外部代办；项目内同名占位可能是用户自己的内容。因此项目级只保留一个动作面：Enable 时选定一个项目文件夹与一个或多个 Agent，沿完全受项目根 containment 约束的目录软链解析最终 skills 容器，再建直指最终实体的条目级软链（[ADR-0001](0001-activation-points-to-entity.md) 不变）。

项目目录的每个 symlink hop 与最终容器都必须留在 canonical 项目根内；安全的项目内 dangling 目标可经 Preview 创建，越界、cycle、不可读或无法唯一解析时对应 cell fail closed。多个 Agent 解析到同一容器时，本次 plan 去重为一次写入并披露全部受影响 Agent，但不持久化 Target group。精确同目标软链为 no-op；其它同名占用经确认后 Replace，真实目录先移入临时备份并明示目录/文件数量，结果关闭前可 Undo。

## Considered Options

- **Project 注册实体 + 项目级 Activation 追踪**：被否。生命周期、移动、失效、优先级全部需要建模，而各 Agent 规则互斥使管控价值落空；用户明确项目自管。
- **完全不支持项目级目标（维持 ADR-0005 排除）**：被否。把 Library 的 Skill 分发到项目目录是核心诉求。

## Consequences

- Activation 词条收窄为「受管启用项」；项目级一次性软链不追踪、不建领域词。
- Agent Configuration 获得项目级 skills 目录约定（仓库相对路径，可空），详见 [ADR-0016](0016-agent-configurations-global-roots-and-shared-targets.md)。
- 扫描汇总与去重分类（[决策:扫描去重、来源分类与汇总呈现](https://github.com/RookieZoe/skill-man/issues/77)）只覆盖全局；Enable 操作面（[决策:Enable 操作面与目标选择](https://github.com/RookieZoe/skill-man/issues/72)）增加项目目标分支与批量语义。
- Project 操作每轮只选择一个项目文件夹，可批量多个 Skill × 多个 Agent；当前 Bound Home 只保存最多 10 条成功使用的 canonical folder path MRU，且它只是便利历史，不是 Project 记录。
- 共享项目工位（`.agents/skills`）或项目内目录软链对多个 Agent 不可避免：本次 plan 按 resolved container 去重并披露物理共享，但 Skill Man 不做提交后的跨 Agent 联动管理。
