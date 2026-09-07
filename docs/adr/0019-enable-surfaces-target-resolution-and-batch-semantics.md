# Enable 操作面、目标解析与批量提交语义

状态：Accepted

> 当前分发状态统一使用“已分发／未分发／部分分发”，见 [CONTEXT](../../CONTEXT.md#distribution-terminology)。本文 Enable/Disable 等名称保留内部接口语义，不代表 Agent 已加载或执行 Skill。

> Library 临时多选模式和批量入口部分由 [ADR-0022](0022-library-file-manager-selection.md) 取代；目标解析和 Core 提交语义保持有效。

[决策:Enable 操作面与目标选择](https://github.com/RookieZoe/skill-man/issues/72)
确认：Skill Man 把全局 Enable 与项目级 Enable 分成两个不可混批的操作面。全局操作创建受管、
Target-scoped Activation；项目级操作只在用户本次选择的项目文件夹内创建不追踪的一次性软链。
二者共用「条目级软链直指最终实体」的不变量，但不伪造相同的生命周期。

## 操作面与目标选择

- Library Desk 的 Agent Inspector 只处理当前 Skill 的全局 Activation。多个 Agent
  Configuration 引用同一 canonical Agent Activation Target 时合并为一个 **Activation Target
  Group**：一份开关、状态和健康事实，Agent 名称作为成员列表，路径作为次级证据。任何确认都列出
  全部受影响 Agent。
- 当前 Skill 的详情区另设 Project Enable 动作。Library Toolbar 的临时多选模式对一个或多个
  Managed Skill 分别发起 Global Enable 或 Project Enable；退出选择模式即清空 draft。两种目标
  选择默认空，可显式 Select all，但不跨操作记忆选择。
- Global 批次可选择多个 Activation Target Group。用户仍以 Agent 为入口，Core 把 Agent
  Configuration 解析到唯一 Target 并按 canonical Target 去重；Target 缺失或 identity mismatch
  时只引导到 Agent Management 修复，Enable 不顺带创建或修改全局配置。
- Project 操作每轮只选择一个项目文件夹，可包含多个 Agent 和多个 Skill。成功至少一个 cell 后，
  当前 Bound Home 只保存最多 10 条 canonical folder path 的 MRU；它没有 ID、Agent 关系、状态或
  自动写能力，不构成 Project 实体，使用时必须重新验证。

## 项目目录解析与直链

Agent Configuration 的 `project_skills_dir` 仍是安全的仓库相对路径。Project plan 从用户选择的
canonical 项目根开始做 bounded symlink walk；目录链的每个 hop、最近现存祖先与最终 skills
容器都必须位于该项目根内。安全的项目内 dangling 目标可在 Preview 明示后创建；越界、cycle、
不可读或无法唯一解析时，该 cell 保持 Blocked 且没有 override。

多个 Agent 解析到同一最终项目 skills 容器时，只在本次 plan 中形成临时 resolved-target group，
物理写入去重为一次，并列出全部已配置且会读取该目录的 Agent；提交后不持久化 group，也不创建
Project Activation。上述 containment 只约束项目 skills **容器**：容器内新建的 Skill entry 仍按
[ADR-0001](0001-activation-points-to-entity.md) 直指 Catalog 最终实体，因此 Git Source Member
指向 `<Home>/skills/git/<remote_id>/<skill_id>/`，Local Source 指向 canonical 外部路径，即使该
实体位于项目外。

## Enable Plan 与批量提交

一个 plan cell 的身份是 `(Managed Skill, resolved physical target, Directory Identity)`。Core
在零写入 Preview 中冻结 Home/WriteGate、Catalog 与 Agent Configuration generation、Skill
最终实体与来源健康、Target/project-root identity、项目目录 hop chain、entry occupancy 和用户的
逐 cell Conflict 处置。React 不循环调用单项命令；Core 以稳定顺序逐 cell 应用同一个 operation。

- Ready cell 独立提交。一个 cell 的 Conflict 或运行期失败不回滚其它成功 cell；全局
  WriteGate/Home identity 失效才停止所有尚未开始项。结果逐项区分 Succeeded、Already enabled /
  no-op、Skipped、Failed 与 Not attempted，Retry 必须重新 preflight。
- 同一 resolved target 上多个所选 Skill 争用同一 Directory Identity 时，用户逐 target 选择
  winner；不能用一个全局 winner，也不能按列表顺序暗选。未解决 cell 保持 Skipped，不阻塞其它
  Ready cell。
- 真实目录 Replace 必须逐 cell 展示将移走的目录与文件数量，不提供 Replace all。替换先把占用项
  rename 到 operation backup，再创建软链；普通 Enable、Managed Switch 与 Replace 都进入同一
  crash-safe journal。
- 结果页提供一次 **Undo this operation**，对全部成功 cell 逐项 CAS/identity 重验并继续撤销其它
  安全项；结果页关闭或应用重启后 finalize、清理临时备份，只保留必要审计，不提供长期任意回滚。

## Conflict 与生命周期

| 范围 / 占用者 | 操作语义 |
| --- | --- |
| Global / 另一 Managed Skill | `Switch` 只原子切换当前 Target 的 entry ownership；旧 Skill 在其它 Target 不变，失败保持旧链接，成功可随本 operation Undo |
| Global / Untracked，单 Skill flow | 保留 ADR-0003 的 Adopt existing / Remove then replace / Cancel；Adopt 的是占用者，不是已 Managed 的待 Enable Skill |
| Global / Untracked，批量 flow | 只提供逐 cell Replace 或 Skip；批量 Enable 不嵌套 Adopt |
| Global / 未纳管但已精确直指目标的一跳软链 | 仍按 Replace 删除并重建，不直接把既有链接接管为 Activation |
| Project / 已精确直指目标的一跳软链 | 幂等 no-op |
| Project / 其它软链或真实目录 | Replace / Cancel；真实目录使用临时备份与本 operation Undo，不提供 Adopt |

只有全局 Activation 进入 Disable、启动健康检查与 Repair。Inspector 按 Activation Target Group
Disable；missing entry 且最终实体健康时可单组 Repair。occupied、Target mismatch、dangling 与
Source Snapshot Mismatch 保持各自 typed closed action，不增加批量 Disable/Repair。Project
Enable 提交后不记录、不提供 Disable、健康检查或 Repair。

Source Snapshot Mismatch 继续阻止新 Enable；Git Source Member 消失或 rename 不自动改写既有全局
Activation；Create Local Source Copy 不自动切换任何 Activation。

## Consequences

- Activation 的 active entry uniqueness 必须按 `(Target, Directory Identity)` 表达；disabled
  历史记录不能继续占用 entry。Managed Switch 是 Target-local ownership transfer，不是先
  Disable everywhere 再 Enable。
- Project path 的既有 filesystem symlink 可以在严格项目内 containment 下参与解析；这取代
  [ADR-0016](0016-agent-configurations-global-roots-and-shared-targets.md) 对 project path
  symlink traversal 的一刀切禁止，但不放宽 `project_skills_dir` 字符串本身的相对路径约束。
- 批量能力深化既有 `plan → apply → undo/finalize` Module，而不是在 React 或 Agent Inspector
  复制一套状态机。代价是结果允许明确的部分成功；收益是跨多个外部目录时不承诺虚假的全局原子性。
