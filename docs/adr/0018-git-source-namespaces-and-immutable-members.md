# Git Source namespace、不可变成员快照与来源级版本策略

状态：Accepted

[决策：Git 来源 Skill 的 Library 布局与命名空间身份](https://github.com/RookieZoe/skill-man/issues/74)确认：Git Repository Source 是版本、成员变动、Update 与 Remove 的唯一聚合；Source Member 是不可单独 Update/Remove、不可在 Home 内修改的当前快照。稳定 `remote_id` 与 `skill_id` 形成 Home namespace，repository 坐标与 `skillPath` 只作来源事实。这样既允许完整 Source Release 中的同名成员进入 Library，也避免 repository rename、成员改名或 URL alias 触发无关的 Home 路径迁移。

本 ADR 取代 [ADR-0014](0014-git-repository-source-releases-and-transitions.md) 的“唯一具体 tracking ref”、Modified Member Resolution、Explicit Member Mapping、Upstream Member Removed 保留内容与逐成员 Remove 语义；取代 [ADR-0003](0003-symlink-strategy.md) 对 Git Source Member 的全局目录名身份、flat `skills/<name>` 布局和 Library 重名阻断。ADR-0014 的来源级 journal/CAS/recovery/Undo、完整成员不可裁剪、Legacy Source Promotion、external ownership 和 fail-closed 事实继续有效；非 Git sourceType 的既有行为不因本 ADR 改变。

## 身份与 Conflict

- 每个 Managed Skill 使用稳定 `skill_id`；目录名是 **Directory Identity**，只决定显示名称和 Activation 在平面 Agent Activation Target 中占用的 entry name。
- Source Member 由 `(remote_id, repository-relative skillPath)` 识别，并关联一个稳定 `skill_id`。同一路径暂时消失后重新出现时复用该 `skill_id`；路径改变是旧成员删除加新成员新增，不按名称、hash、共同祖先或相似度推断 rename。
- 同一或不同 Git Repository Source 的同名 Source Member 可以同时存在于 Library；它们按来源和 `skillPath` 区分。非 Git 与非 Git 的既有 Library Conflict 继续有效。Git 成员的同名关系只有在同一 Agent Activation Target 争用相同 Directory Identity 时才成为 Activation Conflict。
- Create Local Source Copy 是显式 Git 出口，可以让原 Git 成员与同名 Local Source 共存；它不改变其它非 Git Import/Adopt 的 Conflict 规则。

## Repository identity 与 Source Tracking Policy

- Git Repository Source 由稳定 `remote_id` 标识。provider adapter 可证明的 scheme/host/default-port/`.git` 等纯语法等价直接规范化；rename、transfer 或 redirect 只能在 Preview 中由用户确认后更新 canonical repository 并保留旧 alias。fork 始终创建新的 `remote_id`。
- 默认 **Source Tracking Policy** 在每次 Fetch Latest/Update 时依次选择：最新正式 provider Release 对应的 tag；否则最高稳定 SemVer tag；否则 remote default branch 可达、按创建时间及 ref name 确定排序的最新普通 tag；都不存在时才使用 `HEAD`。
- draft 与 prerelease 不进入默认 policy。用户可显式覆盖为 prerelease channel、固定 tag、固定 commit、指定 branch 或 `HEAD`。
- Source Release 不是 provider Release。它是一次 policy 选择得到的 selected ref/tag、resolved commit 和完整 Source Member/tree manifest；resolved commit 才是不可变版本事实。Source Member 没有独立 ref、commit 或版本。

## Home 与 Catalog

```text
<Home>/
├── skills/git/<remote_id>/<skill_id>/   # 当前只读 Source Member 快照
└── remotes/<remote_id>/source.json      # source identity/policy/current release manifest
```

Catalog 与 `source.json` 共同证明 `remote_id`、provider、canonical repository/aliases、Source Tracking Policy 与 current Source Release。每个成功提交的 Source Release 保存不可变完整成员 manifest，以及 `skill_id`、当时 `skillPath`、Directory Identity 和 tree hash 的关系；Preview/Draft 不持久化。旧 release 只保留 metadata/audit，旧字节只在 Source Undo 窗口的隔离副本中存在。Home 不保存 checkout、`.git`、credential、外部 lock 全文或其它旧 Source Content。

Link 的 durable pointer 是 Catalog 中 Local Source 的 canonical 最终实体路径；Home 不再维护第二条 Library 指针软链。Activation 对所有来源都从 Catalog 解析并直指最终实体。

## Source-level lifecycle

- Fetch Latest and Manage、Source Promotion 与 Update 都获取一个完整目标 Source Release，并以单一 Source Group Confirmation 和 Source Transition 提交；成员不能逐项 Include、Update 或 Remove。
- 新 `skillPath` 创建新的 Source Member/`skill_id`，进入 Library Desk 对应 Git Repository Source 分组，默认不 Enable。消失路径在已确认 Source Transition 中删除当前快照；path rename 是删除加新增，不做 Explicit Member Mapping。
- 普通 Remove 删除完整 Git Repository Source；每个当前 Source Member 仍可独立 Enable/Disable。
- Git Source Member 不支持 Modified。快照以只读权限保护，并在启动和来源写操作前按 current Source Release tree hash 重验。字节不一致是 **Source Snapshot Mismatch**：阻止 Update、新 Enable 与普通来源写，不静默覆盖；只读查看、Disable、把当前观察字节 Create Local Source Copy，以及显式 Restore Current Source Release 仍可用。

## Activation 与成员 tombstone

成员消失或改名不自动重写、替换或清理 Activation。它的 `<Home>/skills/git/<remote_id>/<skill_id>/` 消失后，既有全局 Activation 保持 dangling 并进入 Broken；Catalog 在整个 Git Repository Source 生命周期内保留最小 **Source Member Tombstone**，并把 Activation ownership 保留到用户 Disable。同一 `(remote_id, skillPath)` 在后续 Source Release 重新出现时恢复原 `skill_id` 和快照路径，相关全局 Activation 自动恢复 Healthy。

只有受 Catalog 管理的全局 Activation 进入启动健康检查。ADR-0015 的项目级一次性软链仍不记录、不检测、不修复，成员消失后的处置由项目自行负责。

## Library Desk 与 Create Local Source Copy

Library Desk 按 Git Repository Source 分组；来源组显示 repository、tracking policy、selected ref/tag、resolved commit、Update/Remove 和 Source Release 状态，成员行显示 Directory Identity、`skillPath`、详情、Enable/Disable 与 Activation 健康。同名成员不改写 Source Content 名称，以来源组和 `skillPath` 区分。

**Create Local Source Copy** 对一个 Source Member 执行：用户选择 Home、Global Skills Root、installer root 与 App state 外的稳定目录；Skill Man 复制当前稳定观察到的字节（Healthy 时即 current Source Release 快照）并把目标 canonical 最终实体路径登记为新的 Local Source；原 Git 来源和成员保持不变，不自动切换 Activation。副本不含 `.git`，后续内容由外部工具拥有，Skill Man 不保存内容 baseline 或判断内容变化。需要完整 Git workspace 时，用户用外部工具 clone，再通过 Link 加入对应 Skill。

## Consequences

Catalog 不能再以 Directory Identity 全局唯一约束代表 Managed Skill identity；非 Git Library Conflict 必须由来源感知的领域规则执行。Git source schema 必须持久化 tracking policy、selected ref/tag、immutable release-member mapping、稳定 storage namespace、current/absent member 状态和 tombstone；Source Snapshot Mismatch 必须是独立于 Modified 的 closed state。既有实现若仍使用 flat `skills/<name>`、逐成员 baseline/mapping 或固定 tracking ref，只属于旧能力，不能冒充本 ADR 的 Git Repository Source。
