# 桌面工作区、技能阅读与分发交互

状态：Accepted

日期：2026-09-08

实现基线：`main` 的 `bbb1fa4a2aa4f0f442de45e61bcbae9fd3af5c23`。

本 ADR 回溯记录已经实现的交互与状态边界，不提出新功能，也不代表发布或原生平台验收结论。

## 背景与替代范围

当前应用已有技能库、智能体、仓库管理三个工作区。早期原型中的临时多选工具栏、智能体页面内的独立导航列、详情区操作按钮和只读源码预览，已不能完整描述实际界面。

本 ADR 取代 [ADR-0009](0009-ui-information-architecture.md) 与 [ADR-0021](0021-agent-management-and-enable-ui-architecture.md) 中与下述布局、阅读模式、兼容性提示和面板刷新相冲突的视觉约定；延续 [ADR-0022](0022-library-file-manager-selection.md) 的文件管理器式选择。它不改变 [ADR-0016](0016-agent-configurations-global-roots-and-shared-targets.md) 的 Root/Target 权威、[ADR-0019](0019-enable-surfaces-target-resolution-and-batch-semantics.md) 的计划确认边界，以及既有 Home、Git Source Transition 与恢复规则。

## 决议

### 持久的桌面工作区结构

- 顶部主导航是技能库、智能体、仓库管理。智能体的分类切换与新建按钮仅在智能体工作区显示，复用主工具栏的控件尺寸和样式。
- 扫描范围配置、扫描报告与重新扫描属于共用的扫描状态区域，不混入智能体编辑操作。完整 Scan Report 使用非模态工作区；具体写操作再打开预览与确认弹窗。
- 仓库管理采用左侧来源列表、右侧所选来源详情与成员列表，不重复展示一整条页面标题。成员表格与来源信息共用水平边界；“创建本地副本”与成员选择操作放在一起，不改变来源级 Update/Remove 或副本创建语义。
- 弹窗限制在可用视口内，标题、进度和操作栏不随长正文滚动；正文是弹窗内的滚动区域。焦点约束和执行期间的关闭保护仍适用。

### 选择、阅读与写操作分离

- 技能库筛选依次为全部、已分发、未分发、已失效、已修改、本地、Git。来源一级分类初始收起；来源内子分类是始终展开的标题，不提供第二层折叠。非 Git 来源归入“本地来源”。
- 单击、Command/Ctrl 增减选择、Shift 连续选择和键盘行为遵循 ADR-0022；选择本身不写 Catalog。多选不重新引入复选框、批量模式入口或独立操作工具栏。
- 单选在中间阅读 Skill；多选显示选中数量和堆叠卡片，底层最多两个无正文、不可交互的轮廓，不并行渲染多份正文。
- 右侧统一使用“技能分发”标题。项目分发入口及满足原有资格的单选 Remove 入口位于标题旁。Git Source Member 不因按钮位置变化而获得单独 Remove 权限，也不新增批量 Remove。
- 来源、最近活动及技术证据集中在一个详情区域。字段名使用本地化、统一的标签样式；字段值使用统一的等宽字体。来源值为“本地 · {canonical 本地路径}”或“Git · {repository URL}”；仓库地址通过受限的外部链接能力交给系统浏览器打开。

### 分发状态不等于 Agent 执行状态

状态显示统一使用 `DistributionState`，而不是重命名持久化模型：

| 指定 Target 上所选 Skill 的 `desired` | 中文状态 | English UI | 开关 |
| --- | --- | --- | --- |
| 非空选择，全部为真 | 已分发 | Distributed | 开 |
| 全部为假，或空选择 | 未分发 | Not distributed | 关 |
| 有真有假 | 部分分发 | Partially distributed | 关 |

“部分分发”的关闭态开关被点击时，进入所选 Skill 的全局分发预览，而不是撤回已分发的部分。已分发状态下的批量撤回按 Target 执行，逐 Skill 重新计划、校验和提交，遇到失败停止并报告完成数量；不承诺整批原子性。

技能库“已分发”筛选只表示至少一个全局 Target 有分发记录，不表示全部 Target 均已分发。分发记录与链接健康独立，不能据此声称 Agent 已加载或执行 Skill。项目级分发只生成一次性结果，不持续管理项目内的链接、健康或撤回。

`Enable/Disable`、`Activation`、`desired`、`enabledAgentCount`、筛选值 `enabled/disabled` 继续作为 Core、DTO、数据库及命令兼容标识；中英文 App Copy 遵循 [CONTEXT.md](../../CONTEXT.md)，Source Content 不随界面语言改写。

### 保留面板实例，异步读取只更新当前选择

- 切换 Skill 时保留上一次完整的中间展示，等待新的单项详情就绪后再替换，避免先清空再填充。保留的内容在等待期间不可交互，不作为操作对象。
- 右侧 Target 卡片按稳定 Target ID 保留 DOM，不以 Skill ID 为整个面板的挂载键。选择变化仍重新读取对应状态；“不闪烁”不是“不再渲染”或复用旧操作数据。
- 面板快照记录所属选择。当前选择与快照不一致时关闭操作资格，晚到的旧选择响应不得覆盖当前状态。多选计划始终使用当前选中的稳定 Skill ID。
- 正文默认由 Markdown/GFM 渲染；阅读模式移除头部 frontmatter，RAW 模式显示收到的原始文档文本。阅读器跳过原始 HTML、不加载图片资源，并只为 HTTP(S) 地址生成链接，不赋予 Source Content 脚本或任意本地文件访问能力。

### 智能体配置与检测分开呈现

- 已配置与预设模板保留列表/详情结构；已检测但未配置使用全宽卡片列表，不渲染无用的右侧详情占位。
- 检测卡片使用 flex 换行，按容器宽度统一计算列宽，最大宽度为 400 CSS px。完整行 `space-between`；末行补不可见且不可访问的空槽，保持左对齐和跨行列位置一致。同一行卡片拉伸等高。
- Codex 是独立预设；通用目录 `~/.agents/skills` 与 `.agents/skills` 归入 General 预设，不作为其它预设的通用 Root。既有用户配置不因预设调整自动改写。
- 自定义 Agent 不展示“兼容性未知”警告，也不以兼容性标签阻止正常配置或分发。存储中的 `Compatibility::Unknown` 可以保留，表示没有内置预设的验证证据；它不是配置不可用的判定。路径、写权限与 Target 冲突检查照常执行。

### 桌面尺寸与系统外观

- 主窗口初始为 1280 × 760，最小为 1280 × 720 逻辑像素；恢复或程序化 resize 也依据当前显示比例执行最小尺寸约束。组件的窄布局仍用于测试和兼容场景，不等于取消原生窗口下限。
- 使用窗口状态插件保存和恢复尺寸、位置；关闭主窗口时先保存再隐藏，应用继续驻留菜单栏。这里不承诺全屏、最大化或其它未保存状态的恢复。
- 深浅色通过 `prefers-color-scheme` 与统一颜色变量跟随系统，不新增独立的手动主题偏好。应用头部使用主 Logo，菜单栏使用 18 × 18 单色 template 图标，由系统适配背景；主应用打包图标与菜单栏图标承担不同用途。

## 取舍与后果

复用操作面板减少布局跳动，但必须隔离旧展示与当前操作资格，不能省略重新校验。原生最小尺寸保证常用桌面布局可用，但不保证任意缩放或所有小屏幕都能容纳完整三栏。

Markdown 阅读优先于源码展示，同时保留 RAW 和 Source Content 边界。渲染限制意味着部分 HTML、图片及非 HTTP(S) 链接不会按原网页展示，这是只读阅读器的能力范围。

本 ADR 不替代 Core 的 ownership、WriteGate、预览 token 或恢复校验，也不把一组独立写操作描述为事务。

## 实现与回归证据入口

- 工作区和分发：[LibraryDesk.tsx](../../src/features/library/LibraryDesk.tsx)、[SourceGroupCard.tsx](../../src/features/library/SourceGroupCard.tsx)、[GlobalTargetGroups.tsx](../../src/features/library/GlobalTargetGroups.tsx)、[distribution-state.ts](../../src/features/library/distribution-state.ts)。
- 选择和阅读：[library-selection.ts](../../src/features/library/library-selection.ts)、[SkillSelectionStack.tsx](../../src/features/library/SkillSelectionStack.tsx)、[SkillDocument.tsx](../../src/features/library/SkillDocument.tsx)。
- 智能体：[AgentManagement.tsx](../../src/features/agents/AgentManagement.tsx)、[DetectionCardList.tsx](../../src/features/agents/DetectionCardList.tsx)、[agent_configuration.rs](../../src-tauri/src/core/agent_configuration.rs)。
- 桌面和样式：[lib.rs](../../src-tauri/src/lib.rs)、[tauri.conf.json](../../src-tauri/tauri.conf.json)、[styles.css](../../src/styles.css)。
- 回归覆盖：[LibrarySelection.test.tsx](../../src/features/library/LibrarySelection.test.tsx)、[GlobalTargetGroups.test.tsx](../../src/features/library/GlobalTargetGroups.test.tsx)、[SkillDocument.test.tsx](../../src/features/library/SkillDocument.test.tsx)、[DetectionCardList.test.tsx](../../src/features/agents/DetectionCardList.test.tsx)、[LayoutContract.test.tsx](../../src/app/LayoutContract.test.tsx)、[window_configuration.rs](../../src-tauri/tests/window_configuration.rs)。这些是可执行证据入口，不代表所有平台的人工验收声明。
