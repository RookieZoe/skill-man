# 启动观察、手动 Rescan 与无限规模证据存储

状态：Accepted

[决策:启动 Agent Detection 与全局 Rescan 调度和性能预算](https://github.com/RookieZoe/skill-man/issues/78)确认：首个可交互 Library Desk 不等待 Agent Detection、Startup Probe、Activation Health Observation 或完整 Rescan；常态启动不自动执行完整 Rescan。Skill Man 不替 Agent 限制 Skill、Root 或内容规模，完整扫描因此使用可取消、流式、磁盘型的 Scan Run，而不是把外部目录规模变成启动门槛或单个内存 DTO。

本 ADR 衔接 [ADR-0016](0016-agent-configurations-global-roots-and-shared-targets.md) 的 Agent Configuration/Root/Target authority 与 [ADR-0017](0017-canonical-scan-aggregation-and-source-attribution.md) 的 Scan Report contract。它取代 [ADR-0007](0007-first-run-and-settings.md) 的“启动时轻量 Untracked 扫描”，并取代 vNext 旧启动顺序中“Detection、Rescan、health 全部完成后才呈现 Library Desk”的结论。

## 启动观察与完整扫描触发

首屏只等待 Bound Home identity、operation recovery、Catalog open 与 WriteGate readiness。Library Desk 可交互后，Core 在共享的有界 I/O 调度器上并发启动三个相互独立的工作：

- **Agent Detection**：观察九个 Agent Preset 已知用户级 Root；进程启动、打开 Agent Management 和显式 Refresh 时 single-flight 运行。结果只在进程内按 Detection generation 发布，不写 Catalog 或 Scan Evidence Store。读取失败是 `Unknown/Unavailable`，不能伪装成 `Absent`；Add/Restore 配置仍重新验证 Root/Target。
- **Startup Probe**：只观察当前 Agent Configuration 引用的 Global Skills Root 与 Agent Activation Target 的存在性、可读性和路径身份；startup、配置 Apply 后与显式 Retry 运行。它不是轻量 Rescan，不产生 Scan Coverage、Scan Report 或操作资格。
- **Activation Health Observation**：按 Activation Target Group 检查受管 entry 与最终实体。Catalog 可以保存带时间戳的历史 observation；启动时先显示旧结果为 `Checking/Stale`，新结果按 Target 原子更新。startup、成功的 Enable/Disable/Repair、Target 配置变化与显式 Retry 只触发相关 Target；失败保留旧 observation 并标记 `Unknown/Stale + diagnostic`，不得写成 `Missing/Broken`。

三者独立发布并隔离失败；优先级为 Activation health、Startup Probe、Agent Detection。网络 update check 仍最后异步启动，不能改变 bootstrap gate。

完整 Rescan 只由两类显式场景触发：

1. onboarding 首次保存 Agent Configuration 后的解释性扫描步骤；零配置、Skip、Cancel 与零选择均允许完成 onboarding；
2. 用户从 Library Desk 或 Agent Management 手动发起。

常态启动、Startup Probe、Agent Detection 或普通 Agent Configuration 变化都不自动启动完整 Rescan。配置变化只把旧 Report 标为 Stale、使相关 Adopt draft/plan stale，并显示手动 Rescan 入口。

## Scan Run、Scan Report 与 Evidence Store

**Scan Run** 是完整 Rescan 的执行状态；每个 Bound Home 同时最多一个。重复触发只打开当前进度，Retry 始终创建新 generation。运行承载 `Queued/Running/Cancelling/Cancelled/Superseded/Completed`、触发来源、phase、Root、entry/entity/file/byte/Git-probe 计数、elapsed time 和 diagnostic。进度不是证据。

**Scan Report** 继续是 ADR-0017 定义的 generation-bound terminal evidence。只有 Complete 或 Incomplete Run 原子发布新 Report；运行期间继续显示旧 Report，并叠加 `Stale · Running`。Cancelled 与 Superseded 不替换 current Report。

完整 evidence 流式写入当前 Bound Home 的派生 **Scan Evidence Store**：

- 每个 Root 的 entry/entity/source evidence 逐步落盘，内存只保留有界队列、调度状态与汇总计数；UI 通过分页 Interface 读取，不能要求 Core 构造完整 `Vec` 或跨 Tauri 一次序列化全部 Report。
- 每个 Home 只保留一个 current terminal Report、一个可选 active temporary Run 与 compact delta summary；新 Report 原子提升后删除旧 evidence，不保留完整历史。
- Cancelled/Superseded 清理临时 evidence。cache 损坏只表现为 `No cached report`；cache 写入失败或磁盘不足使 Run 成为 `Failed · EvidenceStoreUnavailable` 并保留旧 Report，不能发布部分落盘的伪 Report。
- cache 是展示与差异基线，不是 Catalog truth。跨启动恢复后必须标为 Stale，不能创建 Adopt plan、声称当前 Scan Coverage 或授权操作。Abandon 不删除旧 Home cache，新 Home 不继承。

因为 Evidence Store 是 Skill Man 发起的 Bound Home 写，完整 Rescan 与持久化 health 只在 `WriteGate::Open` 下运行。CatalogReadOnly/Closed 可以浏览旧 stale Report；Agent Detection 仍可零写运行，Catalog 可读时 Startup Probe 可只读运行。Gate 在 Run 中失效时，Run 立即 Superseded 并清理临时 evidence。

## 规模、预算与终止

Skill Man 不设置 configured Root、目录 entry、Skill、appearance、canonical entity、文件数或内容字节的业务上限，也不因超出任一 Agent 自身的加载限制拒绝扫描。各 Agent 是否加载、截断或拒绝某个 Skill 由对应 Agent 管理，Skill Man 不冒充该 authority。

无限业务规模不取消终止和资源护栏：

- symlink walk 保持 cycle detection 与既定 16-hop 上限；worktree discovery 不越过 originating canonical Root；Rescan 网络请求恒为零；
- 同时最多扫描 4 个 Root、hash 2 个 entity、执行 2 个 local Git probe；同一 Run 内的 path identity、worktree 与 lock fact memoize；有界队列通过背压限制内存，而不拒绝剩余 evidence；
- 没有 Scan Run 总 hard deadline。Detection + Startup Probe 超过 1 秒、Activation health 超过 5 秒、单 Root 超过 10 秒、整个 Run 超过 30 秒只发布 `Slow`，不截断；
- 单 Root/Target 连续 30 秒没有任何 entry、byte 或 probe 进度才进入 typed `Unresponsive` 并隔离；只要持续产生进度，合法的大规模扫描可以继续；用户始终可以 Cancel。

Root 是最小证据提交单元。Root 只有完整成功后，其 evidence 才进入候选聚合；中途 unreadable、identity replacement、Unresponsive，或可证明只影响该 Root transaction 且 Store 仍健康的局部 evidence 错误，只发布该 Root 的 coverage diagnostic、已处理计数与最后 phase/entry，不发布依赖枚举顺序的半截候选。其它成功 Root 仍可形成 Incomplete Report；同一实体若也由健康 Root 可达，只保留健康 appearance，但 Report 仍为 Incomplete。Store-wide I/O、磁盘不足、manifest 或完整性失败不能降级成单 Root 失败，整次 Run Failed 并保留旧 Report。

## 并发变化与 plan stale

Scan Run 冻结 `home_id`、WriteGate generation、Agent Configuration generation、configured canonical Root snapshot 与 filesystem-mutation generation。Home、gate、配置变化，或任何可能改变扫描 Root、实体、Activation entry、installer lock 的产品写开始时，写操作优先：当前 Run Superseded 并协作取消，不以长读锁阻塞 Import、Adopt、Update、Remove、Enable/Disable 或 Repair。

新 Scan Run 开始即使全部未应用 Adopt draft/plan stale；取消、失败或 Supersede 后不复活。运行期间不能从旧 Report 新建 Adopt plan。外部文件系统变化无法由进程 generation 预知，Adopt apply 仍逐实体重验 frozen object identity、tree、appearance 与 lock evidence。

Activation health 不使 Scan Report stale，但 observation apply 必须 CAS 当前 Home、Agent Configuration generation、Target identity 与 WriteGate generation。Target 间失败隔离；项目级一次性软链不进入 health、Scan 或 Catalog observation。

## UI 与恢复后果

Library Desk 与 Agent Management 都提供手动 Rescan 状态和入口。非模态 Evidence Ledger 显示 `Never scanned/Stale/Running/Complete/Incomplete`、真实 phase/Root/count/elapsed time、Slow diagnostic，以及 Cancel、Retry、Restart；无限规模下不显示无法证明的百分比或 ETA。具体视觉布局由后续 UI 原型决定。

Safety Snapshot 仍永不自动删除。Restore 后只有同一 `home_id` 的后续成功启动和一份手动产生的 Complete Scan Report 才允许规划删除；Startup Probe 与 Incomplete Report 不满足条件。configured Root union 为空时，显式 Rescan 可以产生 Complete 空范围 Report。选择不在常态启动自动 Rescan 的代价，是 Restore 表面必须明确提供 `Run Rescan`。

## Consequences

首屏延迟不再受用户 Root 规模支配，Detection 不会重新成为 Catalog truth，合法大目录也不会因 Skill Man 自设数量阈值被截断。代价是实现必须引入磁盘型 Evidence Store、分页 Interface、single-flight 调度、progress-aware watchdog、generation/CAS 协调和可见的 stale 状态；不能沿用当前串行全量 `Vec`、单 Root 失败终止整次扫描、启动自动 Rescan 或内存 `last_report` 作为长期架构。

当前 `MAX_ADOPT_SKILLS` 与 `MAX_SKILL_DOCUMENT_BYTES` 不得继续作为 Scan 候选截断或 Skill 合法性判定；实现须以流式读取、分页和背压替代。既定 symlink 16-hop/cycle 则是终止与路径证据护栏，继续保留。
