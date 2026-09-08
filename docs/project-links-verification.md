# 项目内多 Agent 相对链接验证（#102）

范围：单 Skill、必需 Project Skill Copy，以及可选附加 Agent。确认覆盖和批量来源选择仍由 #103、#104 交付。

## 自动验证

- `EnableApi` + 临时真实文件系统：零 Agent、新建/复用、多 Agent 同内容、共享物理容器与全部消费者披露、相对目录别名、正确相对链接 NoOp、冲突保留、越界/循环/绝对别名与产物重叠阻止、项目移动后隔离来源仍可读取。
- 运行期：只读 Agent 容器失败后保留副本和成功兄弟链接；修复权限后重新 Preview 并完成缺失链接。副本目录不可写时，依赖链接全部 Not attempted。复用副本后成功新增链接会更新 MRU，纯复用不更新。
- Undo：先撤销链接；外部修改的条目保留，安全兄弟链接继续撤销，副本保留。父目录描述符和 occupant 身份在删除前重新核对。
- 正式启动维护入口：执行中断、链接写前中断、链接写后但未捕获身份、Undo 开始和部分链接已移除等持久化状态；重复启动不丢失已完成的独立动作，不依赖来源最新内容或 Activation。身份未捕获时保留数据并报告恢复需求。
- `ProjectEnableSheet` + typed `CatalogClient`：零 Agent、多 Agent、必需副本与依赖链接、占用物不提供替换、部分成功、部分 Undo、重新 Preview 后重试。
- 针对性结果：`project_skill_copy` 23 个测试通过；`ProjectEnableSheet` 9 个测试通过。
- 双轴审查：Standards 与 Spec 复查无剩余问题。已修复复用副本后成功新增链接遗漏 MRU 的问题，补齐链接身份未捕获的启动恢复测试。

## 完整本地 CI

`npm run ci:local`：最终通过。37 个前端测试文件、288 个前端测试、691 个 Rust 测试通过；格式、capability、locale、lint、typecheck、release scripts、Web 构建和 Apple Silicon Tauri 原生构建均通过。未使用 GitHub-hosted Actions。

## 原生验收

2026-09-08，通过本次构建的 Apple Silicon `.app` 完成 CUA 原生操作验收。使用现有健康 Skill `build-iterated-agentic-loop` 和现有 Codex、General、omp 配置，全部项目写入可丢弃的 `skillman-102-native-rv2pz4bn` 临时目录，未更改 Agent 配置、来源内容或全局 Activation。

- 共享目录：`.codex/skills → ../.agents/skills`。Preview 披露 Codex、General 共用必需副本，另为 omp 创建逐 Skill 相对链接；结果为 `2 / 2 项已成功`。三入口 `SKILL.md` SHA-256 相同，omp link text 为 `../../.agents/skills/build-iterated-agentic-loop`，基础目录没有自链接。
- 部分失败：另一临时项目中 `.codex/skills` 权限为 `0555`。结果为 `2 / 3 项已成功`，Codex Failed，副本和 omp 链接 Succeeded；真实文件系统确认成功入口仍可读取。
- 重试：恢复该临时容器至 `0755`，从结果页“重新预览”继续。Preview 明示使用项目已有副本；Apply 为 `3 / 3 项已成功`，副本与 omp 为 NoOp，Codex Succeeded；三入口内容摘要相同。
- 部分 Undo：仅将本次新建的临时 Codex 链接改成外部内容，点击“撤销本次操作”。界面显示部分撤销并禁用重复 Undo；外部条目与已有副本保留。关闭结果成功。
- 已检查原生中文及浏览器 fixture 英文预览/结果的实际渲染。截图发现结果名称、状态和长诊断拥挤，已改为名称/状态分列，技术详情折叠；保留原有模态布局和样式变量。此呈现调整已在浏览器实际渲染中复验，并纳入最终 CI 原生构建；开发 fixture 也已修正，项目 Apply 不再委托给全局流程。

这些结果是本次本机构建验收，不代表已签名 Release、发布或 issue 关闭。临时项目路径仅作为本次 QA 产物；成功操作按产品契约记录 MRU，未清空用户原有最近记录。
