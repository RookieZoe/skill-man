# canonical Skill Entity 扫描聚合、来源归属与部分结果

状态：Accepted

当前实现补充：[ADR-0024](0024-local-skill-observation-and-migration.md) 细化依赖观察排除、候选忽略、单项迁移弹窗与操作后的局部报告投影；本文的 canonical 聚合、来源归属和 coverage 权限边界继续有效。

[决策:扫描去重、来源分类与汇总呈现](https://github.com/RookieZoe/skill-man/issues/77)确认：Rescan 只消费已配置 Agent 的 canonical Global Skills Root union，先按物理 Root 去重，再在一次 scan generation 内按最终文件系统对象身份聚合 canonical Skill Entity。来源类型表达 ownership 与纳管方式，不由 `.git` 单独决定；用户拥有的外部开发工作区即使位于 Git repository 中也仍是 Local Source。这个边界避免多 Agent/多 Root 重复候选、dotfiles repository 误判和旧 installer lock 冒充当前 Git Source Release，同时允许单个 Root 失败时保留可解释、受操作影响约束的部分结果。

本 ADR 取代 [ADR-0005](0005-adopt-existing-skills.md) 中按 path 粗略聚合、固定扫描源、safe 候选默认勾选和 Root 失败语义；取代 [ADR-0013](0013-adopt-provenance-and-remote-source-parents.md) 中“Git 来源只能从 lock 进入”的扫描入口。Git Repository Source、完整 Source Release、External Ownership Claim 与 Source Transition 的来源事实仍以 [ADR-0014](0014-git-repository-source-releases-and-transitions.md) 为准；Agent Configuration 与 Root union 以 [ADR-0016](0016-agent-configurations-global-roots-and-shared-targets.md) 为准。

## canonical entity 与 appearance

- 扫描只枚举已配置 Agent 的 Global Skills Root。Root 先按 canonical path identity 求并集，每个物理 Root 只扫描一次；项目级目录永不进入扫描、Adopt 或汇总。
- 一个 **Scan Appearance** 是某个 Root 中的目录入口、其关联 Agent、入口 identity 和到最终实体的完整 bounded symlink chain。同一入口被多个 Agent Configuration 共享时只读取一次，但报告保留全部 Agent 关联。
- 一个 **canonical Skill Entity** 是同一 scan generation 内解析到同一文件系统对象的全部 appearances。聚合键使用卷/设备与 file-id/inode 等对象身份；canonical path、对象身份和 tree fingerprint 一起用于展示及 stale 重验。它不是持久 Skill identity，不能跨 generation 写入 Catalog 当作 truth。
- 多 Agent、多 Root、真实目录与多跳软链的混合形态只形成一个候选，全部 appearances 逐条保留。dangling、cycle、hop-limit、non-UTF-8、读取失败或 identity replacement 停在精确失败位置，不产生猜测实体或部分 fingerprint。
- 同一对象经不同 Skill directory identity 出现是 Identity Conflict，不拆成多个实体，也不按 frontmatter、内容 hash 或 mtime 选名称。

## 来源归属

来源分类先判断最终实体的 ownership 位置和 applicable external claim，再解释 Git metadata：

| 最终实体与证据 | 扫描归属 |
| --- | --- |
| 位于全部 Global Skills Root、installer-managed root、Bound Home、App state 与 system/builtin/cache root 之外，且无 applicable lock | Local Source；即使它属于可读 Git worktree，也视为用户开发工作区，只登记真实 canonical 路径，不移动、复制或改写内容 |
| 完整检查后没有 Git/lock 信号，但实体仍在 Agent/installer 控制区 | Local Source；Adopt 前须显式迁到稳定外部位置，再以 Link 认领 |
| 控制区内存在受支持 provider 的 bounded worktree 证据，或存在唯一有效 applicable Git lock claim | Git Repository Source Candidate；worktree 提供 repository/member hint，lock 提供 repository/ref hint 与 External Ownership Claim |
| 外部开发区只有损坏 gitdir、多 remote 或其它无法唯一解释的 worktree metadata，且无 applicable lock | Local Source，并显示未采用 Git metadata；不得把工具状态冒充用户 ownership |
| 控制区内的 Git metadata 无法解释，或 worktree、lock、repository、member、ref、owner 互相矛盾 | typed Blocked、Provenance Conflict 或 Repository Ref Conflict；没有 Include，不自动降级 Local |
| remote fetch/discovery 暂不可完成 | Verification Deferred；保持现状并允许 Retry，不创建部分来源 |

worktree discovery 只在产生 appearance 的 canonical Global Skills Root 或 installer-managed root 内向上查找最近 repository root；repository root 可以等于该 Root，但不能位于其上层。Root 上方的 Home/dotfiles repository 是 ambient metadata，不参与分类。外部稳定实体不需要 worktree discovery 才能证明 Local；有效 lock 则可为不含 `.git` 的 materialized entity 提供 Git source hint。

worktree 与 lock 是并列证据，不是互相覆盖的优先级。二者一致时汇合；矛盾时 fail closed。Hint 不构成 Git Repository Source 或 Source Release，不继承旧 lock、本地 HEAD、dirty bytes、旧 skillPath、hash 或 commit。候选按 `(provider, canonical repository identity)` 再聚合为一个 Git Repository Source Candidate；同仓多 ref 是一个 Repository Ref Conflict，不拆分来源。只有用户显式 Fetch Latest and Manage、选择 tracking ref 并从远端发现完整不可裁剪 Source Release 后，才进入 Source Group Preview。

## 部分 Scan Report 与操作资格

Scan Report 是 generation-bound 的只读事实，固定携带每个 canonical Root 的 coverage、关联 Agent、成功或 typed diagnostic，以及 canonical entity、appearance、Local candidate、Git source group、Conflict Set、Blocked、Deferred、Excluded 计数。Fetch Latest 后发现的 Source Member 另行计数，不能回写成初始扫描候选或 appearance。

一个 Root 失败不抹掉其它成功 Root 的候选，也不把报告伪装成 complete。失败 diagnostic 在 Preview、plan 与结果中持续可见；其它 Root 可继续处理，但操作资格由物理影响决定：

- 保持最终实体原位的稳定 Local Link 等非破坏操作可以继续；
- 任何需要迁移、删除、替换最终实体或释放 external ownership 的操作，必须等全部 configured canonical Root 成功覆盖后才能计划；
- Root 是最小证据提交单元；中途失败 Root 只发布 coverage diagnostic、计数与最后进度，不把依赖枚举顺序的半截候选写入 Report；
- 部分报告只声称健康 Root 的 known appearances，不能声称完整 appearances，也不能让用户用通用 warning 确认绕过 destructive guard。

扫描调度、缓存、取消、无限规模资源语义和首个可交互快照由 [ADR-0020](0020-startup-observations-and-manual-rescan.md) 冻结；本 ADR 只冻结结果与操作资格 contract。

## Conflict、选择与汇总呈现

Local candidates 以 `NFC + Unicode casefold` Directory Identity key 形成 Conflict Set。同名不同 canonical entity 默认无 winner；用户可以显式选择一个 Local winner，其余实体与 appearances 原地保持 Untracked。已经 Managed 的同名 Skill 不在普通 Local Adopt 中被替换；同一实体多 Directory Identity 必须先统一名称后 Rescan。系统不按内容合并、不自动加后缀或改名。

涉及 Git Source Member 的同名关系保留 Directory Identity、canonical entity、repository identity、repository-relative `skillPath` 和 source membership，但名称相同本身不阻断来源。依 [ADR-0018](0018-git-source-namespaces-and-immutable-members.md)，Git Source Member 使用稳定 `skill_id` 并按 Git Repository Source 分组，同一或跨 repository 同名均可进入 Library；只有向同一 Agent Activation Target 发布相同 Directory Identity 时才形成 Activation Conflict。Local↔Local Conflict Set 与非 Git 既有行为不变。

汇总层级固定为 Scan incomplete、Needs attention、Git sources、Local sources、Excluded/already Managed。External Ownership Claim 是需要来源级审阅的事实；稳定开发目录和 multiple appearances 是信息，不是 warning。Local Include、Conflict winner 与 Git source review 默认都未选择；Blocked/Deferred 没有选择控件。

onboarding 与手动 Rescan 共用同一 Scan Report contract。onboarding 在 Agent Configuration 后解释首次完整扫描，允许零配置、零选择、Skip 或 Cancel；常态启动只做 Startup Probe，不自动执行完整 Rescan。手动 Rescan 使用非模态 Evidence Ledger，运行期间保留旧 stale Report，只有 Complete/Incomplete 才原子发布新 Report。具体视觉结构由[原型:Agent 配置、项目作用域与 Enable 操作面 UI](https://github.com/RookieZoe/skill-man/issues/75)决定。

## Consequences

### 显式迁移到外部 Local Link

本地内容观察排除任意层级的 `.venv`、`venv`、`node_modules` 目录/符号链接子树，排除发生在下降和读取内容之前；同名普通文件仍计入。来源探测只使用 Skill 自身入口及向上探测，不从依赖子树发现来源。扫描统计与 Local Adopt 预览/执行复查采用同一个观察策略，哈希顺序为全局相对路径字节序，而非逐目录深度优先。Scan Store schema 4 使旧观察结果失效。

观察快照与物理迁移快照分离：前者判断 Skill 是否变化；后者完整包含依赖，在执行前冻结并写入 Journal，以校验实际传输及回滚/恢复。预览到确认期间只有依赖内容变化不使本地迁移计划过期；真正 Skill 内容、入口身份、目标父目录及写入门禁仍复查。依赖符号链接按原始目标字符串保留，不跟随复制。复制进行中的物理数据不一致仍拒绝，不能以忽略规则掩盖数据损坏。

`local_link_with_move` 与原地 `local_link` 是不同的计划操作。用户先选择稳定外部父目录；预览只读展示原路径、目标路径和受影响入口，取消不移动文件。目标不得位于 Bound Home、App state、Agent 或 installer 控制目录中，也不得覆盖已有同名目录。只有完整扫描覆盖才能生成迁移计划。

确认时重新验证源实体、目标父目录身份和目标不存在，再通过持久化 journal 暂存原实体、验证复制内容、登记外部 Link 并替换原入口。新目录不成为 Home 内的 File Install。失败回滚；撤销须验证内容与对象身份，发生外部修改或中断身份不明时保留数据并进入 RecoveryRequired，不覆盖用户文件。成功后界面局部更新当前报告，不自动重新扫描；扫描事实与操作结果投影保持分离。

扫描 DTO 必须区分 Root、appearance、canonical entity、source group、source member 与 conflict set，不能继续用一张逐路径候选表承担全部层级。对象 identity 只在 generation 内有效，Catalog 仍以稳定 Skill、Source 与路径事实为权威。部分扫描提高了健康 Root 的可用性，但所有会破坏未知 appearance 的操作都必须等待完整 coverage；这是局部继续与 fail-closed 所有权之间的明确取舍。
