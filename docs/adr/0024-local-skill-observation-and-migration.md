# 本地 Skill 观察、扫描结果投影与显式迁移

状态：Accepted

日期：2026-09-08

实现基线：`main` 的 `bbb1fa4a2aa4f0f442de45e61bcbae9fd3af5c23`。

本 ADR 回溯记录当前本地纳管实现，细化 [ADR-0017](0017-canonical-scan-aggregation-and-source-attribution.md) 的 Local Source 观察及迁移规则，补充 [ADR-0020](0020-startup-observations-and-manual-rescan.md) 的报告 freshness 与手动重扫边界。它不改变 Git Source Release 的完整性校验或 Source Transition 恢复语义。

## 背景

控制目录内的 Local Source 需要真正迁到外部稳定位置，不能把“迁移”接到只允许原地 Link 的计划后返回 `requires_move`。目录选择、只读预览和明确确认必须构成可取消的完整操作，而不是直接从报告跳入写入。

扫描与计划若按不同顺序计算哈希，未改变的 Skill 也可能得到 `plan_stale`；把安装生成的依赖纳入 Skill 变化判断，也会使正常安装依赖影响纳管。反过来，迁移时省略依赖会丢失用户目录内容。因此需要区分逻辑观察与物理传输，而不是统一扩大“忽略”的范围。

## 决议

### 两种“忽略”具有不同边界

| 机制 | 匹配与记录 | 影响 | 不做什么 |
| --- | --- | --- | --- |
| Scan Exclusion：用户忽略整个本地候选 | 当前 Bound Home 的 canonical entity path，持久化到 `scan-exclusions.json` | 当前报告移入“已排除 · 已忽略”；后续分类不再提供该候选的纳管操作 | 不按 Skill 名称全局匹配，不删除目录，不改写既有 Activation，不取消 Root coverage 与安全检查 |
| 依赖子树排除 | 本地 Skill 内任意层级，严格名为 `.venv`、`venv`、`node_modules` 的目录或符号链接 | 不进入扫描内容计数、来源识别和本地纳管的内容变化证据 | 不删除、不作为复制排除规则、不放宽 Git release 或物理恢复校验 |

依赖排除在下降或读取内容之前生效，不跟随依赖符号链接。同名普通文件与相似名称仍属于 Skill 内容；这不是模糊目录名匹配或 `.gitignore` 解释器。来源识别仍基于 Skill 的入口、ownership 区域、适用 lock 与有边界的向上 Git 探测，不从依赖里的 `SKILL.md` 或 `.git` 反推来源。

用户忽略整个候选需要 current report 和开放的 WriteGate。保存忽略名单不提升文件系统 mutation generation，不使其它未变化候选仅因这次忽略而过期；计划与执行均重查忽略名单，不能使用忽略前的旧 token 纳管被忽略的实体。已纳管实体仍优先按 Catalog 归类。

### 扫描事实和操作结果投影分离

- Scan Report 继续是 generation-bound 的只读观察，不是 Catalog truth。界面按 `entityRef` 记录已忽略、已纳管的局部结果，调整候选行、排除分类与汇总数量，不重写原报告事实。
- 忽略、本地纳管及 Git 来源操作完成后，不自动调用完整 Rescan；撤销成功时相应撤回局部投影。Catalog 与相关操作状态可以刷新，这不等于遍历文件系统重扫。
- 局部投影不为后续操作续期。Home、配置、WriteGate 或真实产品文件系统写入仍可能使 Scan Run/Report 与未应用计划失效；外部变更仍在计划和确认时逐项重验。需要新证据时由用户显式重扫，不能以“只更新 UI”为理由绕过 stale 拒绝。
- 新报告身份对应新的投影生命周期，不把旧操作结果跨代解释为新扫描事实。

### 逻辑观察哈希与完整物理快照分开

`FileSystem::skill_observation_snapshot` 和流式 `scan_tree_statistics` 使用同一依赖排除策略，按全局相对路径的原始字节序计算观察哈希。逐目录深度优先顺序不能替代这个全局顺序。扫描与预览/确认对同一未变化 Skill 必须产生相同观察证据。

Scan Evidence Store 的 schema 版本为 4；旧观察规则生成的缓存不能继续授权计划，需要重新扫描。这个版本只描述派生扫描证据，不是 Catalog、Home Binding 或 Git Source Release 的版本迁移。

预览保存逻辑观察证据。执行迁移前，在 WriteGate 下重新读取观察，获取完整物理快照，再复查观察；只有依赖变化时可以接受当前物理内容，但普通 Skill 内容、根对象身份或其它冻结事实变化仍使计划失效。执行冻结的完整快照写入 journal，用于后续复制、回滚和恢复。

依赖从预览到确认之间变化不等于 Skill 内容变化，但冻结物理快照之后的复制不一致仍必须拒绝。`staged_tree_snapshot` 等完整快照能力继续包含依赖，不能全局改成忽略依赖的哈希函数。

### 独立、需确认的 Local Link 迁移计划

稳定外部目录使用原地 `local_link`；仍在 Agent/installer 等控制区内、需要移动的实体使用 `local_link_with_move`，内部对应 `AdoptPlanKind::MoveLink`，不复用 Home 内 File Install 的旧 `Migrate` 语义。

用户操作顺序固定为：

1. 点击本地候选的“迁移并建立本地链接”，打开该候选的迁移弹窗。
2. 在弹窗中通过原生目录选择器选择目标父目录；选择本身不产生写入。
3. 请求只读计划，展示原 canonical 路径、最终目标路径、受影响入口及链接目标。更换父目录先取消旧计划。
4. 只有计划可应用时才允许“确认迁移”；执行期间禁止重复提交和关闭。
5. 在同一弹窗呈现结果、错误及可用的 Undo。取消预览调用 cancel；关闭结果调用 finalize。失败不静默触发重扫或隐藏在报告底部。

目标是所选父目录下的 Skill 目录，不是把父目录本身作为 Skill。计划必须证明父目录在受保护控制区之外、目标 entry 不存在，并冻结父目录身份。迁移要求全部 configured canonical Root 的完整 coverage，以免移动实体时破坏未知 appearance；不能用警告确认替代证明。

### 移动完整内容，守住提交与恢复边界

确认执行仍经过 Core 的计划 token、报告身份、忽略名单、对象身份、appearance 链、内容证据、目标与 WriteGate 检查。随后通过持久化 journal 暂存原实体，完整复制并验证目标内容，登记外部 Link，按计划处理原入口与全局 Activation。

依赖目录和符号链接一并保留；符号链接复制原始目标字符串，不追随目标复制另一棵树，也不自动改写链接语义。目标仍归用户所有，Catalog 记录其 canonical 路径，Home 不因此新增一份 File Install。

失败回滚只处理本次操作能够证明所有权和身份的产物。Undo 同样有条件：迁移后的完整内容或对象身份已变化时拒绝覆盖；依赖排除不免除 Undo 的物理内容校验。中断或回滚无法安全证明现场时保留数据并进入恢复流程，不把未知状态报告为成功，也不靠重新扫描恢复事务。

## 取舍与后果

- 普通单项本地迁移不再使用批量勾选加报告底部按钮；Conflict winner 等其它纳管选择流程仍可保留预览入口，不能据此删除所有 Adopt planning 能力。
- 整体候选忽略是持久偏好，依赖排除是观察规则，迁移是完整物理数据操作。三者分开能避免误报和数据丢失，但测试必须同时验证观察不变与复制内容完整。
- 不自动重扫保留当前浏览位置和操作上下文，但不保证同一报告在任意写入后永远可操作。stale 诊断与手动重扫仍是必要能力。
- 本 ADR 不引入依赖清理、环境重建、符号链接修复或项目级生命周期管理。迁移保留目录字节，不承诺虚拟环境等依赖在新路径上仍能直接运行。

## 实现与回归证据入口

- 界面与投影：[ScanEvidenceLedger.tsx](../../src/features/scan/ScanEvidenceLedger.tsx)、[LocalMigrationDialog.tsx](../../src/features/scan/LocalMigrationDialog.tsx)。
- 计划与执行：[adopt_report_plan.rs](../../src-tauri/src/core/adopt/adopt_report_plan.rs)、[adopt.rs](../../src-tauri/src/core/adopt.rs)。
- 忽略与证据版本：[scan/mod.rs](../../src-tauri/src/core/scan/mod.rs)、[scan_evidence_store.rs](../../src-tauri/src/seams/scan_evidence_store.rs)。
- 观察/物理能力边界：[filesystem.rs](../../src-tauri/src/seams/filesystem.rs)、[macos_fs.rs](../../src-tauri/src/adapters/macos_fs.rs)。
- 回归覆盖：[LocalMigrationDialog.test.tsx](../../src/features/scan/LocalMigrationDialog.test.tsx)、[ScanEvidenceLedger.test.tsx](../../src/features/scan/ScanEvidenceLedger.test.tsx)、[adopt_report_flow.rs](../../src-tauri/tests/adopt_report_flow.rs)、[skill_observation.rs](../../src-tauri/tests/skill_observation.rs)。覆盖预览/取消零写入、目标保护、stale、忽略与旧计划、依赖变化、哈希顺序、完整复制及回滚/撤销，不据此声称所有原生环境均已验收。
