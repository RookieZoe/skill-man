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
| Recovery Profile | Recovery Profile | 恢复配置特征 |
| Existing Home Recovery Plan | Existing Home Recovery Plan | 既有主目录恢复计划 |
| Default Home Recovery Offer | Default Home Recovery Offer | 默认主目录恢复提示 |
| Default Home Recovery Blocked | Default Home Recovery Blocked | 默认主目录恢复受阻 |
| Git Repository Source | Git Repository Source | Git 仓库来源 |
| Source Release | Source Release | 来源版本 |
| Source Member | Source Member | 来源成员 |
| Directory Identity | Directory identity | 目录身份 |
| Source Tracking Policy | Source tracking policy | 来源跟踪策略 |
| Source Snapshot Mismatch | Source snapshot mismatch | 来源快照不一致 |
| Restore Current Source Release | Restore current source release | 恢复当前来源版本 |
| Source Member Tombstone | Source member tombstone | 来源成员墓碑 |
| Create Local Source Copy | Create local source copy | 创建本地来源副本 |
| Source Transition | Source Transition | 来源切换 |
| Repository Ownership Split | Repository Ownership Split | 仓库所有权分裂 |
| Source Transition Journal | Source Transition Journal | 来源切换日志 |
| Source Ownership Commit Point | Source Ownership Commit Point | 来源所有权提交点 |
| Source Undo | Source Undo | 来源撤销 |
| Source Transition Preflight | Source Transition Preflight | 来源切换预检 |
| Source Group Preview | Source Group Preview | 来源组预览 |
| External Ownership Claim | External Ownership Claim | 外部所有权线索 |
| Source Group Draft | Source Group Draft | 来源组草案 |
| Source Group Confirmation | Source Group Confirmation | 来源组确认 |
| Member Diff Summary | Member Diff Summary | 成员差异摘要 |
| Legacy Per-Skill Git State | Legacy Per-Skill Git State | 旧逐成员 Git 状态 |
| Source Promotion | Source Promotion | 来源提升 |
| Source Capability Scan | Source Capability Scan | 来源能力扫描 |
| Repository Ref Conflict | Repository Ref Conflict | 仓库 ref 冲突 |
| Fetch Latest and Manage | Fetch Latest and Manage | 获取最新并纳管 |
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
| Custom Agent | Custom Agent | 自定义智能体 |
| Global Skills Root | Global skills root | 全局技能根目录 |
| Scan Appearance | Scan appearance | 扫描出现位置 |
| Canonical Skill Entity | Canonical skill entity | 规范技能实体 |
| Scan Coverage | Scan coverage | 扫描覆盖 |
| Scan Run | Scan run | 扫描运行 |
| Scan Report | Scan report | 扫描报告 |
| Scan Incomplete | Scan incomplete | 扫描不完整 |
| Startup Probe | Startup probe | 启动探测 |
| Git Repository Source Candidate | Git repository source candidate | Git 仓库来源候选 |
| Conflict Set | Conflict set | 冲突集 |
| Agent Detection | Agent detection | 智能体检测 |
| Agent Configuration | Agent configuration | 智能体配置 |
| Agent Activation Target | Agent activation target | 智能体启用目标 |
| Activation Target Group | Activation target group | 启用目标组 |
| Activation Health Observation | Activation health observation | 启用健康观察 |
| Resolved Project Skills Directory | Resolved project skills directory | 解析后的项目技能目录 |
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
一个 AI agent 技能包：一个含 `SKILL.md` 的目录。Managed Skill 使用稳定 `skill_id`；目录名是 Directory Identity，`SKILL.md` frontmatter 的 name/description 仅为展示元数据。
_Avoid_: Plugin, Extension, 插件

**Directory Identity**:
Skill 目录名经 `NFC + Unicode casefold` 得到的比较键。它决定 Activation 在平面 Agent Activation Target 中占用的 entry name，但不是持久 Managed Skill identity；同名 Git Source Member 可按来源和 `skillPath` 区分。
_Avoid_: Skill ID, Display name, frontmatter name

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
有效且结构完整的 App-level state 中没有 current Home Binding、abandoned history 或 active recovery ledger 的顶层状态。它可以是首次绑定前,也可以是 App-level state 丢失后重新形成的空状态;它本身不证明从未存在 Home。App 只提供显式绑定/恢复入口与 App-level 状态;取消或未完成操作不产生任何 Home 内容。
_Avoid_: First Run, 首次运行

**Recovery Profile**:
用于 Recover Existing Home 的只读结构证明:以 Home marker 与 Catalog 的逻辑 home_id、一致的创建时间、Catalog 完整性/外键检查，以及当前产品所需的真实目录、表、列和约束为准，而非以可编辑的 schema_version 或 Home 所在卷的 UUID 为准。SQLite WAL/SHM 的存在本身不构成失败;但未完成的 operation journal 或未归属 staging 内容必须转入操作恢复，不能直接恢复既有 Home。缺少任何必需能力时拒绝恢复并保持零写入。
_Avoid_: schema version gate, volume identity proof

**Existing Home Recovery Plan**:
由 Recovery Profile 形成、供用户直接确认的一次性既有 Home 恢复计划。它在 App-level state 或已证明的 Home 事实变化时失效；确认前取消或失效不产生任何写入，确认后则产生不可撤回的 Home Binding。不同于会创建或迁移内容的 Home Candidate，也不产生待清理的 Home 产物。
_Avoid_: Home Candidate, durable recovery operation

**Default Home Recovery Offer**:
在有效 Unconfigured App-level state 下，默认路径中完整既有 Home 已通过 Recovery Profile 时提供的显式恢复入口。它只是对已发现 Home 的提示，不是 Home Binding，也不会自动写入或恢复内容；确认仍使用 Existing Home Recovery Plan。该入口只允许恢复此 Home；取消后仍返回此提示，若要新建 Home 必须先恢复并执行 Abandon Home and Start New。
_Avoid_: AppStateUnavailable, automatic rebind

**Default Home Recovery Blocked**:
在有效 App-level state 下，默认路径具有既有 Home 痕迹但未通过 Recovery Profile，或必须先处理未完成 operation 时的 closed state。它提供诊断与重试，但不提供恢复、首次绑定或改选路径入口。
_Avoid_: AppStateUnavailable, Unconfigured

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

**Recover Existing Home**:
在 Unconfigured 下,用户显式选择一个满足 Recovery Profile 的完整既有 Home,审阅其不可变恢复事实后直接确认，由 marker 与 Catalog 证明同一 home_id 并重建丢失的 bootstrap locator 以恢复该 home_id 的动作。它不改写 Home 内容;候选被拒绝或用户取消时保持 Unconfigured,不写 locator、Home、recovery ledger 或 Catalog。locator 提交后不回滚；随后无法访问、身份不匹配或 Catalog 打开失败按既有已绑定状态收敛并关闭写操作。AppStateUnavailable、Legacy/Fixture Recovery Lock、Home Candidate Pending 和任何可读的 Abandoned history 都不能进入此流程。若历史随 App-level state 一同丢失,确认必须明确提示该恢复可能重新激活曾被 Abandon 的 Home。
_Avoid_: Reconnect Same Home, Restore Bound Home, Relocate

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
App-level 状态目录或 bootstrap locator 不可读、损坏或自相矛盾时的顶层状态:既不当作 Unconfigured 也不当作 Bound,禁止产品写与新建绑定。有效状态中 locator 缺失本身不构成 AppStateUnavailable。
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
Import 方式之一：引用一个用户拥有的 Local Source；实体留在原地，Catalog 只记录 canonical 最终实体路径，Home 不创建第二条 Library 指针软链。
_Avoid_: Reference, Activation

**Install**:
Import 方式之一:把 skill 实体装进 Library。两个来源:从远程安装(Git URL 等)与从文件安装(本地文件夹/压缩包拷贝)。
_Avoid_: Clone(来源不止 git), Copy

**Broken**:
状态：Local Source 最终实体不可用，或全局 Activation 的目标不存在。Git Source Member 从当前 Source Release 消失时，其快照路径消失且既有全局 Activation 进入 Broken；项目级一次性软链不在此状态模型中。
_Avoid_: Missing, Dangling

**Modified**:
状态：由 File Install 或仍受旧非 Git contract 管理的 Install 在安装后被本地改动，当前内容不再等同于已记录内容。Git Source Member 是不可变快照，不使用 Modified；长期开发使用 Local Source。
_Avoid_: Dirty, Locally Modified

**Remove**:
把 Managed Skill 或完整 Git Repository Source 从 Library 拿掉的动作。Link 只删除 Catalog 引用、原地实体不动；非 Git Install 删除其实体；Git Source Member 不可单独 Remove，只能 Remove 所属完整来源。
_Avoid_: Delete(不暗示物理删除), Eject, Uninstall

**Conflict**:
状态：非 Git Import/Adopt 违反既有 Library Directory Identity 规则，或 Enable 时 Agent Activation Target 的同名 entry 已被占用。Git Source Member 之间或 Git 与其它 Managed Skill 同名本身不是 Library Conflict；它们只有发布到同一 Target 时才冲突。
_Avoid_: Name Clash, Collision

**Conflict Set**:
一次 Scan Report 中，具有同一 Directory Identity、但指向不同 Canonical Skill Entity 的非 Git 候选集合。Local 候选可以显式选择一个 winner；Git Source Member 的同名关系按来源分组，不进入该 Library Conflict Set。
_Avoid_: Duplicate Skill, Merge Group

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
由外部 lock 线索、最终实体、规范化 remote 与内容证据形成可复核闭环的来源判定。对受支持 Git provider,它至多说明旧逐成员证据，不构成当前 Git Repository Source 或 Source Release；当前来源只能由 Fetch Latest and Manage 的完整远端发现建立。
_Avoid_: Trusted lock, Lock-managed Skill

**Remote Source Parent**:
ADR-0013 的历史逐 Skill 来源聚合，内部 `remote_id` 不随 URL 重命名改变。对受支持 Git provider，已有 Remote Source Parent 只属于 Legacy Per-Skill Git State；新的 Git Repository Source 另行记录 durable identity、Source Tracking Policy 与当前 Source Release。非 Git sourceType 保持 ADR-0013 的既有语义；Git mirror 属于可重建 cache。
_Avoid_: Git Repository Source, Git checkout, Worktree, Mirror

**Remote Binding**:
ADR-0013 的历史逐 Skill 绑定，保存 requested ref、Verification Anchor、skillPath 与内容 baseline。对受支持 Git provider，它只属于 Legacy Per-Skill Git State，不能作为 Git Repository Source 的成员或 Source Release 事实；非 Git sourceType 保持 ADR-0013 的既有语义。
_Avoid_: Git Repository Source member, Source Release fact, Repository checkout

**Git Repository Source**:
以受支持 Git provider、规范化 Git repository identity 和稳定 `remote_id` 定义的来源聚合。它的 Source Tracking Policy 在每个 Source Release 中选择一个 ref/tag 并解析为 commit；全部 Source Member 共同 Update，成员不可单独 Update 或 Remove。
_Avoid_: Per-Skill Git source, Git checkout

**Source Tracking Policy**:
Git Repository Source 选择下一 Source Release 的持久规则：默认依次使用最新正式 provider Release、最高稳定 SemVer tag、default branch 可达的最新普通 tag，最后才 fallback `HEAD`；用户可显式覆盖为 prerelease channel、固定 tag/commit、branch 或 `HEAD`。
_Avoid_: Source Release, Tracking ref, Always HEAD

**Git Repository Source Candidate**:
Scan Report 中按 provider 与规范化 repository identity 聚合的来源线索;它可汇合 bounded worktree evidence 与 External Ownership Claim,但在 Fetch Latest 发现完整 Source Release 前不是 Git Repository Source。
_Avoid_: Verified repository, Source Release

**GitHub Repository Source**:
以 `github` provider 定义的 Git Repository Source。它遵循全部 Git 仓库级成员、release 与更新语义，不形成 GitHub 专有的逐成员例外。
_Avoid_: GitHub-specific source model, per-Skill GitHub source

**Source Release**:
Source Tracking Policy 一次求值得到的 selected ref/tag、resolved commit、完整 Source Member 清单与 tree manifest。它不是 provider Release；resolved commit 是不可变版本事实，成员没有独立版本，也不从旧 lock 或本地内容推断。
_Avoid_: GitHub Release, GitLab Release, Remote Binding version, install commit

**Source Transition**:
Git Repository Source 对一个完整 Source Release 的整体交接或更新。它的成员、来源版本与所有权状态只能共同进入目标 release 或共同保持原状；一旦外部所有权已释放，恢复只能收敛到完整目标 release，不能留下部分成员处于该 release。
_Avoid_: Per-Skill update, partial source release

**Repository Ownership Split**:
同一 Git Repository Source 的旧外部声明跨越多个 installer lock 文件的状态。它不是多个 Git 来源；但由于无法以单一外部所有权变更完成整体交接，Fetch Latest and Manage 必须拒绝并保持零写入，直到用户显式收敛到一个稳定 external installer root。
_Avoid_: Multiple Git sources, sequential multi-lock handoff

**Source Transition Journal**:
固定一个 Source Transition 的原来源与目标 Source Release、完整成员动作和恢复事实的持久记录。它在外部所有权提交点前支持回到原状；该点之后只允许把整个来源收敛至 journal 中固定的目标 release，且不重新解释远端的“最新”。
_Avoid_: Per-Skill journal, recover to current tip

**Source Ownership Commit Point**:
Source Transition 中将单一 external installer lock 文件的全部适用旧声明作为整体释放的不可逆边界。此前失败可以保持原状；此后只能按 Source Transition Journal 完成整个来源，普通写入保持关闭。
_Avoid_: Per-entry commit point, rollback after external release

**Source Undo**:
结果窗口内对一个已完成 Source Transition 的条件性整体逆转。只有全部成员、来源状态和旧外部声明都仍可安全恢复时才恢复先前完整 release；任一 guard 失败即整体拒绝并保持现状。普通 Remove 不构成 Source Undo，也不恢复 external owner。
_Avoid_: Per-Skill undo, ordinary Remove

**Source Transition Preflight**:
在 Source Transition 确认时及 Source Ownership Commit Point 前，对固定目标 release、成员、来源状态、外部所有权与安全内容事实进行的完整重验。任一事实变化即令计划失效并保持提交点前的零写入；提交点之后由固定 Source Transition Journal 收敛。
_Avoid_: Trust stale preview, volume UUID gate

**Source Group Preview**:
对一个 Git Repository Source 的只读 Source Transition 审阅，父节点展示来源与 release 事实，完整 Source Member 集在其下按动作和状态呈现。成员资格不可通过逐项选择裁剪；只有全部阻塞项已处理后才能进行来源级确认。
_Avoid_: Per-Skill Include preview, partial source selection

**External Ownership Claim**:
旧 installer lock 对外部实体所有权的显示线索，说明交接将影响的 lock 文件与声明；它不证明旧本地内容、成员路径或远端 provenance 已被验证。
_Avoid_: Verified Remote Source, remote baseline

**Source Snapshot Mismatch**:
Git Source Member 的 Home 快照字节不等于 current Source Release tree hash 的 closed state。它不是 Modified；Update、新 Enable 与普通来源写保持关闭，但只读查看、Disable、把当前观察字节 Create Local Source Copy，以及显式 Restore Current Source Release 仍可用。
_Avoid_: Modified, Local changes, Auto overwrite

**Restore Current Source Release**:
用户明确丢弃 Source Snapshot Mismatch 字节并从已记录 current Source Release 重新物化完整来源快照的恢复动作。它不获取更新版本，也不改变 Source Tracking Policy。
_Avoid_: Update, Repair Activation, Silent overwrite

**Source Member Tombstone**:
一个已从 current Source Release 消失、但为保留稳定 `(remote_id, skillPath, skill_id)` 和全局 Activation ownership 而留下的最小记录。它在整个 Git Repository Source 生命周期内保留；相同路径重新出现时复用原 `skill_id`，相关 Activation ownership 在用户 Disable 前持续可读。
_Avoid_: Managed current member, Local backup

**Source Group Draft**:
Source Group Preview 中对完整目标 Source Release、tracking policy 选择和来源级冲突处置的可修改集合。它不是 Source Transition，也不产生来源、Home、Catalog、stage、journal 或 lock 的持久变化；成员资格不可逐项裁剪。
_Avoid_: Per-member apply, pending operation

**Source Group Confirmation**:
在所有来源级阻塞和成员冲突已处置后，对完整 Source Group Draft 的一次显式确认。它同时确认全部成员动作和外部所有权影响，随后才可开始 Source Transition。
_Avoid_: Per-Skill confirm, implicit approval

**Member Diff Summary**:
Source Group Preview 中供审阅 Source Member 状态和动作的最小事实：当前/目标 `skillPath`、Directory Identity、目标 tree 摘要，以及 added/current/removed 状态。它可展开为安全的受管内容差异，但不把旧外部内容作为远端基线或允许成员级 opt-out。
_Avoid_: External content proof, remote baseline, Modified member choice

**Legacy Per-Skill Git State**:
既有 Remote Source Parent/Binding 仅拥有逐成员 ref、commit 与 baseline、但不具备 Git Repository Source 的共同 release 与成员集事实的状态。它以实际 Catalog 与 manifest 能力识别，不以 schema version 识别；保持安全读取和维护能力，但只能经用户显式的来源组升级进入新模型。
_Avoid_: Current Source Release, automatic source migration

**Source Promotion**:
用户显式发起、将无歧义的 Legacy Per-Skill Git State 提升为 Git Repository Source 的 Source Transition。它保留已有 remote_id，确认后才可执行必要的结构准备与数据转换，并必须通过新的完整 Source Release 发现建立共同成员事实；旧逐成员证据只是历史 baseline 与冲突输入，不能充当当前 release。
_Avoid_: Startup migration, inferred source release

**Source Capability Scan**:
对 Catalog 实际表、列、约束与来源 manifest 所做的只读能力检查，用于判定 Git 来源能否进入 Source Promotion。它不依赖 schema version，不写入、不推断 Source Release，也不把缺失或部分能力自动修复为可提升状态。
_Avoid_: Schema version gate, automatic source repair

**Source Member**:
Git Repository Source 中由 `(remote_id, repository-relative skillPath)` 识别、并关联稳定 `skill_id` 的 Managed Skill 成员。它从 Source Release 取得成员资格和版本，当前字节是 `<Home>/skills/git/<remote_id>/<skill_id>/` 的不可变快照；路径变化是删除加新增，同一路径重现则恢复原成员。
_Avoid_: Independent Git source, per-Skill release, Editable install

**Repository Ref Conflict**:
同一规范化 Git repository 的旧 lock 声明提出多个 ref 时的 fail-closed 状态。它不拆分来源，也不验证旧内容；用户必须显式选择一个 ref，以该 ref 当前的 Source Release 建立新来源。
_Avoid_: multiple Git sources, inferred ref

**Fetch Latest and Manage**:
把 Git Repository Source Candidate 提升为 Git Repository Source 的显式操作。系统按 Source Tracking Policy（或用户显式 override）选择 ref/tag、解析 commit 并发现完整 Source Release；worktree hint、旧 lock、本地 HEAD/dirty bytes 和旧 Home 内容都不提供当前成员或 baseline。远端不能完成获取和发现时，来源不成立。
_Avoid_: Revalidate old lock, trust old bytes

**Verification Anchor**:
旧逐成员 Git 验证时在 requested ref 可达历史中确定、且其 skillPath tree 与外部 lock hash 和本地内容形成闭环的 Git commit。它只保留为 Legacy Per-Skill Git State 的历史证据或冲突输入，不能充当 Git Repository Source 的当前 Source Release，也不能驱动来源组 Update。
_Avoid_: Original install commit, Guessed commit

**Provenance Conflict**:
worktree、外部 lock、repository、member、ref 或 owner 证据互相矛盾的 Adopt 状态。它保持 Untracked 并阻止自动降级;只有修复证据,或精确处理 applicable external claim 后,才能重新分类。
_Avoid_: Invalid lock(只描述文件,没有表达领域阻塞状态), Local Skill

**Verification Deferred**:
已知来源证据尚未矛盾,但网络离线、认证失败或 remote 服务暂时故障使 Source Release 获取与发现暂时无法完成的 Adopt 状态。它保持 Untracked 并可 Retry;不得创建部分来源或自动降级。
_Avoid_: Provenance Conflict, Offline Skill

**Local Source**:
由用户继续拥有、只以真实 canonical 路径登记的 Skill 最终实体。它可以位于用户的 Git 开发工作区；版本控制 metadata 不改变 ownership。Skill Man 不保存 Local Source 内容 baseline 或判断内容变化；稳定外部实体以 Link 原地认领，实体若仍在 Home、Global Skills Root 或 installer root 中则须先显式迁到这些控制区之外。
_Avoid_: Unverified Remote, File Install

**Create Local Source Copy**:
把一个 Git Source Member 当前稳定观察字节复制到用户选择的外部目录，并把该 canonical 最终实体路径登记为新的 Local Source 的显式动作。原 Git 来源/成员与 Activation 不自动改变，副本不含 `.git`；需要 Git workspace 时由用户用外部工具 clone 后再 Link。
_Avoid_: Fork, Detach Source Member, Clone

**Ownership Handoff**:
外部 installer 向 Skill Man 转交所有权的显式边界。对 Git Repository Source，它只能作为完整 Source Transition 提交：全部适用 external claim 以一次 compare-and-swap 共同释放，之前整体回滚，之后整体收敛到固定 Source Release；非 Git sourceType 保持其既有逐项语义。
_Avoid_: Import, Sync

**Ownership Conflict**:
Ownership Handoff 后外部 installer 又为同一 Skill 身份创建 lock 条目或实体的状态。Skill Man 保留既有 Managed Skill,不自动合并、覆盖或重新接管;用户必须明确保留哪一方后再移除或重新 Adopt。
_Avoid_: Update available, Activation Conflict

**Remote Source Identity Conflict**:
Remote Source Parent 的 `source.json` 与 Catalog 来源记录缺失或不一致，无法证明同一 remote_id 与规范化 repository identity 的状态。影响范围限于该 parent：子 Skill 可读并可 Disable/Remove，但 Git Repository Source 的 Update、Source Promotion、成员变更与 alias 变更 fail closed，直到通过完整 Source Release 重新发现和验证。
_Avoid_: HomeIdentityMismatch, Catalog ReadOnly

**Agent**:
一个从一个或多个 Global Skills Root 加载 Skill 的 AI 编码工具。Agent 身份独立于 Root;它通过 Agent Configuration 取得扫描范围、Agent Activation Target 与可空的项目级目录约定。

**Agent Preset**:
Skill Man 内置的已知 Agent 配置模板,提供稳定 preset key、默认名称、Global Skills Root、Agent Activation Target 与项目级目录约定。Preset 始终可用于创建配置,但创建出的 Agent Configuration 与 Custom Agent 一样可增删改;检测不会自动重建已删除配置。
_Avoid_: Built-in Agent, 内置 Agent(需要强调预填配置时用 Agent Preset)

**Custom Agent**:
用户在 Agent 管理中创建的 Agent Configuration,包含稳定身份、显示名、Global Skills Root、Agent Activation Target 与可空的项目级目录约定。Custom Agent 与由 Preset 创建的配置共同决定扫描、Adopt 与分发范围,兼容性保持 unknown。
_Avoid_: Custom Adapter, scan profile

**Global Skills Root**:
一个或多个已配置 Agent 在用户全局作用域读取 Skill 的 canonical 目录。它是可私有或共享的扫描输入与 appearance 证据;多个 Agent Configuration 引用同一路径时只扫描一次。
_Avoid_: Agent path, Agent directory

**Scan Appearance**:
一次扫描中,Global Skills Root 内某个 Skill 目录入口、关联 Agent 与到最终实体的完整 symlink chain。多个 appearances 可以聚合到同一 Canonical Skill Entity。
_Avoid_: Candidate, Activation

**Canonical Skill Entity**:
同一 scan generation 内解析到同一文件系统对象的全部 Scan Appearance 聚合。它不是持久 Skill identity;canonical path、对象 identity 与 tree fingerprint 只共同证明本次扫描事实。
_Avoid_: Skill ID, Canonical path

**Scan Coverage**:
一次 Scan Report 对全部 configured canonical Global Skills Root 的成功或 typed failure 记录,并保留每个 Root 的关联 Agent。它说明本次结果看见了什么,不把失败 Root 当成空目录。
_Avoid_: Detection, Scan scope

**Scan Run**:
一次以冻结的 Bound Home、Agent Configuration 与预算开始的完整 Rescan 执行,承载触发来源、进度、取消和 terminal status。只有 Complete 或 Incomplete 的 Scan Run 发布 Scan Report;Cancelled 与 Superseded 不替换 current Report。
_Avoid_: Scan Report, Scan Task

**Scan Report**:
generation-bound 的只读扫描结果,包含 Scan Coverage、Canonical Skill Entity、来源分组、Conflict Set、typed diagnostics 与操作资格。它是 Preview 证据,不是 Catalog truth。
_Avoid_: Catalog snapshot, Adopt plan

**Scan Exclusion**:
用户从当前 Scan Report 显式忽略的 Local 候选。按 Bound Home 与 canonical entity path 持久化到 Home 的 `scan-exclusions.json`，不写入可清理的扫描缓存，不按名称匹配其他路径。后续 Scan Report 将它归入“已排除 · 已忽略”，不提供 Adopt 操作；仍保留 Root coverage 与安全检查，且已纳管实体优先按 Catalog 归类。忽略不删除文件，也不修改已有启用项。保存会使旧 Report 的操作证据失效，UI 随后发起完整 Rescan。

**Scan Incomplete**:
至少一个 configured canonical Global Skills Root 未成功覆盖的 Scan Report 状态。健康 Root 的非破坏操作可以继续;可能移动、删除、替换实体或释放 external ownership 的操作须等待完整 Scan Coverage。
_Avoid_: Scan failed, Partial success

**Startup Probe**:
应用启动后对当前 Agent Configuration 引用的 Global Skills Root 与 Agent Activation Target 做存在性、可读性和路径身份的轻量只读观察。它只提供变化信号,不产生 Scan Coverage、Scan Report 或操作资格。
_Avoid_: Light Rescan, Lightweight Scan, Cached Scan

**Agent Detection**:
对 Agent Preset 已知 Global Skills Root 当前是否存在、可读及身份是否一致的只读观察。检测不创建目录、不授予写权限或写入 Agent Configuration;已检测但未配置的 Agent 不进入 Skill 扫描,「未检测到」也不表示 Broken。
_Avoid_: Agent configuration, 自动配置

**Agent Configuration**:
当前 Home 在 Agent 管理中保存的配置,包括稳定 Agent 身份、一个或多个 Global Skills Root、其中唯一一个可写 Root 作为 Agent Activation Target,以及可空的项目级目录约定。配置的增删改共同改变扫描、Adopt 与分发范围;多个配置可以引用同一共享 Root 或 Target。
_Avoid_: Agent Detection, scan configuration

**Agent Activation Target**:
Agent Configuration 从自身可写 Global Skills Root 中指定、供全局 Enable 创建 Activation 的唯一 canonical 目录。Target 按路径身份去重,可以由一个或多个 Agent Configuration 引用;其余 Root 只参与扫描。
_Avoid_: Global Skills Root, scan root

**Activation Target Group**:
同一 canonical Agent Activation Target 与全部引用它的 Agent Configuration 组成的操作投影;它只承载一份全局 Enable/Disable 与健康状态,不是一组彼此独立的 Agent Activation。
_Avoid_: Per-Agent Activation, Agent switch

**Activation Health Observation**:
对 Activation Target Group 中受管 Activation 当前 entry 与最终实体状态的有时间戳观察,可以作为历史结果持久化但不是现场 truth。检查失败保留旧结果并标记 Stale/Unknown,不能改写成 Missing 或 Broken。
_Avoid_: Activation truth, Agent Detection, Project Activation

**Resolved Project Skills Directory**:
一次项目级 Enable 中,由用户所选文件夹与 Agent 的项目级目录约定解析出的项目内最终 skills 容器;它只存在于本次操作计划,不是 Project 实体或 Agent Activation Target。
_Avoid_: Project, Project Activation Target

**Enable / Disable**:
把一个 Managed Skill 向选定 Agent 分发(Enable)或从受管全局 Target 撤回(Disable)的动词对。全局操作解析到 Activation Target Group;项目级 Enable 在 Resolved Project Skills Directory 创建不追踪的一次性软链,没有对应的项目级 Disable。
_Avoid_: Link / Unlink(Link 已用于入库方式), Mount, 挂载

**Activation**:
名词：Agent Activation Target 中的受管符号链接实体，身份由 Managed Skill 与 Target 共同确定，entry name 使用 Directory Identity，并**直指 Catalog 解析出的 Skill 最终实体**（Git Source Member → `<Home>/skills/git/<remote_id>/<skill_id>/`；其它 Install → Home 实体；Local Source → canonical 外部路径），不经过 Library 指针条目串联。
Activation 仅指受 Skill Man 管理且有 Catalog 记录的全局启用项；Enable 到项目级 Agent skills 目录产生的一次性软链不作 Activation 追踪、健康检查或修复(见 ADR-0015)。
_Avoid_: Link, 启用链接
