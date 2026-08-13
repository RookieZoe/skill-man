# 统一 Skill Man Home 的内部边界、首次绑定与不可用语义

Skill Man 需要一个统一的 Home:它承载 Catalog 持久化内容与恢复所需的 Home 内状态,首次启动通过显式确认建立不可变 Home Binding,并在位置不可用或身份不匹配时 fail-closed。本 ADR 定案 Home 内部目录契约、Home 外的 bootstrap locator 与身份证明、首次绑定状态机、Legacy Home 的唯一一次绑定前过渡,以及 HomeUnavailable / HomeIdentityMismatch / Reconnect / Restore / Abandon 的语义;它取代 ADR-0007 中固定 Library 路径、不询问路径与 MVP 无迁移的表述,并落实 ADR-0010 委托给本 ADR 的布局、locator、identity 与不可用细节。词汇遵循 [CONTEXT.md](../../CONTEXT.md)。

## 1. Home 目录契约

默认 Home 路径保持 `~/Library/Application Support/skill-man`,与 Legacy 固定路径相同:Legacy 用户接受 Default 时原位绑定、零搬移;Legacy 识别也以该路径存在为触发。Choose… 只提供自定义位置。

| 路径 | 职责 | 可重建 |
|---|---|---|
| `<Home>/skill-man.sqlite3` | Catalog SQLite(WAL);`catalog_meta` 记录 home_id 与卷身份 | 否 |
| `<Home>/.skill-man-home.json` | Home marker:home_id、卷身份、created_at、schema_version | 否 |
| `<Home>/skills/` | Install 实体,稳定 per-skill 路径 | 否 |
| `<Home>/remotes/` | remote source 父目录,每 remote 一个子目录 | 否 |
| `<Home>/operations/` | operation journal 与 backup,崩溃恢复依据 | 否 |
| `<Home>/cache/` | git mirror 等派生缓存 | 是,启动缺失即重建 |
| `<Home>/staging/` | 操作暂存 | 瞬态,启动清理 |
| `<Home>/fixture-entities/` | 仅污染的 Legacy Home 存在;恢复后消失 | — |

`remotes/` 与 `skills/` 同级、互不为父子;remote-id 的解析与认领规则由 Adopt 来源分类决策定案,本 ADR 只定布局。

## 2. App-level 状态目录与 bootstrap locator

Home 外新增 `~/Library/Application Support/skill-man-state/`(与默认 Home 同级、目录名互不包含),存放:

- `home-binding.json` — bootstrap locator,绑定状态的唯一 truth;
- `recovery-ledger.json` — durable recovery ledger(对齐 ADR-0010 的 tmp→fsync→rename→parent fsync 提交协议);
- locale 持久值(具体格式按 ADR-0011 实施 spec;本 ADR 指定其物理归属,满足「locale 独立于 Home」)。

选择独立文件而非 UserDefaults plist:locator 需要显式 fsync 提交协议与可审计性,cfprefsd 语义不透明。

**身份三方证明**:首次初始化生成 UUID v4 `home_id`,同时写入 bootstrap locator、Home marker 与 SQLite `catalog_meta`;卷身份 = `statfs` f_fsid + APFS volume UUID(外置卷卸载/重挂不变,卷被替换或克隆则变)。启动时三方一致才处于 Bound;locator 缺失或矛盾按 §5 AppStateUnavailable fail-closed,不自动补写或猜测。

## 3. 首次绑定状态机:Unconfigured → Home Candidate → Bound Home

- **Unconfigured**:无 binding。Use Default / Choose… 与 locale 控件可见;取消保持 Unconfigured,不创建任何目录或 SQLite。首启向导门控由绑定状态驱动;`first_run_completed_at` 仍在绑定成功后写入(既有 schema 保留)。
- **Home Candidate 校验**(确认前,只读):绝对路径;解析后路径不得含 symlink 组件;不得位于任何 Agent skills 目录内或为其父/子;不得等于或位于状态目录内;目标不存在时父目录须存在且可写;已存在时必须为空(可复用);含任何内容一律拒绝,不「收养」;候选卷可用空间 ≥ 100MB。
- **确认与初始化**:显式按钮确认后:创建目录 → 建 SQLite(schema + home_id + 卷身份)→ 写 marker → 校验(integrity_check、marker↔SQLite home_id、目录指纹)→ 原子提交 locator → Bound。
- **崩溃恢复**:locator 提交是唯一 commit 点。提交前崩溃 → 下次启动仍 Unconfigured,检测到未绑定候选(marker + SQLite、无 binding)→ Continue 复用候选完成提交,或经确认 Cancel 删除 app 自建候选(只删初始化产物,不涉用户内容)。

## 4. Legacy Home 识别与唯一一次绑定前过渡

- **识别**:仅当无 binding 且默认固定路径存在时触发,每次启动、绑定向导前只读执行。有 `skill-man.sqlite3` → Legacy Home,先执行只读 fixture 检测与 ADR-0010 恢复;mixed/unknown 保持 Fixture Recovery Lock,绑定向导不出现位置选择(不得另选位置绕过)。目录存在但无 SQLite 且无 fixture 指纹 → 不是 Legacy;非空未知内容使 Default 校验失败,只能 Choose… 其它位置。已有 binding 时 Legacy 路径完全忽略:不检测、不迁移、不展示。
- **过渡**:Default = Legacy 路径 → 原位绑定零搬移。自定义路径 → 先在外部 ledger 记录过渡(op id、live/目标路径、manifest、阶段)→ 源 SQLite checkpoint/WAL 收口后树拷贝(SQLite+WAL+SHM 一致集)→ 校验(逐文件大小 + tree hash + SQLite integrity_check + home_id 写入)→ 原子提交 locator → 下次启动验证。失败或崩溃 → 删除不完整副本(app 自建)或按 ledger 重试;Legacy 原样保留至提交成功;提交成功后 Legacy 路径 inert,App 永不自动删除。

## 5. 不可用与身份不匹配 fail-closed

| 状态 | 判定 | 行为 |
|---|---|---|
| HomeUnavailable | binding 存在但路径/卷不可达、权限拒绝、目录消失 | 不写不绑不静默改绑;SQLite 可只读打开则只读 Catalog,卷离线则无 Catalog;产品写全部拒绝(`home_unavailable`);locale 仍可读写;四个 Preferences 开关读默认值、写被拒;恢复界面提供 Reconnect / Restore / Abandon |
| HomeIdentityMismatch | 路径可达但卷身份 / marker / SQLite home_id 与 binding 不一致 | 现场绝不视为 Bound Home、不采纳现场内容;fail-closed 只读;绑定不自动解除;Reconnect 仅当重校验一致才成功;唯一出路 Abandon |
| AppStateUnavailable | 状态目录不可读、locator 解析失败或历史矛盾(binding 与 abandoned 冲突) | 既不当作 Unconfigured 也不当作 Bound;禁止产品写与新建绑定;只提供重试与 diagnostic 导出 |
| 空间不足 | 写前预留检查失败 | 拒绝(`insufficient_space`),不算 HomeUnavailable |

四个 Preferences 开关仍存 SQLite 内,HomeUnavailable 期间不落盘;locale 是唯一必须可读写的 App-level 状态(ADR-0011)。

## 6. Reconnect / Restore / Abandon

- **Reconnect Same Home**:资格 = HomeUnavailable / HomeIdentityMismatch,用户显式发起。重跑三方校验(卷 → marker → SQLite home_id + integrity);成功则重新打开 SQLite、recover operations、放行写;失败保持不可用,仅记 diagnostic,无其它副作用。
- **Restore Bound Home**:资格 = binding 有效、身份校验通过但内容验证失败(SQLite integrity/foreign-key 失败、schema 或 identity 不一致、Bound Home 检出 fixture 污染、未完成恢复)。完整执行 ADR-0010 状态机:当前内容同卷原子隔离为 Safety Snapshot,准备同 home_id 的干净 Home,验证后原子 promote,提交后保持写锁至用户确认。不触 Activations,不改 locator 中 home_id。
- **Abandon Home and Start New**:资格 = 任意已绑定 Home 状态(Bound / Unavailable / Mismatch / Lock);Unconfigured 不需要,Legacy Lock 期间不提供(否则构成绕过)。高摩擦:输入 home_id 或固定短语 + 二次确认 + 副作用警告。副作用边界:locator 永久记录旧 home_id 为 abandoned(旧卷回归时显示「已 Abandon,不重新绑定」,不自动绑定、不作为候选);不删除旧 Home 任何内容;不清理旧 Activation(指向旧 Home 实体的 Activation 变 Broken,只报告不修复);locale 不变;产生新 home_id,回到全新首次绑定流程。

## 7. cache 与原子性边界

随 Legacy 过渡与 Restore 原子处理:SQLite+WAL+SHM 一致集、`skills/`、`remotes/`、`operations/`、marker 与 manifest。`cache/`、`staging/` 可重建,不要求原子;恢复后的干净 Home 不复制它们,Safety Snapshot 仍整体包含以便可逆。Safety Snapshot = 与 Home 同卷的兄弟目录 `<Home>.snapshot-<op-id>/`,同卷 rename 原子隔离,永不自动删除,至少一次成功启动 + 显式确认后才可删除;记录于外部 ledger。

## 取代与衔接

- 本 ADR 取代 ADR-0007 的:Library 固定使用 `~/Library/Application Support/skill-man`(仅保留「该路径为默认位置」这一事实)、首次启动不询问路径、MVP 不提供 Library 路径设置或迁移能力;以及三步向导中「欢迎并创建默认 Library」的隐式创建语义(改为显式确认绑定)。
- 与 ADR-0010 衔接:Home 内部 layout、bootstrap locator、identity/bookmark、Legacy 一次性过渡与不可用语义在此定案;具体 tuple/hash/cursor、故障注入与验收矩阵拆入「汇总:vNext 真实数据可信、可配置与中文体验 spec」的实施票。
- 与 ADR-0011 衔接:locale 持久值物理上位于状态目录,独立于 Home 与绑定;HomeUnavailable 期间可读写,不受 Home write gate 影响。
- 绑定后不存在 Preferences 改址、Relocate 或普通 re-home;Reconnect / Restore 同一身份与显式 Abandon 是仅有的身份相关动作。

## Consequences

首次启动要求用户显式确认路径,牺牲一步到位的自动体验,换来不可变身份与 fail-closed 可靠性;Legacy 用户接受默认值时零搬移,自定义位置走唯一一次 copy 过渡;Home 外的状态目录是绑定与恢复的唯一 truth,App 自身状态损坏时宁可不工作也不猜测。汇总 spec 负责把状态机、目录契约与验收矩阵拆成实施票。
