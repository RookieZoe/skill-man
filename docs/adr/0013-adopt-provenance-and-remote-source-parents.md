# Adopt 的 lock provenance、Remote Source Parent 与所有权交接

`.skill-lock.json` 是外部 installer 可编辑且可能陈旧的 provenance hint，不是实体所有权或内容完整性证明。Adopt 只有在 lock、外部 canonical 实体、remote/ref、Verification Anchor、skillPath 与 tree 形成闭环后，才把候选认领为 Remote Install；其余候选保持用户所有，以 Local Link 认领。Remote Source Parent 只持久化 repository 身份，Skill 实体与版本关系分别留在 `<Home>/skills/` 和 per-Skill Remote Binding，避免 checkout、cache、Library 与外部 installer 同时拥有内容。

本 ADR 细化 ADR-0004 的 remote 来源元数据与更新规则，并取代 ADR-0005 对 `.skill-lock.json` 管理候选、installer/shared root 内候选的通用 Migrate 判断；其它 Adopt 扫描、Conflict、Activation、journal 与批量隔离规则继续有效。Home 布局沿用 ADR-0012：`remotes/`、`skills/` 同级，`cache/` 可重建。

## 1. 领域模型

- **Verified Remote Source**：lock 线索、唯一外部 owner、canonical 实体、审核过的 source adapter、remote/ref、Verification Anchor、skillPath、provider hash 与本地/remote tree 均通过验证的来源。
- **Verification Anchor**：requested ref 可达历史中、其 `skillPath` tree 与 lock 和本地候选形成闭环的确定 commit。它证明该 commit 下的 Skill 内容，不冒充外部 installer 未记录的原始安装 commit。
- **Remote Source Parent**：一个 remote repository 的稳定聚合，身份为内部 UUID `remote_id`；URL、ref 或 commit 不充当身份。
- **Remote Binding**：每个 Remote Install 独立记录 requested ref、Verification Anchor、skillPath、remote baseline 与当前内容 baseline。Parent 没有“当前 commit”。
- **Local Source**：没有成为 Verified Remote Source、继续由用户拥有的最终实体；Adopt 只登记 Link，不把它伪装成没有 provenance 的 File Install。
- **Provenance Conflict**：lock 声明存在，但 schema、owner、name、location、source、ref、path、hash、commit 或实体证据缺失/矛盾。
- **Verification Deferred**：已知证据未矛盾，但 DNS/TLS/timeout/rate limit/401/403/404 等可用性事实不足，暂时不能完成 remote 闭环。
- **Ownership Handoff**：Verified Remote Source 从外部 installer 显式、可恢复地转交给 Skill Man 的每 Skill 事务。
- **Ownership Conflict**：交接后外部 installer 再次创建同名 lock 声明或 canonical 实体；它不属于 Update 或 Activation Conflict。
- **Remote Source Identity Conflict**：`source.json` 与 Catalog parent row 缺失或不一致，不能证明同一 `remote_id` 与 canonical URL。

## 2. Lock discovery 与可信闭环

### 2.1 发现和失败域

只读枚举上游已知的默认 lock 路径与当前 XDG 规范路径，并在 Preview 中显示命中位置。一个 Skill 必须只有一个外部 lock owner：零条声明走 No Lock；恰好一条有效声明才可继续 remote 验证；两个位置都声明同名 Skill，即使内容相同也进入 Provenance Conflict，不自动决定后续哪个文件是 authority。

文件级 UTF-8、JSON、version 或 duplicate-key 错误阻止该 lock 所治理 installer root 内的全部候选，并禁止改写该文件；结构有效后，单条 entry 的缺项或矛盾只阻止该 entry。位于其它、不受该 lock 治理位置且可证明没有声明的候选仍按 No Lock 处理。未知未来 version 不按 v3 猜测。

### 2.2 Verified Remote Source 的硬门槛

全部条件同时成立才提升：

1. lock 严格为已审核的 v3 schema；对象 key 是安全目录名。
2. lock key、外部 installer canonical 子目录名、Adopt candidate identity 与 materialized remote Skill 目录身份一致。frontmatter `name`、`pluginName` 与内容 hash 不参与身份；不同名称 appearances 不自动合并或重命名。
3. 最终实体是该 installer canonical 路径下的真实目录，而不是仅仅有同名 Agent entry、fallback copy 或指向任意位置的软链。
4. `sourceType` 有 Skill Man 审核过的验证 adapter。GitHub、GitLab、generic HTTPS Git 分别验证自己的 URL/ref/hash 语义；未知、download、local、well-known 或算法不明类型不伪装成 Git。
5. `source`、`sourceUrl` 与 sourceType 经 provider parser 后指向同一 repository；embedded credentials、SSH、非法 ref/path 或互相矛盾均拒绝。credentials 不持久化到 manifest、Catalog、journal 或日志。
6. lock 缺失 ref 时记录 `HEAD`，表示持续跟踪 remote default branch；存在 ref 时保留原值。Pinned tag/commit 必须在精确 commit 匹配，不能搜索替代版本。
7. `skillPath` 安全、精确、指向含可读 `SKILL.md` 的目录；路径消失或名称不同不按 name/hash 猜测改名。
8. provider-specific `skillFolderHash` 在 Verification Anchor 上匹配。然后 materialize `anchor + skillPath`，对 remote tree 与本地最终实体分别计算 Skill Man `tree-sha256-v1`；任何读取错误使整棵 tree 失败，不接受部分结果。
9. Apply 重新检查 lock fingerprint、entry、路径/inode、tree 与 appearances，Preview 证据不能跨外部变化继续使用。

GitHub 的 subtree Git tree SHA 在合集仓库中会跨多个无关 commits 保持相同，因此不能要求“唯一历史 commit”。Moving ref 的 tip 若 subtree 匹配，tip 即 Verification Anchor；否则在该 requested-ref ancestry 中使用最新匹配 commit作为 deterministic anchor，并记录“original install commit unknown”。这不声称还原了外部 installer 当时未记录的 commit，只证明 anchor 下 Skill tree 完全相同。Pinned ref 仍必须精确匹配。

### 2.3 可用性与矛盾

按 normalized remote 共享一次 fetch；同 remote 的候选共同复用 objects，但逐 Skill 验证 ref、anchor、skillPath 与 tree。DNS、TLS、timeout、rate limit、401、403 与 404 无法可靠区分离线、私有、删除或暂时服务故障，统一 Verification Deferred；其它 remote 组继续。fetch 成功后 ref/path/hash/entity 不匹配才是 Provenance Conflict。二者都保持 Untracked，不自动降级；用户可 Retry，或在看过证据后显式忽略 lock 并选择 Local Link。

## 3. 分类决策

| 证据 | Preview / 可选动作 | Apply 后模型 |
|---|---|---|
| 没有 lock 声明 | Local Link | 实体保持或迁到用户选择的稳定目录；无 Remote Parent/Binding |
| lock 文件级损坏、重复 owner、entry/identity/source/path/hash 矛盾 | Provenance Conflict；默认不可 Apply；可修复后 Retry。仅在文件可安全定位并 CAS 删除该 entry 时，用户才可显式忽略并转 Local Link | 未处理时保持 Untracked；转 Link 后外部 lock owner 退出 |
| remote 暂不可验证 | Verification Deferred；Retry 或显式忽略后转 Local Link | 未处理时保持 Untracked；不因网络故障自动固化来源分类 |
| remote 闭环成立，local tree = remote tree | Remote Install | Home entity 逐字等于当前可见内容；Remote Binding baseline = anchor tree；Healthy |
| remote 闭环成立，local tree ≠ remote tree | 三路显式选择；默认保留当前字节 | 见下文 Modified 分支 |
| remote 或选定 Home-owned tree 违反 Install 安全规则 | Remote Install 路径禁用；保持 Untracked 或显式 Local Link | 不把绝对/越界/dangling symlink、特殊文件等复制进 Home |
| 同一实体以不同目录名出现 | Identity Conflict；先改名/移除再 Rescan | 不按 frontmatter、remote path 或 hash 自动统一 |

Verified-but-Modified 的三个动作严格分开：

1. **保留当前字节**：把当前 tree 原样复制进 `<Home>/skills/<name>`；Remote Binding 记录 Verification Anchor 的 remote baseline，Skill 另记当前 local hash，立即进入 Modified。只有 local 修改通过 Home-owned Install 安全校验时可选。
2. **放弃修改并重装**：显式破坏性确认后 materialize Verification Anchor，不跳到当前 ref tip；当前字节留在短期 Undo 隔离副本中。
3. **转 Local Link**：保留当前字节，不创建 Remote Binding；若实体在 installer/shared/Agent skills root 内，用户必须选择这些 root、Skill Man Home 与 App state 之外的稳定目录，再执行 journaled move。外部 lock entry 以 CAS 删除，所有权转给用户。

如果 remote baseline 本身不符合 Install 安全规则，只能保持 Untracked 或转 Local Link；如果只有 local 修改不安全，可按安全 anchor 重装或转 Local Link，不能借 Adopt 绕过 ADR-0004。

## 4. Remote Source Parent 与 Remote Binding

### 4.1 Durable layout

```text
<Home>/
├── skills/<skill-name>/
├── remotes/<remote-id>/source.json
└── cache/git/<url-hash>.git
```

`remote_id` 是 UUID。`source.json` 仅保存 `schema_version`、`remote_id`、canonical HTTPS clone URL、用户确认过的 URL aliases 与 `created_at`，并以 temp → fsync → rename → parent fsync 提交。它不保存 checkout/worktree、Git objects、凭据、整份外部 lock 或 per-Skill binding。Catalog 保存 parent row 与每 Skill Remote Binding；operation audit 只保存完成恢复/审计所需的 entry fingerprint 和证据摘要，不复制无关 lock 内容。

`skills/` 是 Home-owned Install 实体的唯一内容位置；`remotes/` 不含第二份实体；bare mirror 只在可重建 `cache/`。同 remote 的多个 Skills、refs 与 commits 共用 parent/fetch，但 Preview、baseline、Modified、Update 与提交都逐 Skill独立；同仓 Skills 暂时位于不同 commits 是合法状态。

### 4.2 URL identity、alias 与碰撞

identity normalization 在 provider parser 已拆掉 ref/subpath 后执行：scheme/host 小写，去默认端口、末尾 `/` 与 `.git`；path case 保留；query/fragment 不进入 repository identity；embedded credentials 直接拒绝。fetch 的最终 HTTPS URL、仓库 rename 或 redirect 不静默改写 identity：Preview 显示变化，用户确认后给同一 remote_id 增加 alias/更新 canonical URL。

新 Adopt 命中已有 canonical URL/alias 时复用该 parent。两个既有 parents 后来汇聚到同一 repository 时，Update 与 alias 变更 fail closed；用户显式选择 survivor，逐 Skill 重验证并改绑，确认无 Skill identity 冲突后删除空 loser，审计保留旧 remote_id。不得自动选择“最早”或按 URL hash 合并。

最后一个 child Remote Binding 被 Remove 后，删除 parent row 与 `<Home>/remotes/<remote-id>/`；审计留在 operation history，cache 独立清理。未来重新安装同 remote 创建新 remote_id。上游 `skillPath` 消失不改变 parent；保持当前 Skill，进入 Upstream Path Gone，用户显式重选路径、pin 或 Remove，不按名称/hash跟随。

### 4.3 Parent integrity

`source.json` 与 Catalog parent row 必须对 `remote_id`、canonical URL/aliases 一致。缺失或不一致进入 Remote Source Identity Conflict：子 Skill 与 Activation 可读，可 Disable/Remove；该 parent 的 Update、新 Remote Binding 与 alias 变更被阻止；其它 parents 和 Local Sources 继续工作。恢复必须以 remote fetch + 全部 child bindings 重验证，不能静默任选 SQLite 或 manifest 覆盖另一份。

## 5. Ownership Handoff 状态机

Ownership Handoff 每 Skill 独立，批量只共享 discovery/fetch/Preview。状态机为：

```text
Planned → Staged → Source Isolated → External Ownership Released
                                      (CAS commit point)
        → Managed Committed → Finalized
```

1. **Planned**：冻结 full lock fingerprint、exact entry、canonical path/inode/tree、全部 appearances、用户选择、remote 证据与目标 parent；写 durable Home operation journal。
2. **Staged**：逐字 stage 当前 tree 或明确选择的 Verification Anchor tree；计算 hash、执行完整 Install/Link 安全校验与空间检查。Catalog/Activation 尚未声称成功。
3. **Source Isolated**：把外部 canonical 真实目录 rename 到同父隐藏 operation 路径，原子冻结外部 source；重验 inode/tree 与 appearances。CAS 前任何失败都把目录原位恢复，lock 与 Catalog 不变。
4. **External Ownership Released — commit point**：严格解析原 lock，并只在整文件 fingerprint 与 exact entry 仍匹配时删除 `skills[identity]`。改写保留 version、其它 top-level 值、其它 entries 与未知 JSON 字段；规范化重序列化为合法 v3，最后一个 entry 删除后仍保留空 lock；temp → fsync → rename → parent fsync。fingerprint 变化则不写、恢复 source、停止该 Skill，并停止同批尚未提交项。
5. **Managed Committed**：CAS 成功后只允许 roll-forward：publish `<Home>/skills/<name>`、原子写 Catalog parent/binding/Skill/Activation desired state，把已发现的真实 Agent 私有 appearances 改为直指 Home entity 的 Activations。`~/.agents/skills` 等 installer/shared canonical 路径保持不存在，永不留下受管 Activation；否则外部 CLI 可再次沿链修改 Home，并破坏独立 Enable/Disable。
6. **Finalized**：验证 Catalog、Home entity、private Activations 与外部 canonical 缺失；保留短期 Undo 隔离副本，结果窗口关闭或重启后才清理。

CAS 前崩溃按 journal rollback；CAS 后崩溃按 journal roll-forward，完成前进入 recovery write lock，不允许其它常规写。跨 Home、外部目录、SQLite 与 lock 不假装存在物理单事务；CAS lock 退出是唯一逻辑 commit point，使恢复方向确定。

No Lock 且已在稳定外部位置的 Local Link 不需要 Ownership Handoff；按既有 Adopt journal 登记 Link 并压平 private Activations。No Lock 但实体位于 Agent/shared/installer root 时，用户先选稳定目录，journaled move 成功后登记 Link；取消则保持 Untracked。

### 5.1 Undo 与日后 Remove

结果窗口关闭/重启前保留有条件 Undo。只有 Home entity/appearances 未变化、外部 canonical 位置可恢复、目标 Link 位置未被新工作占用、lock 仍严格有效且该 key 未被占用时，才以 CAS 恢复旧 entry、原实体与 appearances；任何 guard 失败都拒绝 Undo并保持已提交状态，不覆盖交接后的外部工作。

普通 Remove 不恢复旧外部 owner：它按 Install 语义删除 Home entity、Activations 与 Remote Binding，最后一个 child 时删除 parent。只有上述短期 Undo 能恢复旧 lock/entity；日后若用户要回到外部 skills CLI，由用户重新执行外部 add。

## 6. Update 与再次出现的外部 owner

Adopt 与 Update 分离。Verification Anchor 老于当前 tracked ref 且 upstream Skill tree 已变化时，Preview 显示“接管后已有更新”，但 Ownership Handoff 只接管已验证版本；即使选择放弃 Modified，也重装 anchor，不顺带跳到 HEAD。提交后继续使用 ADR-0004 的 Update 流程：同 parent 一次 fetch，逐 Skill Preview/Apply；Modified 禁止静默覆盖；path gone 显式重选/pin/Remove。

交接后若外部 installer 又创建同名 lock entry 或 canonical entity，Skill Man 保留现有 Managed Skill 与 Home 字节，标记 Ownership Conflict，不自动删 lock、合并、覆盖或重新接管。外部实体与 Managed Skill 是不同 owner 的同名实体，即使 hash 相同也不自动合并；用户必须选择删除外部副本，或先 Remove Skill Man 版本再重新 Adopt。普通扫描不得把这种状态误报为 Update。

## 7. 迁移验收矩阵

| Scenario | Required evidence |
|---|---|
| No Lock、稳定外部实体 | 直接 Link；实体 inode/tree 不变；无 Home copy、Remote Parent/Binding 或 lock write |
| No Lock、实体在 Agent/shared/installer root | 未选稳定目录时不可 Apply；选择后 tree 逐字一致，旧 shared path 消失，只有真实 Agent 私有 Activations |
| 单一有效 lock、clean tree | provider hash、anchor remote tree、本地 tree 闭环；Home tree 等于 Apply 前本地 tree；entry 被 CAS 删除；external canonical 缺失；Healthy Remote Binding |
| verified + Modified，保留 | Home tree 等于 Apply 前本地 tree；remote/local 双 baseline 不同；状态立即 Modified |
| verified + Modified，放弃 | Home tree 等于 Verification Anchor，不等于 ref tip 的意外新版本；旧 tree 仅在短期 Undo 隔离副本；有显式破坏性确认 |
| verified + Modified，Link | 用户稳定目录 tree 等于 Apply 前本地 tree；lock entry CAS 删除；无 Remote Binding；外部 canonical 缺失 |
| unsafe remote/local tree | 不安全的 Home-owned 选项禁用；不删除/改写不安全条目；Local Link 路径仍需显式确认 |
| duplicate lock owner / corrupt file / identity、source、path、hash mismatch | 零 filesystem/Catalog/lock 变更；具体 Provenance Conflict 证据可见 |
| DNS/TLS/timeout/rate-limit/401/403/404 | 该 remote 组 Verification Deferred；其它 remote 组继续；无自动 Link 或写入 |
| same remote 多 Skill/多 ref | 单 parent、单 fetch；每 Skill 独立 anchor/baseline/commit；一个失败不回滚已成功其它 Skill |
| lock 并发变化 | CAS 失败；隔离 source 原位恢复；Catalog 无 owner；剩余未提交项停止 |
| crash before / after CAS | CAS 前完整 rollback；CAS 后 recovery lock 下完整 roll-forward；不存在长期双 owner 或无人 owner |
| URL redirect / parent collision | 未确认不改 alias；新来源复用现有 parent；既有 parents 只经显式 reverify/merge 汇聚 |
| parent manifest/row mismatch | 仅该 parent 的 update/new binding/alias fail closed；read/Disable/Remove 与其它来源可用 |
| conditional Undo | guards 全满足才恢复 exact entry/entity/appearances；任一新占用或 lock 变化时不覆盖并明确拒绝 |
| external installer reappears | Managed Home entity 不变；显示 Ownership Conflict；不误作 Update、不自动接管 |
| last child Remove | Home Skill/Binding 删除；空 parent manifest/row 删除；旧 external lock 不恢复 |

## Consequences

该方案牺牲“看到同名 lock 就一键收编”的成功率，换取内容保真、单一 owner、可解释 provenance 与确定崩溃恢复。它故意不在 `remotes/` 保存 checkout/mirror，不把 temporary network failure 当 Local Source，不在 Adopt 中顺带 Update，也不让 shared/installer root 继续充当 Activation。实现需要新增 parent/binding 与 handoff journal 状态、strict lock parser/provider adapters、CAS lock writer、source-level recovery gate 与上述故障注入矩阵；汇总 vNext spec 负责把这些契约拆成实施票。