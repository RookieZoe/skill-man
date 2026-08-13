# 首次启动、Agent 预设与 MVP 设置范围

> **部分取代。** [ADR-0012](0012-skill-man-home-binding-and-unavailability.md)
> 取代本文的固定 Library 路径、隐式创建与可跳过首次启动结论；
> [ADR-0011](0011-interface-locale-and-message-ownership.md) 取代「Preferences
> 严格四项 / 不提供语言设置」。四个既有 boolean 的行为、Agent Preset 与路径规则继续有效。

Skill Man 的首次启动应尽快进入可用状态，同时避免静默修改 Agent 目录；MVP 只暴露会显著改变后台行为或 macOS 应用形态的设置。本文补充 [符号链接策略](0003-symlink-strategy.md)、[Install 更新规则](0004-install-sources-and-updates.md)、[Adopt 流程](0005-adopt-existing-skills.md)与[应用更新策略](0006-macos-distribution-and-updates.md)。词汇遵循 [CONTEXT.md](../../CONTEXT.md)。

## Library 与首次启动

Library 固定使用 `~/Library/Application Support/skill-man/`。首次启动直接在该位置创建 Library，不询问路径；MVP 不提供 Library 路径设置或迁移能力。「应用自有位置」表示 Library 不复用任何 Agent 约定目录，不表示用户可以自选位置。后续若要支持迁移，必须另行设计 SQLite、Install 实体和 Install-source Activation 的一致搬移与回滚流程。

首次启动采用可跳过的三步短向导：

1. 欢迎并创建默认 Library；
2. 展示内置 Agent Preset 的检测状态，允许用户显式设置；
3. 按 [ADR-0005](0005-adopt-existing-skills.md) 完成只读的存量 Skill 扫描，展示结果，并由用户决定是否进入 Adopt。

跳过向导会直接进入主窗口，不创建 Agent 目录、不 Adopt，也不修改任何已有 Skill。以后仍可从 Agent 管理和 Rescan 入口完成相同行为。

## Agent Preset 与路径

Claude Code 与 Codex 两个 Agent Preset 始终展示，默认路径分别为 `~/.claude/skills` 与 `~/.codex/skills`。MVP 只根据当前配置路径是否存在判断状态：存在为可用，不存在为「未检测到」；不额外探测 CLI、应用安装状态或任意候选路径，目录缺失也不属于 Broken。

首次启动不自动创建缺失目录。「未检测到」状态提供显式的「设置」操作；只有用户确认后，Skill Man 才创建规范目录。用户可以把 Preset 覆盖为其他绝对、可写目录，并可「恢复默认路径」。Preset 是可恢复的默认配置，不是不可修改的绑定。

路径选择遵循以下安全边界：

- 解析后的路径不得是 Library、位于 Library 内，或与另一个 Agent 目标相同/互为父子；共享目录不能作为两个独立 Agent 的受管 Activation 目标。
- 目标已有内容时只读扫描为 Untracked；不自动 Adopt、不覆盖。
- Agent 存在任何 Managed Skill 的 Activation 时，阻止修改或恢复路径，列出阻塞项并要求用户先全部 Disable。MVP 不在 Agent 路径之间自动迁移 Activation，也不遗留旧路径链接。

## Preferences

MVP Preferences 只包含四个持久化开关：

| 设置 | 默认值 | 行为 |
|---|---|---|
| 登录时启动 Skill Man | 关 | 开启后在 macOS 登录时后台启动，不自动打开主窗口 |
| 在 Dock 中显示 | 开 | 立即生效；关闭后使用 Accessory/菜单栏模式，不出现在 Dock 或 `⌘Tab`，菜单栏仍可打开窗口与退出 |
| 自动检查应用更新 | 开 | 按固定 24 小时冷却后台检查；关闭后仍可手动检查；下载与安装始终需要确认 |
| 自动检查 Skill 更新 | 开 | 只检查 ADR-0004 中可跟踪的远程来源，按固定 24 小时冷却；关闭后仍可手动检查；永不自动应用 |

Agent 路径配置属于 Agent 管理界面，不属于 Preferences。下列行为在 MVP 中保持固定，不再增加设置：

- 菜单栏在应用运行时始终存在；
- 红色关闭按钮只关闭主窗口，不退出应用，`⌘Q` 或「退出 Skill Man」才终止进程；
- 启动时始终执行 Activation 健康检查和轻量 Untracked 扫描；
- 两类更新检查的冷却期固定为 24 小时，更新都由用户手动应用；
- Library 路径、主题、语言、通知和「关闭窗口时退出」不提供 MVP 设置。

## Consequences

首次启动不要求用户理解文件布局，也不会为了探测 Agent 而制造目录或改动现有 Skill；代价是需要自定义 Library 位置的用户必须等待后续迁移设计。Agent Preset 能覆盖非标准路径，但通过「先 Disable 再切换」避免半迁移、Conflict 和旧 Activation 残留，牺牲了一步到位的路径迁移体验。四个设置明确覆盖网络请求、登录项与 Dock 形态，其余行为保持固定，以限制 MVP 的状态组合与测试面。
