# 项目级 Skill 分发：项目副本与 Agent 相对链接

状态：产品决策与测试 seam 已确认；#101 已实现，#102–#104 待实现，原生人工验收待完成。产品决策依据为 ADR-0025。

已发布：[GitHub issue #100](https://github.com/RookieZoe/skill-man/issues/100)。以下保留发布时的完整目标契约；当前交付进度见 [ADR-0025 的实现状态](adr/0025-project-local-skill-copies.md#实现状态)。

## Problem Statement

本规格提出前，项目级分发创建直指 Library 来源实体的软链。项目移动、分享，或原 Skill Man Home / Local Source 不再可用时，项目可能失去 Skill 内容。用户希望项目自己拥有一份可修改的内容，多个 Agent 共用它，同时明确知道哪些已有文件会被保留、哪些条目会被覆盖。

原有流程要求选择至少一个 Agent，无法表达“只交付项目副本”。原有提交与撤销也以独立链接为单位，未表达一个副本被多个链接依赖的关系。

## Solution

每次选择一个项目文件夹及一个或多个 Managed Skill。项目内 `.agents/skills` 是固定、必须存在的基础目录，界面在选择和确认阶段明确提示。无需选择附加 Agent 也可完成分发。

首次分发生成 `.agents/skills/<Skill 目录名>/` 中的 Project Skill Copy；已有可用副本保持原样并复用。可选 Agent 在其 Resolved Project Skills Directory 中获得逐 Skill 相对软链，指向项目副本。副本由项目自行管理，与 Library 来源后续更新、删除独立。

预览展示将创建、复用、覆盖、跳过或阻止的项目；覆盖既有 Agent 条目前取得确认。执行结果区分副本与各 Agent 链接，保留部分成功，并提供结果页内的安全 Undo。

## User Stories

1. As a project maintainer, I want a required project-local Skill copy, so that my project owns its distributed content.
2. As a project maintainer, I want to distribute without selecting an additional Agent, so that I can prepare the shared project directory alone.
3. As a project maintainer, I want the required directory explained before confirmation, so that I understand the filesystem changes.
4. As a project maintainer, I want selected Agents to share one copy through relative links, so that their Skill content stays consistent.
5. As a collaborator, I want the project to remain readable after relocation without the original Home, so that I can use its Skill files independently.
6. As a project maintainer, I want existing project copies preserved, so that redistribution never overwrites project edits.
7. As a project maintainer, I want newly selected Agents to use the existing copy, so that project customizations apply to them too.
8. As a project maintainer, I want reuse clearly distinguished from copying Library content, so that I know which version was distributed.
9. As a project maintainer, I want invalid existing copy entries preserved and reported, so that resolving an error does not destroy my files.
10. As a project maintainer, I want source updates and removals to leave my copies untouched, so that project changes remain under my control.
11. As a project maintainer, I want multiple Skills distributed together, so that I can prepare a project in one operation.
12. As a project maintainer, I want to choose one source for competing Skill directory identities, so that the shared copy has an explicit origin.
13. As a project maintainer, I want unresolved name conflicts skipped independently, so that other Skills can still be distributed.
14. As a project maintainer, I want shared Agent directories deduplicated and disclosed, so that I understand all affected Agents.
15. As a project maintainer, I want an Agent using the base directory to reuse it directly, so that no self-link replaces the copy.
16. As a project maintainer, I want existing correct relative links left untouched, so that repeated distribution is harmless.
17. As a project maintainer, I want conflicting Agent paths and directory contents disclosed before replacement, so that I can make an informed choice.
18. As a project maintainer, I want old external Agent links replaced only after confirmation, so that migration to local copies is deliberate.
19. As a project maintainer, I want Preview and cancellation to leave files untouched, so that reviewing a plan is safe.
20. As a project maintainer, I want changed paths or contents rechecked before execution, so that an outdated preview cannot authorize unexpected writes.
21. As a project maintainer, I want hidden files, dependencies and executable scripts copied, so that useful Skill payload is retained.
22. As a project maintainer, I want Git metadata excluded, so that copying a Skill does not copy repository administration data.
23. As a project maintainer, I want internal Skill links rewritten relatively, so that they remain valid inside the copy.
24. As a project maintainer, I want external, dangling and cyclic payload links reported with their paths, so that incomplete copies are not presented as usable.
25. As a project maintainer, I want project directory links constrained to the project, so that distribution cannot escape the selected folder.
26. As a project maintainer, I want dependent links skipped when copying fails, so that distribution creates no links to an unfinished copy.
27. As a project maintainer, I want successful work retained when one Agent fails, so that available results remain usable.
28. As a project maintainer, I want retry to create a fresh plan, so that fixes and intervening changes are revalidated.
29. As a project maintainer, I want Undo to restore replaced Agent entries, so that I can reverse this operation while reviewing its result.
30. As a project maintainer, I want Undo to preserve pre-existing or externally edited copies, so that it cannot erase unrelated work.
31. As a project maintainer, I want a new copy retained if a dependent link cannot be undone, so that partial Undo avoids breaking retained links.
32. As a project maintainer, I want interrupted operations recovered safely, so that restarting does not lose replaced content or expose incomplete copies.
33. As a Chinese or English user, I want localized instructions and precise per-item results, so that I understand the operation without reading internal diagnostics.
34. As a user of global distribution, I want its existing behavior preserved, so that this project feature does not change my global Activation state.

## Implementation Decisions

### Module and public contract

- Extend the existing Enable Module and its `EnableApi` plan → apply → undo/finalize surface. Keep Core → seam → adapter boundaries; the frontend consumes typed DTOs and does not perform filesystem mutations or loop over individual write commands.
- A project request accepts one project folder, a nonempty Managed Skill selection, zero or more additional Agent identities, and explicit conflict decisions. The base copy does not depend on a General Agent configuration existing or being selected.
- Plan and result DTOs must distinguish copy creation, existing-copy reuse and dependent Agent link actions. Expose stable action identities, copy dependencies, resolved destinations, affected Agents, eligibility, typed blocking reasons and per-action outcomes. Exact field names are implementation choices, but a zero-Agent plan must contain its required copy actions and be executable.
- Preserve existing global Enable/Disable, Activation, desired-state and storage identifiers. Add project-specific typed facts without fabricating project Activation records or changing global target grouping.
- Reuse the existing filesystem seam for safe copy, occupancy observation, bounded directory resolution and journal operations. Existing copy helpers preserve raw link text; they require the new payload validation and relative-link transformation, and cannot be treated as already implementing this specification.

### Copy and name resolution

- Resolve directory collisions by Directory Identity, including the existing Unicode normalization and case-folding rules. The physical directory spelling comes from the chosen Skill or existing project entry; display metadata is not a collision key.
- When a valid copy already exists, preserve it and link selected Agents to it. Verify a usable Skill directory and self-contained links; do not require byte equality with the selected Library Skill. Reuse is read-only, including no dependency cleanup or removal of existing Git metadata.
- When no copy exists and multiple selected Skills share its Directory Identity, choose one source per project copy. All dependent Agents follow that choice. No choice means this identity is skipped; other identities continue. Global per-Target winner semantics remain unchanged.
- Ordinary files, dangling links and unusable or non-self-contained entries at the copy location remain untouched and block that Skill. Old links at the base location pointing outside the project must not be treated as valid copies or silently overwritten. Existing safe project-local directory aliases remain subject to bounded resolution and identity checks.
- Copy all regular payload content, hidden files and dependency directories without interpreting `.gitignore`; exclude entries named `.git` used as Git metadata. Preserve executable permission bits. Special filesystem objects unsupported by the safe copy seam fail explicitly instead of yielding incomplete successful copies.
- Validate all included payload links. Links whose resolution stays inside the source Skill are recreated relatively within the copy; external, dangling and cyclic links block the Skill. A link to excluded content must not become a dangling link in the copy. No external content is automatically imported.
- Source health and Home/WriteGate gates for new Enable continue to apply; this feature does not introduce a way to bypass Source Snapshot Mismatch or other closed source states. Copy staging must be consistent with the frozen source observation, and incomplete staging must not appear as the completed project copy.

### Directory and Agent link safety

- Resolve the base directory and configured Agent directories within the canonical project root. Every symlink hop and final container must remain inside the project; reject escape, cycles, unresolvable paths and unavailable containers. Safely creatable missing containers are disclosed by Preview.
- Calculate Agent link text relative to its resolved physical parent and the ready project's copy. Verify portability by moving the whole project; do not merely check that the link text lacks an absolute prefix. Existing absolute directory aliases cannot satisfy relocation guarantees merely because their current target is inside the project: report them as incompatible rather than silently rewriting user-owned aliases.
- Deduplicate physical shared containers and disclose all configured affected Agents. A target resolving to the copy container reuses the directory without constructing a self-link. Overlapping destinations must never allow an Agent overwrite to destroy a copy, its ancestor or another planned output; such invalid relationships block the affected actions.
- An existing correct self-contained relative Agent link is NoOp. Other same-name Agent entries, including legacy external links, require explicit per-entry replacement confirmation. Real directories show directory/file counts; retain the existing restriction against indiscriminate directory Replace all.
- Occupancy confirmation authorizes only the previewed object. Changed occupant identity, container identity or link chain requires a refreshed plan or a typed rejection, not an expanded overwrite.

### Execution, results and recovery

- Preview and cancellation make zero filesystem, Catalog, journal or MRU writes. It may hold a transient plan token. Freeze the relevant source/copy observations, project identity, directory hops, configuration/catalog generations and per-entry confirmations; revalidate before writes.
- Commit each new copy only after validated staging is complete; only then apply its dependent links. Existing-copy reuse also requires current readiness validation. A failed/blocked copy prevents all dependent writes.
- An Agent failure preserves the ready copy and successful sibling links. Return explicit Succeeded, NoOp/reused, Skipped, Failed and Not attempted facts with understandable dependency reasons; never summarize partial success as full completion. Independent Skills continue unless Home/WriteGate or a recovery-required condition closes further writes.
- Before replacing an Agent entry, preserve its original contents in the operation backup. Failure during that replacement must restore the original when safe; inability to restore becomes an explicit recovery condition, retaining recoverable data.
- Extend the existing crash-safe journal to record project action kinds, owned artifacts, copy-link dependencies, original occupant evidence and durable commit phases. Recovery must work without project Activation records and without recopying a newer Library version. Preserve completed independent actions; roll back incomplete actions when proven safe. Uncertain identity or externally modified data remains preserved with a typed recovery requirement.
- Undo proceeds from dependent links to copies. Restore confirmed original Agent occupants, then remove only unchanged copies created by this operation whose dependent links were safely undone. A refused link Undo retains its copy. Pre-existing copies and externally modified copies are never removed. Continue other safe Undo actions and report partial Undo.
- Finalize closes the result-page Undo window and cleans only safely disposable operation artifacts. Restart uses startup recovery before finalization; it must not discard a needed original backup merely because the UI session ended. Repeated recovery/finalize must remain safe. Legacy journals and global recovery retain their existing supported semantics.
- Keep only the existing bounded recent-project-folder convenience history after a successful write, including copy-only success. Preview, failures and pure reuse/NoOp do not invent persistent project distribution state. Retry always re-plans against the current filesystem.

### User interface

- Keep the existing single- and multi-Skill project entry points. Present the required base copy destination independently of optional Agents, including when no Agent is selected or configured.
- Preview separates “copy from Library”, “use existing project copy” and “link for Agent”; show source winner choices, shared consumers, blocked paths and each replacement's impact before Apply. Explain that Agents reading the required/shared directory may see its Skills even when not selected individually.
- The result view reports copy readiness and each Agent outcome, offers safe Undo during the existing window, and supports a new Preview for retry. Closing a result finalizes it; cancelling a preview does not mutate anything.
- Use 已分发 / 未分发 / 部分分发 only for the existing distribution semantics. Project copy/link results are operation results, not a new persistent project DistributionState. Agent discovery, trust and execution are not inferred from successful writes.
- Localize App Copy in English and Simplified Chinese. Preserve Source Content, user paths, Skill names and diagnostics as data, with typed localized explanations around them.

## Testing Decisions

测试 seam 已确认。优先一个主要行为入口，已有恢复入口和 UI 测试仅补足它不能表达的生命周期与交互。

- **主要入口：EnableApi。** 通过真实 request/response DTO 执行项目 plan、apply、undo、finalize，连接真实 Core 与隔离临时 Home/项目文件系统。既有项目分发集成测试已覆盖 EnableService 的同一流程，可复用其环境搭建并把新验收提升至 API；不以私有函数调用、数据库行数或内部调用顺序作为业务正确性断言。
- **恢复入口：既有 StartupMaintenance / MaintenanceService 启动恢复。** 在受控文件系统故障或持久化阶段中断后重新创建运行时，走正式恢复入口，观察原条目、完整副本、链接和公开错误。底层 filesystem seam 可用于故障注入，不能以“某个 helper 被调用”代替结果验证。现有全局 Enable journal 恢复测试可提供故障场景先例，但项目恢复不能依赖其全局 Activation facts。
- **UI 入口：ProjectEnableSheet + typed CatalogClient。** 复用现有 Testing Library 交互方式，验证空 Agent 选择可继续、必须目录提示、同名来源选择、逐项覆盖确认、复用说明、部分成功及部分 Undo、中英文文案。通过用户动作与可见内容断言，不锁死组件内部状态或静态布局。

必须覆盖以下行为矩阵：

| 场景 | 可观察的验收结果 |
| --- | --- |
| 单 Skill、零 Agent | Preview 零写入；Apply 只交付完整副本，结果准确，Undo 安全移除本次副本 |
| 多 Skill、多 Agent | 每个 Directory Identity 一份副本；各 Agent 相对链接指向它；同物理目录只写一次 |
| 项目已有修改过的副本 | 内容与权限保持原样；新增 Agent 读到项目版本；UI 明示复用 |
| 同名普通文件、无效 Skill、外部链接或断链 | 原物保留；该 Skill 被阻止；无依赖链接写入 |
| 多来源同名、含大小写或 Unicode 等价名字 | 显式 winner 决定唯一副本；未选只跳过该 identity；已有有效副本时复用 |
| Agent 同名真实目录、文件、旧外部链接 | 未确认保持原物；确认后备份并替换；Undo 恢复原物；正确相对链接 NoOp |
| 隐藏文件、依赖、脚本、Git 元数据 | 包含规定 payload 与执行位，排除 `.git`，不按 `.gitignore` 丢文件 |
| 内部相对/绝对 payload 链接 | 合法内部目标转成副本内相对链接；移动项目并隔离原 Home/来源后仍可读取 |
| 外部、循环、断裂 payload 链接及特殊文件 | 精确路径被报告；无完成态的残缺副本或依赖链接 |
| 项目内目录别名、共享容器、越界或绝对别名 | 合法且可迁移的别名可用；不自链、不覆盖产物；越界或不可自包含者阻止 |
| Preview/取消、配置或根目录替换、源/副本变化 | Preview/取消零写入；stale 不覆盖新占用物，不提交混合版本内容 |
| 副本失败、单 Agent 失败、其它 Skill 成功 | 副本失败无依赖链接；Agent 失败保留成功项；结果可解释每项状态 |
| Undo 时链接或副本被外部改动 | 原有/已改副本保留；失败依赖保留新副本；其它安全项继续撤销 |
| 拷贝暂存、备份后、链接创建后、提交或 Undo 中断 | 重启恢复保持完整内容、可恢复原物和正确依赖；重复恢复安全 |
| 无项目生命周期副作用 | 公共全局分发查询状态不变；来源更新/删除不改变项目副本；纯 NoOp 不产生新成功历史 |
| 全局回归与旧 journal | 原全局 Enable/Disable/Repair、target winner 与恢复行为保持有效 |

实现阶段执行针对性 Rust 集成测试、DTO/UI 测试，再执行 Apple Silicon Mac 上的 `npm run ci:local` 完整门禁。GitHub-hosted Actions 不是所需门禁。原生人工验收覆盖目录选择、覆盖确认、结果页、移动项目后链接读取和重启恢复；自动测试与人工验收分别报告。完整行为矩阵覆盖 #101–#104，不能将 #101 的自动测试通过视为全部规格或原生人工验收已完成。

## Out of Scope

- Project 注册实体、持久项目 Activation、长期分发状态、项目 Disable、健康检查、Repair、自动同步或长期任意 Undo。
- 用 Library 内容更新/覆盖已有项目副本，或把项目修改反向纳管为 Local Source。
- 全局 Activation 语义、Agent 预设路径的兼容性调研、Agent 优先级、信任门或实际加载执行保证。
- 跨全部 Skill/Agent 的全局原子事务；本功能承诺明确的部分成功与安全恢复。
- 依赖安装、环境重建、自动导入外部链接内容、改写脚本中的绝对路径、解释 `.gitignore`、Git 自动提交或仓库配置变更。
- 复制元数据的完整归档保真（如 owner、ACL、扩展属性、时间戳或硬链接关系）；需保证规定内容、相对链接与脚本执行权限。

## Further Notes

- 决策依据为 ADR-0025；ADR-0015/0019 的项目分发部分按 ADR-0025 取代，全局规则保持有效。CONTEXT 中 Project Skill Copy 与 Activation 的区分适用于代码、DTO 和 UI。
- 用户要求的 `.agents/skills` 等路径属于产品契约；实现模块以概念名引用，不固定源代码文件布局。复制内容与可用性的边界是文件系统自包含，不等于第三方运行环境可以迁移。
- 按依赖理解实现范围：副本计划与安全物化是 Agent 链接的前提；有依赖的 journal/Undo/恢复必须随写入能力交付，不能作为可延期的安全补丁。后续 tickets 应按端到端可验收行为拆分，并声明实际阻塞边。
- GitHub issue 是发布后的规格权威，本地文件保留发布正文。实施任务已拆分为 #101–#104，当前进度见 ADR-0025。
- 发布时 ADR-0025 与相关领域文档尚未提交；本规格完整包含所需行为契约，不依赖尚未存在的远端 ADR 链接。
