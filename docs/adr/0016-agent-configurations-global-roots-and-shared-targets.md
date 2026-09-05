# Agent 配置驱动多 Root 扫描与共享分发目标

状态：Accepted

[决策：Agent 模型扩展——自动检测、Custom Agent 与多目录](https://github.com/RookieZoe/skill-man/issues/71)
确认：Skill Man 用内置 **Agent Preset 模板**发现已知 Agent，但只有用户显式创建、保存在当前
Bound Home Catalog 中的 **Agent Configuration** 才决定扫描、Adopt 与分发范围。一个配置可扫描
多个 **Global Skills Root**，其中恰好一个可写 Root 是 **Agent Activation Target**；多个 Agent
可以共享同一 Target，Activation 因而按 `(Managed Skill, Target)` 而不是 `(Managed Skill, Agent)`
持久化。这样既保留 canonical Skill 去重和选择 Agent 的分发心智模型，也如实表达
`~/.agents/skills` 会同时影响多个 Agent 的物理事实。

本 ADR 取代 [ADR-0007](0007-first-run-and-settings.md) 的 Claude/Codex-only、单 Agent 单路径和
路径存在即 Agent 状态的模型，以及 [ADR-0005](0005-adopt-existing-skills.md) 的固定扫描源和
“shared root 永不作为 Activation 目标”结论；项目级分发继续遵循
[ADR-0015](0015-project-level-enable-target-only.md)。

## Preset、检测与配置

`PresetRegistry` 是随应用版本发布的只读模板注册表，不是 Catalog seed。首批模板为：

| Agent Preset | 默认用户级 Global Skills Root | 默认 Agent Activation Target | 项目级目录 |
| --- | --- | --- | --- |
| omp | 当前 native root（默认 `~/.omp/agent/skills`；支持 active profile / `PI_CODING_AGENT_DIR`）及其已启用的用户级 compatibility roots | 当前 native root | `.omp/skills` |
| Claude Code | `~/.claude/skills` | `~/.claude/skills` | `.claude/skills` |
| Codex | `~/.agents/skills`、`$CODEX_HOME/skills`（legacy compatibility） | `~/.agents/skills` | `.agents/skills` |
| Gemini CLI | `~/.gemini/skills`、`~/.agents/skills` | `~/.gemini/skills` | `.gemini/skills` |
| Cursor | `~/.cursor/skills`、`~/.agents/skills`、`~/.claude/skills`、`~/.codex/skills` | `~/.cursor/skills` | `.cursor/skills` |
| opencode | `~/.config/opencode/{skill,skills}`、`~/.claude/skills`、`~/.agents/skills`，以及本机配置的 `skills.paths` | `~/.config/opencode/skills` | `.opencode/skills` |
| GitHub Copilot | `~/.copilot/skills`、`~/.agents/skills`，以及用户显式添加的本机目录 | `~/.copilot/skills` | `.github/skills` |
| Zed | `~/.agents/skills` | `~/.agents/skills` | `.agents/skills` |
| Windsurf | `~/.codeium/windsurf/skills`、`~/.agents/skills`，以及启用 Claude 配置读取时的 `~/.claude/skills` | `~/.codeium/windsurf/skills` | `.windsurf/skills` |

表中动态 Root 只由有版本证据的 Adapter 解析；无法解析的配置形成 diagnostic，不能猜路径。
默认扫描只覆盖用户级 native、compatibility、shared 与 legacy Root。builtin、system/admin、
plugin/extension cache 和 cloud-only Root 不进入默认 Adopt 扫描；用户可在 Agent 管理中显式添加
安全的 scan-only Root。路径和软链能力证据来自
[主流 Agent 项目级 Skill 目录约定与软链行为调研](https://github.com/RookieZoe/skill-man/blob/research/agent-project-skill-dirs/docs/research/2026-08-28-agent-project-skill-dirs.md)。

Agent Detection 是零写入 Observation：

- **未检测到**：Preset 的已知用户级 Root 均不存在；不是 Broken。
- **已检测、未配置**：至少一个已知 Root 存在，但 Catalog 没有 Agent Configuration；不进入 Skill
  扫描。
- **已配置**：Catalog 配置及其 Root/Target 身份有效；这些 Root 进入扫描。
- **Target unavailable / mismatch**：配置存在，但 Target 缺失、不可读或身份变化；扫描其它有效 Root
  可以继续，Enable/Disable 与配置写入 fail closed。

Detection 不创建目录、不写 Catalog，也不因重启或 Rescan 自动重建已删除配置。Fresh Home 的 Agent
管理面由 `PresetRegistry + Detection Observation + 空配置列表` 合成；用户显式 Add/Configure 后才写
Agent Configuration。因此生产不再依赖 Agent seed，空配置表是合法产品状态。

Preset 创建出的配置与 Custom Agent 使用同一 CRUD 和 schema。Preset 保留 `preset_key`，可从模板
恢复或删除后重新添加；Custom Agent 使用生成的稳定 ID，compatibility 默认为 `unknown`。名称去除
首尾空白后须为 1–80 个字符，并以 NFKC + Unicode casefold identity 唯一；原始显示名仍是 Source
Content。Custom Agent 与 Preset 配置都可保存多个扫描 Root、一个 Target 和可空的安全仓库相对项目
目录。

### Home 配置后的扫描引导

Home 就绪后的引导先展示 Detection 与已有 Agent Configuration。用户可选择尚未配置的 Preset，
通过现有 configuration plan 审阅扫描 Root、Activation Target、项目级目录及缺失 Target 的创建影响，
再显式确认保存。批量保存逐项提交，每项在当前 generation 重新规划并核对已审阅的影响；失败保留
已保存项，重试只处理尚未配置项，不覆盖已有配置。自定义 Root 继续从 Agent Management 编辑。

用户可跳过整个引导；无配置时不能继续执行空范围扫描。配置完成后的 Continue 发起共享 Scan Run，
完成状态只接受相同 run identity 的 terminal Report，Finish 打开完整 Evidence Ledger。常态启动
不自动扫描；用户可从 Agent Management 的“配置扫描范围”重新进入这一流程。

## Catalog 与 Module Interface

下一 Catalog migration（当前 v7 之后）采用以下逻辑结构；具体 SQL 名称可调整，但约束不能弱化：

```text
agent_configurations(
  agent_id PK,
  origin(preset/custom),
  preset_key nullable,
  name,
  name_identity_key UNIQUE,
  compatibility,
  project_skills_dir nullable,
  created_at, updated_at
)

global_skill_roots(
  root_id PK,
  configured_path,
  path_identity_key UNIQUE,
  created_at, updated_at
)

agent_global_roots(
  agent_id FK,
  root_id FK,
  role(scan_only/activation_target),
  PK(agent_id, root_id)
)

activations(
  skill_id FK,
  target_root_id FK,
  directory_identity_key,
  desired_enabled,
  expected_entry_path,
  expected_target_path,
  observed_state,
  last_enabled_at, last_checked_at,
  PK(skill_id, target_root_id)
)

UNIQUE active_activation_entry(
  target_root_id, directory_identity_key
) WHERE desired_enabled
```

每个 Agent Configuration 恰有一个 `activation_target` membership；同一 Root 可被多个配置引用。
只有 `desired_enabled` 的记录占用 `(Target, Directory Identity)`；disabled 历史记录不持有 entry，
因此 [ADR-0019](0019-enable-surfaces-target-resolution-and-batch-semantics.md) 的 Target-local
Managed Switch 可以在同一 Catalog transaction 中释放旧 ownership、取得新 ownership。
任何存在 Activation 的 Target 必须至少有一个 Agent Configuration 引用。删除 Agent、Root 或 Target
使用 `RESTRICT`/Core invariant，而不是 cascade 掉 Activation。旧 `agents.skills_path` 迁移为单一
`activation_target` Root，旧 Activation 按 canonical Target 聚合；非法路径、重复 identity 或不能
无歧义聚合时整个 migration fail closed，不迁移“看起来安全”的子集。旧 `detected` 不迁移。

Agent 配置仍是 Home-scoped Catalog 状态；App-level state 不新增 `agents.json`。这使扫描范围、写授权、
Activation 与其变更护栏保持在同一个 Bound Home 和 WriteGate snapshot 中，Abandon 后的新 Home 不会
继承旧 Home 的外部写权限。

Core 提供一个深的 Agent Management Module：调用者只取得管理 snapshot、规划/应用配置变更、取得
canonical scan-root snapshot，以及把 `agent_id` 解析为 Target 与全部受影响 Agent。Preset 动态解析、
Detection、path identity、共享 Target、Catalog transaction 和恢复逻辑留在 Implementation 内。扫描
Module 不自行读取 Agent 配置；Activation Module 不自行推导路径。

## 扫描、Adopt 与分发

Rescan 只消费已配置 Agent 的 Root。先按 Root canonical path 求并集、每个物理 Root 扫一次，再按
[ADR-0017](0017-canonical-scan-aggregation-and-source-attribution.md)在一次 generation 内以最终文件
系统对象身份聚合 Canonical Skill Entity 与全部 Scan Appearance。删除 Agent Configuration 会从下一
snapshot 移除仅由它引用的 Root；未纳管候选随 Rescan 消失，已经 Managed 的 Skill、来源记录和 Home
实体不自动 Remove 或回滚。

启动与缓存语义以 [ADR-0020](0020-startup-observations-and-manual-rescan.md) 为准：常态启动只运行
零写入 Detection、Startup Probe 与 Target-scoped Activation health，不自动执行完整 Rescan。
onboarding 首次配置后或用户手动触发的完整 Scan Run 才产生 Scan Report；配置变化使旧 Report/plan
stale，但不自动重跑。

去重后的候选按 ownership 位置、bounded worktree 与 applicable lock 分类：稳定外部开发工作区即使
属于 Git repository 仍是 Local Source，在 Catalog 记录真实目标路径；控制区内的 Git hint 与有效
*Git lock 按 repository 聚合，Fetch Latest 后才形成完整 Source Release。Git Repository Source
按 [ADR-0018](0018-git-source-namespaces-and-immutable-members.md) 安装到
`<Home>/skills/git/<remote_id>/<skill_id>/`（旧版 `<Home>/skills/{repo_user}/{repo_name}/{skill_name}`
不再是当前路径）；正式路径始终从不可变 Bound Home 解析，不另设固定 Home。单 Root 失败保留 typed
diagnostic 和其它 Root 的部分结果，但会移动、删除、替换实体或释放 external ownership 的操作必须
等待完整 Scan Coverage。

全局 Enable 先选择 Agent，再解析其唯一 Target；不逐次询问目录，也不复制到全部扫描 Root。多个 Agent
引用同一 Target 时只存在一个物理 Activation。Inspector 必须把这些 Agent 作为同一 Activation Target
Group 呈现，只提供一个开关与健康状态；从任一 Agent 发起的 Enable/Disable 都操作同一记录，并在确认前
列出全部受影响 Agent。Target 缺失或 identity mismatch 时，Enable 只引导到 Agent Management，
不得顺带创建或修复配置。

## 路径与变更护栏

- Global Skills Root 使用绝对、UTF-8 可表示的安全路径；`~` 只允许作为开头并由 Core 展开。存在路径按
  canonical identity 去重；缺失尾部只能基于已 canonicalize 的现存祖先规范化，并在首次写入前重验。
- 精确相同的 Root 共享同一记录；不同 Root 不得相同、互为父子、等于 Bound Home、位于 Bound Home
  内或包含 Bound Home。system/builtin/cache Root 不可成为 Target。
- Agent Activation Target 必须是该 Agent 的可写 Root。缺失 Target 只有在 Agent Management
  的显式配置 plan 中确认后才创建；Enable plan 不创建 Target。目标既有内容只读归类为
  Untracked/Conflict，不自动 Adopt、覆盖或删除。
- `project_skills_dir` 字符串必须是无 root、`.`、`..` 的仓库相对目录。Project Enable 可以解析
  既有 filesystem symlink，但 bounded walk 的每个 hop、最近现存祖先与最终 skills 容器都必须位于
  canonical 项目根内；安全的项目内 missing target 可经 Preview 创建，其余 fail closed。项目级写入
  仍按 ADR-0015 逐次选择项目文件夹。
- 修改、恢复或删除配置绝不自动迁移 Activation，也不删除 Root 内容。旧 Target 仍有其它 Agent
  引用时，只解除当前关系并明示该 Agent 将不再看到的 Skill 数；当前配置是最后引用者且 Target 仍有
  Activation 时阻止操作，要求先全部 Disable。
- 仅修改 scan-only Root 不受 Activation 阻断。任何配置 Apply 前重验名称、Root identity、Target
  occupancy、Home overlap 与 WriteGate generation；变化使 plan stale。

## UI 与后果

Agent Configuration 使用独立 **Agent Management** 表面，由主界面顶层入口和应用菜单进入，并复用于
首次绑定后的检测/配置流程；它不进入 Preferences。管理面区分 Configured、Detected but Unconfigured
和其它 Preset 模板，显示全部 Root、唯一 Target、shared consumers、compatibility evidence，以及
Add/Edit/Delete/Restore/Rescan。Library Desk 的 Agent Inspector 只管理当前 Skill 的
Enable/Disable，并按共享 Target 合并开关。

被拒绝的模型包括：检测即配置、把 Detection 持久化为 truth、App-level Agent 配置、单 Agent 单扫描
路径、Enable 自动写全部 Root，以及把共享路径伪装成彼此独立的 Agent Activation。代价是 schema、
DTO 与 Inspector 都必须从 Agent-scoped Activation clean-cutover 到 Target-scoped Activation；收益是
扫描、Adopt、共享目录和实际分发结果使用同一套可验证物理语义。
