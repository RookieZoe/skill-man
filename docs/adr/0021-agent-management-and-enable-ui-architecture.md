# Agent Management 表面与 Enable 操作面的 UI 信息架构

状态：Accepted

> 编号说明：本 ADR 原编号 0020，与[启动观察与手动 Rescan](0020-startup-observations-and-manual-rescan.md)撞号，改为 0021；决议内容不变。

[原型:Agent 配置、项目作用域与 Enable 操作面 UI](https://github.com/RookieZoe/skill-man/issues/75)的 A/B/C 原型评审选定 **A — Calm desk** 作为新能力的 UI 基线：[ADR-0009](0009-ui-information-architecture.md) 的 Library Desk 三栏骨架继续有效，Agent Management 作为独立顶层表面以「状态分组导航 + 配置列表 + 配置详情」三栏呈现，全部目标选择、冲突处置、扫描汇总与来源管理沿用上下文保留 sheet。评审依据为 [throwaway 原型分支 `prototype/wayfinder-75-agent-enable-ui`](https://github.com/RookieZoe/skill-man/tree/prototype/wayfinder-75-agent-enable-ui)（commit 3dd9e9e）与分支内 19 张截图；原型代码不合入 `main`。

## 决议

- **表面切换**：主窗口 toolbar 增加 Library / Agents 两个表面，Library 仍是默认主页。Agent Management 不进入 Preferences（[ADR-0016](0016-agent-configurations-global-roots-and-shared-targets.md) 的独立顶层表面结论由此获得具体形态）。
- **Agent Management 三栏**（A 基线）：左栏按 Configured / Detected but Unconfigured / Preset templates 三段分组导航，携带「检测零写入」常驻提示与 New custom agent 入口；中栏为配置列表，Configured 段展开 Root 数与唯一 Target 摘要，Detected 段只列检测证据，Preset 段是可发起配置的横条；右栏为所选配置详情：compatibility evidence 只读卡、Global Skills Roots 列表（radio 标注唯一 Agent Activation Target）、shared consumers 卡与 Edit / Configure from template 入口。增删改使用单一 sheet：多 Root 增删、radio 选唯一 Target、`project_skills_dir` 字段、删除需确认且说明既有 Activation 仍按 Target 保留。
- **Library Desk 右栏**：Agent Inspector 呈现为 Activation Target Group 卡——shared target 单开关并列出全部消费者数量，冲突目标单独成卡提供处置入口；底部常驻「项目级操作独立」脚注，防止把一次性软链误读为受管生命周期（[ADR-0015](0015-project-level-enable-target-only.md)）。
- **Enable 操作面**：全部为上下文保留 sheet。Global Enable 三步（目标组选择，选择默认空 → Skill × resolved target 预览矩阵，冲突逐 cell 决策 → 结果，区分 Succeeded/Skipped 并提供一次 Undo this operation）；Broken（成员从 Source Release 消失）成员只有专属 Disable 三步流，共享目标组整组停用；批量经 Library Toolbar 多选与底部 selection shelf 发起，Global 与 Project 不混批（[ADR-0019](0019-enable-surfaces-target-resolution-and-batch-semantics.md)）。
- **Project Enable 四步**：选项目文件夹（MRU 列表 + 原生选择器占位）→ 选 Agent → Preview（按 resolved container 去重，临时 resolved-target group 披露全部受影响 Agent 与"1 次物理写入"）→ 结果。同名占用为真实目录时，替换前明示将移走的目录/文件数量，并要求显式勾选承认删除后果；不提供 Replace all；结果页警示「未创建 Project 记录」，Undo 仅在结果关闭前可用。
- **扫描汇总**：onboarding 与常态 Rescan 共用同一 sheet——funnel 计数（Configured Agents → declared roots → canonical roots → appearances → canonical entities）、四类计数卡（Git source candidate / Local / Conflict set / Excluded·already Managed appearances）、Root coverage 表与候选列表（Source group 整体、Local 逐项、Conflict set 挑 winner，全部默认空）。Scan Incomplete 时破坏性主按钮禁用（[ADR-0017](0017-canonical-scan-aggregation-and-source-attribution.md)）。
- **Git 来源组呈现**：来源组卡承载 policy 级联（Release → SemVer tag → tag → HEAD）与显式 override、Source Snapshot Mismatch 面板（阻止 Update/新 Enable，提供 Restore Current Source Release 与 Create Local Source Copy，后者不切换 Activation）与来源级 Update/Remove；成员行只读展示 `skillPath` 与健康，成员删除使全局 Activation Broken 且仅提供 Disable（[ADR-0018](0018-git-source-namespaces-and-immutable-members.md)）。
- **设计签名与适配**：详情区 Evidence rail（Directory identity / Canonical entity / Source release / Activation evidence 四格）为签名元素；mid 断点下右栏收为抽屉并提供浮动入口；en / zh-Hans 双语文案在全部新增表面与 sheet 覆盖。

## Considered Options

- **B — Source ledger**：来源组表格使仓库级生命周期一目了然，但把来源账目置于 Library 心智之上，Agent 管理的目标拓扑图偏工程图，削弱 ADR-0009 评审确立的安静原生工具气质。被否。
- **C — Target workbench**：把 Enable 提升为一等通道对高频分发更直接，但常驻 Global/Project 双通道使一次性 Project 操作在界面中获得近似实体的地位，与「项目级不是实体」的领域边界存在表达冲突，且三栏密度最高。被否。
- **A — Calm desk（选定）**：Library Desk 骨架与既有心智完全保留，Agent Management 与 Enable 能力以平行表面 + sheet 渐进呈现；代价是 Agent 侧信息密度低于 B、Enable 直达性低于 C，需要靠 Evidence rail 与 selection shelf 补足。

## Consequences

- 规格修订（把本决议写入 `docs/vnext-implementation-spec.md` 的 UI 章节并补齐双语 key 清单）由地图后续票交接；产品实施在地图之外另立 ready-for-agent 票。
- 原型分支与截图保留为评审依据；三变体中未被选中的 B/C 结构不进入 spec，可引用其截图作为被否方案的证据。
- Toolbar 表面切换、selection shelf、Evidence rail、mid 抽屉成为共享 UI 契约，后续任何表面变更需与本决议对齐。
