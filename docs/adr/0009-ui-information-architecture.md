# 主窗口与菜单栏的信息架构

Skill Man 的主窗口采用 **Library Desk**：以 Library 中的 Managed Skill 为第一视角，使用 Skill 列表、详情与按 Agent Activation 检查器组成三栏工作台。用户从目录名识别 Skill，在同一详情上下文中查看来源、健康状态与只读内容，并对每个 Agent 独立 Enable / Disable。该基线由 [Wayfinder #6 的 A/B/C 原型评审](https://github.com/RookieZoe/skill-man/issues/6) 选定；B 的 Agent-first switchboard 与 C 的 attention queue 不进入首个 spec。

选择 A 是因为它最直接表达现有领域模型：Skill 身份=目录名，Library 是唯一可信源，Activation 是每个 Agent 目录中直指实体的符号链接。它能同时覆盖浏览、诊断与高频开关；Agent 支持、Conflict、Broken、Modified、Import 和 Adopt 则作为当前详情、弹层或明确事务流程进入，而不改变主心智模型。视觉方向遵循评审后的 Apple Liquid Glass：SF Pro、系统灰、Apple 蓝与低对比分隔线；玻璃材质只用于 toolbar、sidebar、inspector、sheet 和菜单栏面板，正文与表格保持清晰。

## 主要交互范围

- 左侧 Library 列表按全部、Broken、Modified、Link 和 Install 筛选；Skill 主标识显示目录名，frontmatter 名称只作辅助展示。
- 中间详情显示来源、最终实体路径、健康状态、只读 `SKILL.md` 与必要的 Broken / Modified 提示；不提供 app 内编辑。
- 右侧 Enable by Agent 检查器显示每个 Agent 的独立 Activation。普通 Enable / Disable 立即可见；Activation Conflict 必须进入 Adopt、显式移除后替换或取消，不静默覆盖。
- Import 和 Adopt 是顶层入口，但完整流程使用 sheet：Import 经历来源、发现、多选、预览与结果；Adopt 保留扫描表、完整变更计划、逐 Skill 结果与当前会话 Undo。
- 菜单栏面板采用 A 的 Library Desk 快捷视图，优先显示最近启用 Skill、状态和打开主窗口入口，而不是变成第二套完整管理界面。
- 首次启动可重放且可跳过；Preferences 仍严格限于 ADR-0007 的四项。MVP 不出现 CLI 或无头自动化入口。

## Consequences

正式 spec 和实现应以 A 的三栏关系为骨架，而不是把 B 的 Agent-first 三列或 C 的行动收件箱并列为主窗口；相关能力被吸收到 Agent 检查器、筛选、sheet 和菜单栏提醒中。代价是用户若只想为某个 Agent 配置环境，需要先从 Skill 进入，而不是以 Agent 为主页；这也明确强化了 Library 作为单一可信源的心智模型。Wayfinder #6 的 [throwaway 原型分支](https://github.com/RookieZoe/skill-man/tree/prototype/wayfinder-6-ui-variants) 与截图保留为评审依据，但原型代码不得直接合入 `main`。
