# 生产启动完整性与 fixture 恢复策略

Skill Man 的生产启动必须在打开可写 SQLite 和构造 Catalog 服务前完成 Home 身份校验与 fixture 分类,永远不得 seed 或 fallback 到 demo fixture;Fresh Home 必须是 Empty Library 加真实 Agent Preset 配置。发现历史 fixture footprint 时,应用先进入 Fixture Recovery Lock:只有完整的数据库与文件系统证据证明 fixture-owned 状态保持精确初始形状,且除明确白名单配置外不存在真实或未知关系,并且用户确认后,才能创建完整 Safety Snapshot、可逆隔离旧 Home 并准备干净状态;混合、未知或部分失败一律 fail closed。Fixture Recovery 不修改任何外部来源 Skill,也不触发 Adopt、Remove、Enable 或 Activation Repair。

Fixture Recovery 必须服从 Home Binding。Bound Home 的恢复只能替换同一 Home 身份下的内容,不得改写 locator 或静默 Relocate;尚未绑定的 Legacy Home 必须先完成识别与恢复,才可进入唯一一次首次绑定过渡。Reconnect 或 Restore 只有在验证为同一 `home_id` 时才成立;创建新身份只能通过用户明确执行 Abandon Home and Start New。whole-Home 隔离所需的最小 locator 与 durable recovery ledger 必须位于活动 Home 外,Safety Snapshot 也不作为活动 Home 使用。

本 ADR 在 Home Binding 与首次显式确认两项上取代 ADR-0007 的固定路径、不询问路径约束;Home 的内部 layout、bootstrap locator、identity/bookmark、Legacy 一次性过渡与不可用语义由后续 Home 决策定案。具体 fixture ID、历史行形状、内容 hash、SQLite/WAL/SHM 边界、cursor、fsync、故障注入与验收矩阵属于实现 spec 和[恢复决策](https://github.com/RookieZoe/skill-man/issues/34),不固化在本 ADR 中。
