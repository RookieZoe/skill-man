# 架构决策记录

ADR 保留历史决策；后续 ADR 只取代明确列出的范围，不按编号自动废弃全部旧决议。词汇和界面语义以 [CONTEXT.md](../../CONTEXT.md) 为准。

## 当前实现补充记录

以下记录基于 2026-09-08 的 `main` 提交 `bbb1fa4a2aa4f0f442de45e61bcbae9fd3af5c23`，描述已实现行为，不是未来需求或发布验收声明。

- [ADR-0023：桌面工作区、技能阅读与分发交互](0023-implemented-desktop-workspace-and-distribution.md)：三工作区布局、分发状态、稳定面板、Markdown 阅读、智能体呈现和原生窗口约束。
- [ADR-0024：本地 Skill 观察、扫描结果投影与显式迁移](0024-local-skill-observation-and-migration.md)：依赖排除、持久忽略、局部结果投影、迁移弹窗、逻辑观察与完整物理快照。

## 相关既有决策

- 基础技术与实体：[ADR-0002](0002-tauri-v2-react-stack.md)、[ADR-0001](0001-activation-points-to-entity.md)、[ADR-0003](0003-symlink-strategy.md)。
- 界面语言与 Home：[ADR-0011](0011-interface-locale-and-message-ownership.md)、[ADR-0012](0012-skill-man-home-binding-and-unavailability.md)。
- Git 来源与版本：[ADR-0014](0014-git-repository-source-releases-and-transitions.md)、[ADR-0018](0018-git-source-namespaces-and-immutable-members.md)。
- Agent 与分发：[ADR-0015](0015-project-level-enable-target-only.md)、[ADR-0016](0016-agent-configurations-global-roots-and-shared-targets.md)、[ADR-0019](0019-enable-surfaces-target-resolution-and-batch-semantics.md)。
- 扫描与选择：[ADR-0017](0017-canonical-scan-aggregation-and-source-attribution.md)、[ADR-0020](0020-startup-observations-and-manual-rescan.md)、[ADR-0022](0022-library-file-manager-selection.md)。

早期视觉方案保留在 [ADR-0009](0009-ui-information-architecture.md) 与 [ADR-0021](0021-agent-management-and-enable-ui-architecture.md)；当前覆盖关系见 ADR-0022、ADR-0023。其余历史 ADR 继续保留原编号与内容。
