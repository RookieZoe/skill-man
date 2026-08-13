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
| Unconfigured | Unconfigured | 未配置 |
| Home Candidate | Home candidate | 候选主目录 |
| Bound Home | Bound Home | 已绑定主目录 |
| Reconnect Same Home | Reconnect Same Home | 重新连接同一主目录 |
| Restore Bound Home | Restore Bound Home | 恢复已绑定主目录 |
| Abandon Home and Start New | Abandon Home and Start New | 放弃主目录并重新开始 |
| HomeUnavailable | Home Unavailable | 主目录不可用 |
| HomeIdentityMismatch | Home Identity Mismatch | 主目录身份不匹配 |
| AppStateUnavailable | App State Unavailable | 应用状态不可用 |
| Catalog ReadOnly | Catalog read-only | 技能库只读 |
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

**Unconfigured**:
首次绑定前的顶层状态:没有 Home Binding,App 只提供绑定向导与 App-level 状态;取消或未完成绑定不产生任何 Home 内容。
_Avoid_: First Run, 首次运行

**Home Candidate**:
首次绑定流程中已通过只读校验、等待用户显式确认并原子提交的候选路径;确认前不创建任何 Home 内容。
_Avoid_: chosen path, 所选路径

**Bound Home**:
已建立不可变 Home Binding、身份校验通过的 Home;绑定后不存在 Preferences 改址、Relocate 或普通 re-home。
_Avoid_: active Library, 当前主目录

**Reconnect Same Home**:
HomeUnavailable / HomeIdentityMismatch 下由用户显式发起、对同一 home_id 重新校验并恢复正常访问的动作;不属于 Relocate。
_Avoid_: Retry mount, 重新挂载

**Restore Bound Home**:
同一 home_id 下,对内容验证失败的 Bound Home 执行 Fixture Recovery 状态机、以 Safety Snapshot 可逆恢复内容的动作;不属于 Relocate。
_Avoid_: Reset, 重置主目录

**Abandon Home and Start New**:
产生新 Home Identity 的唯一高摩擦逃生口:输入确认、不删除旧 Home、不清理旧 Activation,旧 home_id 永久记入 locator 历史。
_Avoid_: Delete Home, 删除主目录

**HomeUnavailable**:
Home Binding 存在但 Home 无法访问(卷离线、权限丢失、目录消失)的顶层状态:fail-closed,不写不绑,只读能力按实际可用性提供。
_Avoid_: Missing, Offline

**HomeIdentityMismatch**:
路径可达但 binding、Home marker、Catalog 与卷身份不一致的顶层状态:现场内容绝不视为 Bound Home,唯一逃生口是 Abandon。
_Avoid_: Wrong Home, 换了目录

**AppStateUnavailable**:
App-level 状态目录或 bootstrap locator 不可读、损坏或自相矛盾时的顶层状态:既不当作 Unconfigured 也不当作 Bound,禁止产品写与新建绑定。
_Avoid_: Corrupted state, 状态损坏

**Home Identity (home_id)**:
逻辑 Home 身份:首次初始化生成的稳定标识(UUID v4),同时记录在 bootstrap locator、Home marker 与 Catalog SQLite 中;绑定、Reconnect、Restore 与 Abandon 都以它而非路径字符串为判定基准。
_Avoid_: Home UUID, Home path

**bootstrap locator**:
Home 外(状态目录中)的最小持久记录,是绑定状态的唯一 truth;记录 home_id、路径、卷身份与 abandoned 历史,以 tmp→fsync→rename→parent fsync 原子提交。
_Avoid_: Home setting, bookmark, 路径设置

**Home marker**:
Home 根部的身份文件,记录 home_id 与卷身份;与 bootstrap locator、Catalog SQLite 三方一致才证明同一 Home。
_Avoid_: home.json, 身份文件

**Catalog ReadOnly**:
Catalog 因 Home 不可用或 schema/权限问题进入的只读访问状态;与 Fixture Recovery Lock 不同,后者禁止全部常规产品写并走专用恢复流程。
_Avoid_: Locked catalog, 只读锁定

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

**Verified Remote Source**:
由外部 lock 线索、最终实体、规范化 remote、requested ref、resolved commit、仓库内 Skill 路径与本地/远端 tree 共同形成可复核闭环的来源。外部 lock 只是 provenance hint;只有闭环成立,Adopt 才能把当前内容认领为 Remote Install。
_Avoid_: Trusted lock, Lock-managed Skill

**Remote Source Parent**:
Library 中代表一个 remote repository 的稳定来源聚合;内部 remote_id 不随 URL 重命名、requested ref 或 resolved commit 改变。它只承载 durable provenance 与所属 Skill 清单;requested ref、resolved commit、skillPath 和内容 baseline 属于各 Skill,Git mirror 属于可重建 cache,Skill 实体只存在于 `skills/`。
_Avoid_: Git checkout, Worktree, Mirror

**Remote Binding**:
一个 Remote Install 对其 Remote Source Parent 的独立版本关系,记录 requested ref、Verification Anchor、skillPath 与 remote/content baseline。Remote Source Parent 没有单一当前 commit;同一 remote 的多个 Skill 可位于不同 commit,fetch 可共享,Preview 与提交按 Skill 独立。
_Avoid_: Parent version, Repository checkout

**Verification Anchor**:
Adopt 验证时在 requested ref 可达历史中确定、且其 skillPath tree 与外部 lock hash 和本地内容形成闭环的 Git commit。它是后续 materialize/update 的确定锚点,不冒充外部 installer 未记录的原始安装 commit;若多个 commits 的 Skill tree 相同,使用最新匹配 commit并明确标记原安装 commit 未知。新 Remote Install 仍直接记录实际 resolved commit。
_Avoid_: Original install commit, Guessed commit

**Provenance Conflict**:
外部 lock 声称某个 Skill 有 remote 来源,但来源闭环缺失或证据矛盾的 Adopt 状态。它默认保持 Untracked 并阻止自动降级;用户查看证据后可显式忽略该 lock,再按 Local Link 路径处理。
_Avoid_: Invalid lock(只描述文件,没有表达领域阻塞状态), Local Skill

**Verification Deferred**:
外部 lock 的结构与已知证据尚未矛盾,但网络离线、认证失败或 remote 服务暂时故障使 Verified Remote Source 闭环暂时无法完成的 Adopt 状态。它保持 Untracked,可 Retry 或由用户显式忽略 lock 后转 Local Link;不得自动降级。
_Avoid_: Provenance Conflict, Offline Skill

**Local Source**:
未被认定为 Verified Remote Source、由用户继续拥有的 Skill 最终实体。Adopt 以 Link 认领;实体必须位于 Skill Man Home、Agent/shared skills 根与 installer-managed 根之外,否则先由用户显式选择稳定位置并完成可回滚迁出。
_Avoid_: Unverified Remote, File Install

**Ownership Handoff**:
Verified Remote Source 从外部 installer 转交给 Skill Man 的显式 Adopt 边界。每个 Skill 独立提交:保持当前字节、使 Home 内实体与 Catalog/Activation 生效,并以 compare-and-swap 退出对应外部 lock 所有权;任何一步失败都回滚该 Skill,外部状态并发变化则停止剩余未提交项。
_Avoid_: Import, Sync

**Ownership Conflict**:
Ownership Handoff 后外部 installer 又为同一 Skill 身份创建 lock 条目或实体的状态。Skill Man 保留既有 Managed Skill,不自动合并、覆盖或重新接管;用户必须明确保留哪一方后再移除或重新 Adopt。
_Avoid_: Update available, Activation Conflict

**Remote Source Identity Conflict**:
Remote Source Parent 的 `source.json` 与 Catalog parent row 缺失或不一致,无法证明同一 remote_id 与 canonical URL 的状态。影响范围限于该 parent:子 Skill 可读并可 Disable/Remove,但 Update、新 Remote Binding 与 alias 变更 fail closed,直到通过 remote 与全部子 binding 重新验证。
_Avoid_: HomeIdentityMismatch, Catalog ReadOnly

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
