# Wayfinder 会话简报:决策:统一 Skill Man Home 的内部边界、首次绑定与不可用语义(#35)

新 omp 会话的开机简报。你在 `/Users/zoe/Codes/AI/skill-man`(GitHub 仓库 RookieZoe/skill-man)。
任务:按 /wayfinder 的 "Work through the map" 推进地图 #32 的前沿票 #35,一次只解决这一张票;
产出决策,不产出产品代码;全部完成后删除本简报文件。上一张票 #37 已在 Codex 中按同样流程关闭
(resolution: https://github.com/RookieZoe/skill-man/issues/37#issuecomment-5256612340),作为格式范本。

## 0. 状态快照(开工前必须用 gh 复核,不要盲信本节)

地图:#32,子票 8 张,已关 4 张。
- 已关闭:#33 调研 .skill-lock.json、#34 Fixture Recovery、#36 窗口布局原型、#37 locale 契约。
- 开放:#35(本票)、#38 汇总 spec(blocked_by 含 #35、#39)、#39 Adopt Preview 原型(blocked_by #40)、#40 Adopt 来源分类(blocked_by #35)。
- 因此 #35 是当前唯一前沿票;关闭它解锁 #40。

## 1. 必读主源(按顺序)

1. `gh issue view 32 --comments` — 地图:Destination / Decisions so far / Out of scope / Safety note。
2. `gh issue view 35` — 本票全文:Question / Fixed direction / Decision required(8 块)/ Acceptance boundary。
3. `gh issue view 34 --comments` — Fixture Recovery 决议全文(#35 的直接上游;含恢复状态机顺序与 Home Binding 不变量)。
4. `docs/adr/0007-first-run-and-settings.md` — 现行首启向导与「Library 固定 ~/Library/Application Support/skill-man、不询问路径」;本票决议将取代其中 Home 相关表述,冲突处要显式标注取代。
5. `docs/adr/0010-production-fixture-recovery.md` — locator / recovery ledger 必须 Home 外;Legacy 先恢复后绑定。
6. `docs/adr/0011-interface-locale-and-message-ownership.md` — locale authority 独立于 Home,在 Unconfigured / HomeUnavailable / Catalog ReadOnly / Fixture Recovery Lock 期间仍可读写。
7. `CONTEXT.md` — 词汇表;输出必须使用其中规范词。
8. 现状代码事实:`src-tauri/src/lib.rs:68-74` 生产 composition root 硬编码 `~/Library/Application Support/skill-man` 并在 onboarding UI 前打开 SQLite;`src-tauri/src/core/startup.rs` 首启判定(`first_run_completed_at`)+ 三步向导;Preferences 四开关在 `src-tauri/src/core/preferences.rs`。
9. `docs/agents/issue-tracker.md` + `.github/workflows/ci.yml` — tracker 操作命令与 CI 门禁。

## 2. 复核前沿后认领

先复核(防并发会话已认领):
gh api repos/RookieZoe/skill-man/issues/35 --jq '{assignees:[.assignees[].login], blocked_by_open:.issue_dependencies_summary.blocked_by}'
gh issue view 35 --comments
若已有 assignee 或 resolution 评论:停止,报告用户,不要重复认领。
否则第一步写操作必须是认领:
gh issue edit 35 --add-assignee @me

## 3. Grilling(调用 /grilling 与 /domain-modeling 技能)

事实自己查(读码、读 ADR、必要时派 research 子代理),决策全部问用户。
按轮:每轮列出当前可问的全部前沿问题,每题编号并给推荐答案(格式 ❓ **QN** / ➡️ 建议),等用户回答后再算下一轮。
用户可整轮回答「全部按建议」。

问题面必须覆盖 #35 body 的 8 块 Decision required,不得遗漏:
1. 稳定 Home layout:SQLite、托管 Skill 实体、remote source 父目录、cache、staging、operations、recovery journal 各放哪;哪些可重建。
2. 最小 bootstrap locator(Home 外)的物理位置与格式;外部 binding、Home marker、Catalog 身份(home_id)如何共同证明同一个 Home。
3. 首次绑定状态机 Unconfigured → Home Candidate → Bound Home:默认/自定义路径的创建、权限、空间、路径重叠、symlink、已有内容、取消、崩溃恢复。
4. Legacy Home 的识别时机与只读检测;Fixture Recovery 必须先于一次性绑定过渡,mixed/unknown 不得绕过。
5. 旧固定路径 → 首次选定位置的唯一一次绑定前过渡:copy 还是 rename、验证、回滚、locator 原子提交(tmp→fsync→rename→parent fsync 语义,对齐 #34 ledger)。
6. HomeUnavailable / HomeIdentityMismatch 的 fail-closed 行为:外置卷离线/重挂载、权限丢失、路径或 symlink 被替换、marker/卷身份不匹配、空间不足。
7. Reconnect Same Home、Restore Bound Home、Abandon Home and Start New 的资格、确认与副作用边界。
8. cache 可重建性;哪些内容必须随 Legacy 过渡或 Restore 原子处理;Safety Snapshot / recovery ledger 如何独立于活动 Home 存放。

已定案约束(来自地图、#34、ADR-0010/0011;作为前提陈述,不重新问):
- Home 外必须存放:最小 Home Binding locator、durable recovery ledger、Safety Snapshot、locale 选择。
- Fixture Recovery 先于一次性绑定过渡;mixed/unknown 不得通过另选位置绕过;Safety Snapshot 在旧 App 完全退出后包含 SQLite、WAL、SHM 与完整旧 Home。
- 绑定逻辑 home_id 而非路径字符串;绑定后无 Preferences 改址 / Relocate / 普通 re-home;Reconnect/Restore 只接受同一 home_id;Abandon Home and Start New 是产生新身份的唯一逃生口,不删旧 Home、不自动清理旧 Activation。
- Agent 的 skills 目录不是 Home;属各 Agent Preset / Activation 边界。
- locale 选择在 Unconfigured、HomeUnavailable、Catalog ReadOnly、Fixture Recovery Lock 期间仍可读写,Home layout、Catalog SQLite 与 Home Binding 不得吸收或锁住它。
- 不实现产品代码;不执行本机数据清理;继续遵守地图 Safety note(恢复/绑定实施前不手工 Adopt/Remove/删 fixture)。

## 4. 落盘(用户确认全部答案后)

最终 resolution 必须包含:目录契约表(含每目录可重建性)、bootstrap locator 与 identity/marker 契约、首次绑定状态机、Legacy 一次性过渡、HomeUnavailable/HomeIdentityMismatch 状态机与验收矩阵、Reconnect/Restore/Abandon 资格与副作用;并明确绑定后不存在改址或普通迁移。对照 #35 Acceptance boundary 逐条检查。

1. 新规范词写入 `CONTEXT.md`(沿用现有格式:术语、中英对照显示词、Avoid;新词加入 Localized display terms 表与 Domain terms)。
2. 新建 `docs/adr/0012-skill-man-home-binding-and-unavailability.md`(编号 0012 已确认为下一个空号;格式按 /domain-modeling 的 ADR-FORMAT)。开头注明它取代 ADR-0007 的哪些表述(固定 Library 路径、不询问路径、MVP 无迁移);与 ADR-0010 的衔接处指向本 ADR。
3. 提交前跑 CI 等价门禁并全绿:
   npm run format:check; cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
   npm run lint; npm run typecheck
   cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
   (纯文档改动,npm test / cargo test 预期不受影响;如被改动波及则一并跑。)
4. 直接在 main 提交并推送(沿用 #37 模式;仓库惯例为直推 main):git add -A; git commit; git push origin main。

## 5. 关闭与地图更新

1. `gh issue comment 35 --body "<完整 resolution>"`(结构对标 #37 resolution)。
2. `gh issue close 35`。
3. 在 #32 body 的 "Decisions so far" 追加一行指针:gist + 链接(编辑 issue body,不重述细节)。
4. 若 resolution 让 fog 可精确描述出新票:create-then-wire(`gh issue create` + sub_issues + `gh api --method POST repos/RookieZoe/skill-man/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`,blocker-db-id 用 `.id` 不是 number);否则更新 "Not yet specified" 为「当前可见问题均已形成子票」。
5. 删除本简报文件,随最后一次提交一起推送。

## Done 判定

- #35 关闭,带完整 resolution 评论;Acceptance boundary 全部满足(领域术语、目录契约、首次绑定状态机、Legacy 一次性过渡、不可用/重连/恢复/Abandon 状态机与验收矩阵;无绑定后改址或普通迁移)。
- CONTEXT.md 与新 ADR-0012 已提交,main 已推送,CI run 绿。
- #32 Decisions so far 已含 #35 指针;本简报已删除。
- 未触碰产品代码;未清理本机数据。

## 后续(不在本会话)

#40 → #39 → #38 依次用同一流程(每张票一个新会话,一次一张)。#38 关闭、地图清空后,
按 ask-matt 路线离开 wayfinder:/to-spec 汇总成可实施 spec → /to-tickets → /implement。
