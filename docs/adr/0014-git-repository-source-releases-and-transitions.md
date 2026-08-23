# Git Repository Source 的整仓 release 与来源切换

状态：Accepted

受支持的远程 Git provider 不再把同一仓库中的每个 Skill 当作可独立验证、交接和更新的来源。它们统一以规范化 Git repository 为一个 Git Repository Source，在唯一 tracking ref 的一个 Source Release 中发现完整成员集，并通过 Source Transition 共同交接或更新。这样与外部 installer 实际按整个仓库安装和更新全部 Skill 的行为一致，也避免把陈旧的逐 Skill lock 或本地内容误称为已验证远端 provenance。

本 ADR 取代 [ADR-0013](0013-adopt-provenance-and-remote-source-parents.md) 中仅针对受支持 Git provider 的逐 Skill Remote Binding、Ownership Handoff、Update 与验收语义；GitHub 是 provider 子类型，不再有例外模型。ADR-0013 的严格 lock 解析、provider URL 规范化与凭据拒绝、安全 tree materialize、exact CAS、单一 owner、失败关闭以及非 Git sourceType 行为继续有效。

本决定收束 [既有逐 Skill 远程模型迁移与规格取代](https://github.com/RookieZoe/skill-man/issues/59) 的路线。它不实施真实用户数据迁移，也不扩大非 Git sourceType 的范围。

## 领域模型

- **Git Repository Source**：受支持 Git provider 加规范化 Git repository identity 的来源单元。一个 source 只有一个 tracking ref，并由稳定 `remote_id` 标识。
- **Source Release**：tracking ref 在一个 resolved commit 上的完整、可发现 Source Member 集及其远端 tree 事实。成员没有独立的 ref 或 commit。
- **Source Member**：Git Repository Source 中由 repository-relative `skillPath` 识别的 Managed Skill 成员。路径变动不能按名称、hash 或相似度推断为同一成员。
- **Source Transition**：一个 Git Repository Source 从既有状态进入某个固定 Source Release，或在两个 release 之间更新的整体操作。整个来源是唯一 commit、恢复和 Undo 单位。
- **Legacy Per-Skill Git State**：具有旧 parent/binding 的逐成员 ref、commit 或 baseline，但缺少共同 release 与完整成员事实的既有状态。它不是当前 Source Release。
- **Source Promotion**：用户显式确认的、把无歧义 Legacy Per-Skill Git State 原地提升为 Git Repository Source 的 Source Transition。它保留 `remote_id`，但不继承旧成员证据为当前 release。

## 迁移与能力判定

启动对 Catalog 的实际表、列、约束和 source manifest 做零写入 Source Capability Scan。它不以 `schema_version` 判定状态，不推断 release，不补写缺失结构，也不自动修复数据。

满足 Git Repository Source 所需结构且有共同 release 与完整成员集的记录，按新模型工作。旧逐成员记录归为 Legacy Per-Skill Git State：可读、可 Disable/Remove，保留历史审计；来源级 Fetch Latest、Update 和自动合并一律关闭。

只有用户发起 Source Promotion 时，系统才重新获取用户选择的 tracking ref、发现完整 Source Release，并在最终 Source Group Confirmation 后的受控 Source Transition 中执行幂等结构准备和数据转换。唯一、无冲突的 legacy parent 原地提升并保留 `remote_id`；旧逐成员 ref、commit、anchor 和 baseline 只保留为历史审计或冲突输入。多个 parent、多个 ref、缺失能力、部分成员或结构完整性失败均 fail-closed，保持 Legacy Per-Skill Git State。

同一规范化 repository 的旧 lock 声明出现多个 ref 时，进入 Repository Ref Conflict。用户必须显式选择一个 ref；系统从该 ref 的当前 Source Release 建新来源，不拆分来源，也不从旧 local bytes、old `skillPath` 或 hash 推断成员和 baseline。

## 来源组预览与确认

对受支持 Git 的入口是 Fetch Latest and Manage：以 `sourceType`、`sourceUrl` 和用户选择的 ref 获取仓库并发现当前完整 Source Release。旧 external lock 仅显示 External Ownership Claim；它可说明将被 CAS 释放的声明，却不证明旧实体、成员路径、remote baseline 或旧内容已被验证。

Source Group Preview 在一个来源父节点下显示 provider、规范化 repository、tracking ref、resolved commit、完整成员集与每个成员动作。成员资格不可用逐项 Include 裁剪。预览仅创建 Source Group Draft；所有冲突处理都只是草案，取消或重新扫描不写 Home、Catalog、stage、journal 或 lock。

对目标 release 仍存在的已修改成员，用户只能显式选择保留当前内容并标记 Modified，或以目标 release 内容替换；两种选择都留在同一个 Source Transition，不能把成员跳过或改为 Local Link。目标 release 不再含有旧路径时，保留 Home 内容，用户必须显式 Remove、Link 或做 Explicit Member Mapping 到一个未占用的目标路径；系统绝不猜测重命名。所有阻塞与成员动作解决后，只有一次 Source Group Confirmation，确认完整来源和所有外部所有权影响。

同一来源的外部声明若跨多个 installer lock 文件，进入 Repository Ownership Split。因为无法以一个外部所有权提交点完成整体交接，Fetch Latest and Manage 必须零写入拒绝，直到用户显式收敛到一个稳定 external installer root。

## Source Transition、恢复与更新

确认后先写入 Source Transition Journal，固定旧状态、目标 Source Release、完整成员动作、外部 lock fingerprint/claims 和恢复事实；此后不重新解释远端的“最新”。完成 staging、路径/inode/tree/成员/空间重验后，必须在 Source Ownership Commit Point 前再次做完整 Source Transition Preflight。

单一 external installer lock 文件中属于该来源的全部 applicable claims 以一次 full-file exact CAS 共同释放，这是唯一逻辑 commit point。此前任何失败都恢复全部隔离来源并保持零 Catalog/lock 变化；此后只能按 journal 把完整来源 roll-forward 到固定目标 release。绝不留下同一 release 的部分成员、长期双 owner 或无人 owner。

Source Undo 是结果窗口内的条件性来源级整体逆转：只有全部成员、来源记录、Home 实体、external path 和旧 lock claims 都仍满足 guard 才能恢复；任一 guard 失败则整体拒绝。普通 Remove 不恢复 external owner。

日后 Update 重新获取唯一 tracking ref，发现新的完整 Source Release，并重复 Source Group Preview、冲突草案、最终确认和 Source Transition。外部 installer 重新出现仍是 Ownership Conflict，不是 Update；不自动覆盖、合并或再次纳管。

## Consequences

新的 Git Repository Source ADR 和 vNext Spec 是受支持 Git provider 的当前权威；已关闭的逐 Skill 实施记录仍是历史事实，不能作为继续实现旧模型的依据。后继实施必须同时覆盖实际结构能力扫描、legacy promotion、来源组 UI/DTO、全来源 journal/CAS/recovery、源级 Update/Undo，以及非 Git sourceType 不变的回归证据。
