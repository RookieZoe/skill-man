# Install 来源、发现与更新规则

Skill Man 的 Install 兼容 [`vercel-labs/skills`](https://github.com/vercel-labs/skills) CLI 的来源解析、两阶段 Skill 发现、多选与 lock 元数据语义，但由 **Rust 后端原生实现**；不调用 `npx`，不要求用户安装 Node/npm，也不让上游 CLI 直接操作 Agent 目录。所有 Install 只写入 Library，分发给 Agent 仍由 Activation 负责。

## 远程来源与发现

MVP 支持公开的 GitHub `owner/repo` shorthand、GitHub/GitLab 完整 URL（含 `tree/<ref>/<subdir>`）和通用 Git HTTPS URL；不管理 OAuth、PAT 或 SSH 凭据。私有仓库由用户先 clone 到本地，再用 Link 或从文件 Install。

发现严格采用 `skills` CLI 的两阶段语义：先扫描仓库根、`skills/` 等标准位置和已声明的插件目录；没有结果时再递归扫描。高级选项可强制全深度扫描。发现一个 Skill 时直接预览，发现多个时允许多选；每个选中的含 `SKILL.md` 目录分别成为一个 Managed Skill。

格式兼容性不作为 Install 的硬门槛：只要目录名安全且 `SKILL.md` 可读即可进入 Library。解析 frontmatter 后按 Agent 显示兼容性警告；Enable 到明确不兼容的 Agent 前再次确认。

## 本地来源

“从文件安装”支持文件夹与 `.zip`，沿用相同的两阶段发现与多选。单独选择 `SKILL.md` 时要求改选其父目录。文件来源是一次性快照：记录原始文件名/路径、安装时间和内容 hash，仅供溯源，不跟踪原路径、不检查更新；新版通过重新从文件安装进入替换流程。

## 安全与原子性

远程内容、本地文件夹和 ZIP 都先进入 staging。ZIP 必须拒绝路径穿越；Skill 内只保留解析后仍位于所选 Skill 根目录内的相对软链，绝对链接、越界链接和 dangling 链均拒绝安装。全部发现、格式与冲突校验通过后，才以稳定路径 `<Library>/skills/<name>` 原子移入或替换；失败时旧实体与 Activation 保持不变。

Library 内同名仍遵循 ADR-0003：普通 Import 阻止并引导改名。显式 Update / 重新从文件安装是替换流程；替换同名稳定路径，因此现有 Activation 无需重建。

## 来源元数据与更新

SQLite 对每个远程 Skill 记录：source URL、requested ref、resolved commit、仓库内 skill path、content hash，以及安装、检查、更新时间。未指定 ref 时记录并跟踪远端默认分支；显式 branch 也持续跟踪；tag 和 commit 固定，不做后台更新检查。用户要从一个 tag 升到另一个 tag 时显式更换 ref。

应用启动后按冷却期后台检查可跟踪来源（默认每 24 小时最多一次），离线或检查失败不打扰用户。检查只显示更新，不自动应用；用户确认后才 Update。合集仓库只 fetch 一次，再按各 Skill 的 `skillPath + contentHash` 判断实际变化，UI 按仓库分组并允许逐个或批量更新。

Install 实体与已记录内容不一致时进入 **Modified** 状态。Update 禁止静默覆盖，用户必须选择“放弃本地修改并更新”或取消；长期开发应改用 Link。远端新版本中原 `skillPath` 消失时，保留当前本地版本与 Activation，提示用户重新选择仓库内路径、固定当前版本停止检查，或 Remove；不按名称自动猜测迁移，也不随上游自动删除本地 Skill。
