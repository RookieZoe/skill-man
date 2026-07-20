# Skill Man

一个 macOS 桌面应用:统一管理本机所有 AI agent 的 skills(技能包),并按 agent 配置启用。本文件是项目词汇表 —— 所有产出(issue 标题、UI 文案、代码命名、提案)必须使用这里的规范词汇。

## Language

**Skill**:
一个 AI agent 技能包:一个含 `SKILL.md` 的目录。**身份 = 目录名**(唯一标识,Conflict 判定基准);`SKILL.md` frontmatter 的 name/description 仅为展示元数据。
_Avoid_: Plugin, Extension, 插件

**Library**:
Skill Man 自有的中央仓:一个应用自定义位置的目录树(Install 来源的 skill 实体所在)加索引(Link 来源 skill 的指针)。本机全部 Managed skill 的单一可信源(single source of truth),不复用任何 agent 体系的约定目录。
_Avoid_: Store, Central Repo, 中央仓库(叙述中可用「中央仓」指代,命名一律用 Library)

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
_Avoid_: Missing, Dangling, 失效(叙述可用,命名用 Broken)

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
一个 AI 编码工具(如 Claude Code、Codex),它从约定的 skills 目录加载 skill。Skill Man 以「名称 + 目录路径」描述一个 Agent;内置 Claude Code / Codex 两个预设,也支持自定义。

**Enable / Disable**:
把一个 Managed skill 在某个 Agent 上打开(Enable)/ 关闭(Disable)的动词对。Enable 的本质是在该 Agent 的 skills 目录创建 Activation;Disable 是移除它。
_Avoid_: Link / Unlink(Link 已用于入库方式), Mount, 挂载

**Activation**:
名词:某个 Agent 的 skills 目录里的符号链接实体,**直指 skill 最终实体**(Install 来源 → Library 目录树内;Link 来源 → 源目录),不经过 Library 指针条目串联。一个 Managed skill 可以在多个 Agent 上各有一个 Activation。
_Avoid_: Link, 启用链接
