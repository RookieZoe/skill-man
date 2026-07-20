# CLI 与无头自动化接口的产品范围

Skill Man 的 MVP 不提供自身 CLI，也不提供 URL scheme、本地 API、AppleScript、Shortcuts 或其他正式的无头自动化接口；所有会读取或改变产品状态的受支持操作均通过 GUI 发起。外部 `vercel-labs/skills` CLI 仍只作为 [Install 来源语义](0004-install-sources-and-updates.md)的兼容参照，不是运行时依赖，也不构成 Skill Man 的用户接口。

这一取舍避免在首个版本同时引入第二个写入口，以及随之而来的 SQLite、Library、staging、补偿 journal 和 Agent 目录的跨进程一致性问题。MVP 已包含 Import、Update、Enable / Disable、Adopt、健康检查与恢复等有事务要求的操作；在这些边界尚未通过真实 GUI 工作流验证前，提前承诺 CLI 命令、稳定输出契约或多客户端架构会扩大状态空间，并迫使产品过早决定非交互冲突、确认与 Undo 语义。

## Rust 核心边界

CLI 后置不意味着业务逻辑可以绑定到 Tauri UI。MVP 的业务规则、状态变更、文件系统与 SQLite 事务应落在 UI 无关的 Rust 核心中，并以事务与回滚测试作为 spec 验收条件；Tauri command 只承担界面适配。该边界是内部实现约束，不是公共 API：MVP 不为未来 CLI 预定义参数、命令名、DTO、JSON 输出、错误码、兼容承诺、进程间 IPC 或跨进程锁。

## 重新评估条件

CLI 作为非阻塞的 post-MVP 候选单独追踪，不承诺交付版本，也不阻塞首个可开工 spec。只有同时满足以下条件时才重新进入产品设计：

1. 已出现 GUI 无法满足的具体终端、脚本或自动化需求；
2. UI 无关 Rust 核心的事务、回滚与崩溃恢复边界已经由 GUI 实现和测试验证。

重新评估时，`list`、`doctor`、`enable` 等只能作为用例示例，不能视为已承诺的最小命令集。任何实现开始前必须另行形成并发 ADR，在 GUI IPC 单写者、独立本地服务、跨进程锁等方案之间作出选择，并同时定义非交互确认、Conflict、Modified、Adopt / Undo、分发、版本兼容与机器可读输出契约。

## Consequences

首个 spec 可以按单一受支持入口设计，不需要实现 GUI/CLI 并发、命令安装或 shell 自动化测试矩阵，同时仍通过清晰的 Rust 核心边界保留未来扩展空间。代价是 MVP 用户不能从终端或自动化工具查询、诊断或改变 Skill Man 状态；内部 Rust 接口也不得被文档化为隐藏或实验性产品接口。
