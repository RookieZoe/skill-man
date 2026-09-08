# 项目级分发使用项目内副本

状态：Accepted（设计已确认；#101–#103 已实现，#104 待实现；原生验收状态见验证记录）

项目级分发将 Skill 复制到项目内必须存在的 `.agents/skills/<Directory Identity>/`，并为可选的其他 Agent 创建指向该副本的逐 Skill 相对软链。这样项目移动或分享后不依赖原 Skill Man Home 或外部 Local Source；代价是项目副本与 Library 来源分离，不再随来源变化。

## 已确认的决策

- 项目副本归项目自行管理。Library 来源更新或删除不改变副本；继续不注册 Project 实体，不追踪项目分发，不做自动更新或长期撤回。
- `.agents/skills` 是固定且必须存在的基础目录。分发界面必须明确提示；即使不选择附加 Agent，也可以只交付项目副本。
- 其他 Agent 的条目使用项目内自包含的相对软链，指向对应项目副本。例如 `.claude/skills/foo → ../../.agents/skills/foo`。不通过链接整个 skills 目录来分发。
- 重复分发时保留已有项目副本，不用 Library 内容覆盖；新选 Agent 链接到项目现有内容，并提示“使用项目已有副本，未复制 Library 内容”。同名位置若是普通文件、断链或不满足自包含要求的目录，则保留原物并阻止该 Skill 分发。
- Agent 目标已有同名条目时，执行前列出覆盖路径；真实目录显示目录、文件数量，用户确认后才覆盖。已有正确的项目内相对链接无需重建。
- 复制 Skill 时，指向该 Skill 内部的软链转换为副本内相对链接；指向 Skill 外部、断裂或循环的链接阻止该 Skill 分发，并说明具体路径，不自动复制外部内容。
- `.agents/skills` 及 Agent 项目目录允许目录软链，但每一跳与最终容器必须留在项目根内；任何越界都拒绝。Agent 解析到副本所在的同一容器时共用副本，不创建自链接。
- 副本就绪后才创建依赖它的 Agent 链接。副本创建失败时，不执行该 Skill 的任何依赖链接；单个 Agent 链接失败时保留副本及其它成功链接，逐项报告部分成功。不同 Skill 独立处理，重试重新 Preview。
- 结果页保留“撤销本次操作”：恢复被覆盖内容，只撤销本次变更，不删除原有项目副本；外部修改过的内容不强行撤销。结果页关闭或应用重启后的 finalize 继续沿用 ADR-0019，不提供长期任意回滚。

- 多个所选 Skill 争用同一 Directory Identity 时，项目尚无副本则由用户明确选择一个来源，所有所选 Agent 共用该副本；未选择则跳过此名字，不影响其它 Skill。项目已有可用副本时保留并复用，不再选择来源。这一选择以项目副本为单位，不能让不同 Agent 各自选择不同来源却链接同一副本。
- 复制完整 Skill 内容，包括隐藏文件和依赖目录，但排除 `.git` 元数据；保留脚本执行权限，不解释 `.gitignore`。所有复制内容均受内部软链规则约束，包括依赖目录；指向被排除内容的链接不能成为副本中的断链。自包含约束针对文件链接，不承诺复制的虚拟环境可直接运行，也不改写脚本或配置中的路径。
- Undo 先撤销 Agent 链接，再删除本次新建的副本。只要有依赖链接无法安全撤销，就保留副本并报告部分撤销；副本被外部修改时同样保留。原有副本始终不删除。

## 取代范围

本设计取代 ADR-0015 与 ADR-0019 中项目条目直指 Catalog 最终实体、项目操作必须选择 Agent、项目内同名 winner 按各 Agent 目标分别选择，以及项目副本与依赖链接可无依赖地独立提交的相关约束。全局 Activation 仍遵守 ADR-0001；全局按 Target 选择 winner 的规则不变，项目产物不属于 Activation。

实现继续遵守 ADR-0019 的零写入 Preview、执行前现场重验、覆盖备份与 crash-safe journal 约束，补充本设计的副本及链接依赖顺序。当前实现范围见下节；设计确认、代码实现与发布验收分别记录。

## 实现状态

- [#101：项目副本交付与复用](https://github.com/RookieZoe/skill-man/issues/101) 已在代码提交 `511012d` 实现单 Skill、零附加 Agent 的完整流程：零写入 Preview、新建或保留复用副本、内部链接转换与自包含校验、安全 Undo、finalize 和启动恢复。即使没有 General Agent 配置也可交付副本。
- [#102：多个 Agent 相对链接](https://github.com/RookieZoe/skill-man/issues/102) 扩展同一 Enable operation：必需副本优先，可选 Agent 按解析后的物理目录去重，共用项目内相对链接；计划披露所有受影响的已配置 Agent。正确链接 NoOp，其它占用保留并报告冲突。结果逐项展示，重试重新 Preview。
- 依赖链接 journal 不参与 Activation；Undo 与正式启动恢复先处理链接，依赖无法安全撤销时保留副本。链接写入后、身份尚未持久化时若中断，恢复保留数据并报告 Recovery Required。
- [#103](https://github.com/RookieZoe/skill-man/issues/103) 已实现原 Preview token 内逐项确认覆盖 Agent 条目，同目录排他重命名备份原物，Undo/启动恢复保留无法安全恢复的备份，并通过 typed `recoveryRequired` 呈现部分 Undo 的恢复需求。验证见 [#103 验证记录](../project-replacement-verification.md)。
- 当前入口仍阻止多 Skill；批量分发由 [#104](https://github.com/RookieZoe/skill-man/issues/104) 交付。不会回退到旧项目外直链。验证及原生验收状态见 [#102 验证记录](../project-links-verification.md)。
- 当前安全快照有 128 MiB 的常规文件内容总量上限，超限时拒绝处理，不截断内容；这属于当前实现限制，不代表完整规格已经验收。
- #101 的针对性测试、完整本地 CI 与代码审查已通过；原生人工验收尚未完成。Issue 关闭及应用发布分别跟踪，不以本记录或代码推送代替验收。
