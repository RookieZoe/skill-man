# 符号链接策略与冲突规则

Skill Man 的 Activation 管理与冲突处置规则。词汇遵循 [CONTEXT.md](../../CONTEXT.md),Activation 结构遵循 [ADR-0001](0001-activation-points-to-entity.md)(直指实体,不串联)。

**Activation 生命周期.**
- **Enable** = 在 agent skills 目录建条目级符号链接,直指 skill 最终实体(Install 来源 → `<Library>/skills/<name>`;Link 来源 → 源目录)。**只建条目级,绝不把整个 agent skills 目录做成软链**(Claude Code #38051 整目录软链回归史)。
- **Disable** = 删除 Activation。统一对所有 agent 删链,**不写 agent 原生开关**(Codex `config.toml` / Gemini `/skills disable` / Claude `skillOverrides`)——保持单一语义,原生开关记录为未来增强。
- **Remove** = Install 来源删除实体(连带先移除其所有 Activation);Link 来源仅断开 Library 引用,原地实体不动。

**健康检查(Claude Code #50052 的兜底).**
Claude Code 自动更新会静默删除 `~/.claude/skills` 下的软链(未修复确认)。Skill Man 在**每次启动时**校验所有「应为 Enable」的 Activation 是否仍在、目标是否有效,缺失者标出并提供**一键修复**(重建)。实时监听(agent 目录 fsnotify)不入 MVP。

**冲突规则(Conflict,两类).**
- **Library 内重名**:阻止入库,提示「已存在同名 skill」,引导改名后再入 / 移除旧的再入 / 取消。不自动加后缀、不建命名空间(Skill 身份=目录名)。
- **Activation 占用**:Enable 时目标位置已被 Untracked 实体/链接占用 → 提示三选一:Adopt 它(收编进 Library)/ 移除它并用 Library 版替换 / 取消。**绝不静默覆盖用户文件**。

**Broken 处置.**
Link 来源源目录失效 → 标红,详情页提供「重新定位 / 从 Library 移除」。**Activation 保留不动** —— dangling 软链对各 agent 无害(Codex 记 warning、Gemini/opencode 静默跳过),源恢复后自动复原,不主动清理。

**Library 布局.**
`<Library>/skills/<name>` 放 Install 实体与 Link 指针;`<Library>/` 下用 **SQLite** 存索引(来源元数据、各 agent 启用矩阵、健康检查状态)。Library 默认位置 `~/Library/Application Support/skill-man/`。
