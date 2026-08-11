# Skill Man

一个 macOS 桌面应用:统一管理本机所有 AI agent 的 skills(技能包),并按 agent 配置启用。本文件是项目词汇表:代码、DTO 与 issue 标题使用稳定英文领域词,各 locale 的 UI 使用这里指定的显示词;本地化只改变显示,不重命名领域概念。

## Language

### Localized display terms

| Stable domain term | English UI | 简体中文 UI |
|---|---|---|
| Skill Man | Skill Man | Skill Man |
| Skill | Skill | 技能 |
| Library | Library | 技能库 |
| Library Desk | Library Desk | 技能库工作台 |
| Skill Man Home | Skill Man Home | Skill Man 主目录 |
| Home Binding | Home Binding | Skill Man 主目录绑定 |
| Legacy Home | Legacy Home | 旧版 Skill Man 主目录 |
| Fixture Recovery | Fixture Recovery | 测试数据恢复 |
| Safety Snapshot | Safety Snapshot | 安全快照 |
| Fixture Recovery Lock | Fixture Recovery Lock | 测试数据恢复锁定 |
| Agent | Agent | 智能体 |
| Agent Preset | Agent preset | 智能体预设 |
| Import | Import | 导入 |
| Link | Link | 链接 |
| Install | Install | 安装 |
| Adopt | Adopt | 纳管 |
| Managed | Managed | 已纳管 |
| Untracked | Untracked | 未纳管 |
| Enable / Disable | Enable / Disable | 启用 / 停用 |
| Activation | Activation | 启用项 |
| Broken | Broken | 已失效 |
| Modified | Modified | 已修改 |
| Conflict | Conflict | 冲突 |
| Remove | Remove | 移出技能库 |
| Preferences | Preferences | 设置 |

### Content ownership

**App Copy**:
Skill Man 拥有语义与措辞的可见或 accessible 界面内容;所有 App Copy 都必须按当前 locale 本地化。Placeholder 中的自然语言属于 App Copy,其中的路径、URL、命令、文件名与格式示例是保持原样的 technical token。

**Source Content**:
Skill、用户或外部来源提供且必须原样展示的内容,包括 Skill 名称、描述与正文、路径、URL、Git 标识、用户自定义 Agent 名称、release notes 与外部命令输出。App 只本地化包裹这些值的 App Copy。

### Domain terms

**Skill**:
一个 AI agent 技能包:一个含 `SKILL.md` 的目录。**身份 = 目录名**(唯一标识,Conflict 判定基准);`SKILL.md` frontmatter 的 name/description 仅为展示元数据。
_Avoid_: Plugin, Extension, 插件

**Library**:
Skill Man 管理全部 Managed Skill 的逻辑边界与单一可信源(single source of truth):由 Catalog 中的 Managed Skill 索引和 Skill Man Home 内的 Install 实体组成;Link 来源的实体留在外部,Library 只记录其指针。Library 不复用任何 Agent 约定目录,也不等同于 Skill Man Home 的物理根。
_Avoid_: Store, Central Repo, 中央仓库(叙述中可用「中央仓」指代,命名一律用 Library)

**Skill Man Home**:
Skill Man 的 Home-scoped 活动产品状态所属的物理根。它承载 Library 持久化内容及应用恢复所需的 Home 内状态,但不包含必须独立于活动 Home 的 App-level 控制状态,包括最小 Home Binding locator、durable recovery ledger 与 locale 选择。
_Avoid_: Library path, App Data directory

**Home Binding**:
当前应用配置与一个逻辑 Skill Man Home 身份之间的持久关联。首次确认并初始化成功后绑定不可变;Reconnect 或 Restore 同一身份不属于 Relocate,只有明确 Abandon 后才能建立新绑定。
_Avoid_: Home path setting, Library location preference

**Legacy Home**:
在 Home Binding 引入前由旧版 Skill Man 创建的固定位置 Home。它必须先被识别并完成必要恢复,才能进入唯一一次首次绑定过渡。
_Avoid_: Old Library

**Fixture Recovery**:
一次性修复被生产 fixture 污染的 Legacy Home 或 Bound Home 的流程。它不把污染项视为合法 Managed Skill,也不触发 Adopt、Remove、Enable 或 Activation Repair。
_Avoid_: Remove, Undo, Operation Recovery

**Safety Snapshot**:
Fixture Recovery 写入前隔离的完整旧 Home,用于验证与回滚,不作为活动 Home 使用。它必须由用户显式删除,不得自动清理。
_Avoid_: Migration backup, Undo backup

**Fixture Recovery Lock**:
检测到 fixture 污染或恢复状态不确定时,在用户确认、验证并提交 Fixture Recovery 前禁止全部常规产品写操作的状态;Fixture Recovery 自身经确认的受控写入是唯一例外。它与 Catalog 因 schema 或权限问题进入的 ReadOnly 访问状态不同。
_Avoid_: ReadOnly, RecoveryRequired

**Import**:
把一个 skill 收入 Library 的动作。两种方式:Link(引用本地目录)、Install(安装,实体进 Library)。
_Avoid_: Register, Add, 注册

**Link**:
Import 方式之一:引用一个在本地开发的 skill 目录 —— 实体留在原地,Library 里放指向它的符号链接。
_Avoid_: Reference

**Install**:
Import 方式之一:把 skill 实体装进 Library。两个来源:从远程安装(Git URL 等)与从文件安装(本地文件夹/压缩包拷贝)。
_Avoid_: Clone(来源不止 git), Copy

**Broken**:
状态:Library 条目存在,但其目标不可用 —— Link 来源的 skill 源目录被删除/移动,或 Activation 指向已消失的 Library 条目。
_Avoid_: Missing, Dangling

**Modified**:
状态:Install 来源的 skill 在安装或最近一次更新后被本地改动,当前内容不再等同于已记录的安装内容。长期开发中的 skill 应使用 Link,而不是维持 Modified。
_Avoid_: Dirty, Locally Modified

**Remove**:
把一个 Managed skill 从 Library 里拿掉的动作。对 Link 来源的 skill 只是断开引用(原地实体不动);对 Install 来源的 skill 是否删除实体,由「符号链接策略与冲突规则」决策。
_Avoid_: Delete(不暗示物理删除), Eject, Uninstall

**Conflict**:
状态:命名撞车。两类:Library 内重名(同名 skill 来自不同来源)、Activation 冲突(要 Enable 的 agent 目录已被同名 Untracked 实体/链接占用)。处理规则由「符号链接策略与冲突规则」决策。
_Avoid_: Name Clash, Collision

**Managed**:
状态:skill 在 Library 中、受 Skill Man 追踪。
_Avoid_: Adopted

**Untracked**:
状态:skill 存在于某个 agent 的 skills 目录中,但不在 Library 里、不受 Skill Man 追踪。
_Avoid_: Wild, 野生, External

**Adopt**:
把一个 Untracked skill 收编为 Managed 的动作(移入 Library 或登记到 Library,并在原处按规则处理)。
_Avoid_: Import(收编存量用 Adopt;Import 只用于新增入库), 收编(叙述可用,命名用 Adopt)

**Agent**:
一个 AI 编码工具(如 Claude Code、Codex),它从约定的 skills 目录加载 skill。Skill Man 以「名称 + 目录路径」描述一个 Agent;内置 Claude Code / Codex 两个 Agent Preset,也支持自定义。

**Agent Preset**:
Skill Man 内置的 Agent 初始配置,预填名称与规范 skills 目录。Preset 是可恢复的默认值,不是锁定绑定;用户覆盖路径后仍是同一个 Agent。
_Avoid_: Built-in Agent, 内置 Agent(需要强调预填配置时用 Agent Preset)

**Enable / Disable**:
把一个 Managed skill 在某个 Agent 上打开(Enable)/ 关闭(Disable)的动词对。Enable 的本质是在该 Agent 的 skills 目录创建 Activation;Disable 是移除它。
_Avoid_: Link / Unlink(Link 已用于入库方式), Mount, 挂载

**Activation**:
名词:某个 Agent 的 skills 目录里的符号链接实体,**直指 skill 最终实体**(Install 来源 → Library 目录树内;Link 来源 → 源目录),不经过 Library 指针条目串联。一个 Managed skill 可以在多个 Agent 上各有一个 Activation。
_Avoid_: Link, 启用链接
