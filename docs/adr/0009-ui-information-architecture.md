# 主窗口与菜单栏的信息架构

> **部分取代。** Library Desk 的信息架构继续有效；窗口断点、滚动归属与
> overlay 行为由[窗口自适应布局决策](https://github.com/RookieZoe/skill-man/issues/36)
> 取代，locale/Preferences 结论由
> [ADR-0011](0011-interface-locale-and-message-ownership.md) 取代；Git Source Member 的
> 身份、来源分组和同名呈现由
> [ADR-0018](0018-git-source-namespaces-and-immutable-members.md) 取代。
> 全局 Activation Target group、Project Enable 与批量操作面由
> [ADR-0019](0019-enable-surfaces-target-resolution-and-batch-semantics.md) 取代旧的
> per-Agent 独立开关结论。

Skill Man 的主窗口采用 **Library Desk**：以 Library 中的 Managed Skill 为第一视角，使用 Skill 列表、详情与按 Activation Target Group 汇总的 Agent Inspector 组成三栏工作台。用户从目录名识别 Skill，在同一详情上下文中查看来源、健康状态与只读内容，并对每个物理全局 Target Enable / Disable。该基线由 [原型:主窗口 + 菜单栏 UI](https://github.com/RookieZoe/skill-man/issues/6) 的 A/B/C 原型评审选定；B 的 Agent-first switchboard 与 C 的 attention queue 不进入首个 spec。

选择 A 是因为它保持 Managed Skill 浏览、诊断与 Enable/Disable 的主视角，Library 仍是单一可信源，Activation 仍直指最终实体。Directory Identity 不再充当持久 Managed Skill identity；Library Desk 对 Git 成员按 Git Repository Source 分组，并用来源与 `skillPath` 区分同名成员。Agent 支持、Conflict、Broken、Modified、Import 和 Adopt 仍作为详情、弹层或明确事务流程进入。视觉方向遵循评审后的 Apple Liquid Glass：SF Pro、系统灰、Apple 蓝与低对比分隔线；玻璃材质只用于 toolbar、sidebar、inspector、sheet 和菜单栏面板，正文与表格保持清晰。

## 主要交互范围

- 左侧 Library 列表按全部、Broken、Modified、Link 和 Install 筛选；Git Source Member 按 Git Repository Source 分组，成员主标识显示 Directory Identity，repository/`skillPath` 区分同名项，frontmatter 名称只作辅助展示。
- 中间详情显示来源、最终实体路径、健康状态、只读 `SKILL.md` 与必要的 Broken / Modified 提示；不提供 app 内编辑。
- 右侧 Enable by Agent 检查器按 canonical Target 合并全部引用 Agent，只显示一份 Target-scoped Activation 开关、状态与健康操作，并在 Preview 列出全部受影响 Agent。
- 中间详情提供单 Skill 的 Project Enable；左侧 Library Toolbar 可进入临时多选模式，对显式选择的 Managed Skill 分别发起 Global 或 Project 批量 Enable。Global 与 Project 不混批，目标默认空。
- Activation Conflict 必须按占用者与范围进入 Target-local Switch、Adopt、逐项 Replace、Skip/Cancel 之一，不静默覆盖；项目级 flow 不提供 Adopt 或提交后的生命周期管理。
- Import 和 Adopt 是顶层入口，但完整流程使用 sheet：Import 经历来源、发现、多选、预览与结果；Adopt 保留扫描表、完整变更计划、逐 Skill 结果与当前会话 Undo。
- 菜单栏面板采用 A 的 Library Desk 快捷视图，优先显示最近启用 Skill、状态和打开主窗口入口，而不是变成第二套完整管理界面。
- 首次启动可重放且可跳过；Preferences 仍严格限于 ADR-0007 的四项。MVP 不出现 CLI 或无头自动化入口。

## Consequences

正式 spec 和实现应以 A 的三栏关系为骨架，而不是把 B 的 Agent-first 三列或 C 的行动收件箱并列为主窗口；相关能力被吸收到 Agent 检查器、筛选、sheet 和菜单栏提醒中。代价是用户若只想为某个 Agent 配置环境，需要先从 Skill 进入，而不是以 Agent 为主页；这也明确强化了 Library 作为单一可信源的心智模型。[原型:主窗口 + 菜单栏 UI](https://github.com/RookieZoe/skill-man/issues/6) 的 [throwaway 原型分支](https://github.com/RookieZoe/skill-man/tree/prototype/wayfinder-6-ui-variants) 与截图保留为评审依据，但原型代码不得直接合入 `main`。
