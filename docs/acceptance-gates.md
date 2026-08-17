# vNext 人工验收 Gate 本地测试清单（issue #49）

> 对应票：[验收:vNext 本机恢复、真实 Adopt、窗口与双语 Gate #49](https://github.com/RookieZoe/skill-man/issues/49)
> 权威依据：[vnext-implementation-spec.md §10.3–§10.4](vnext-implementation-spec.md)
> 本清单是逐项可勾选的执行手册；结果按 §10.3 记录到 issue #49 评论，**不得**粘贴 Skill 正文、token 或凭据。

## 0. 前置条件

- [ ] 候选 build commit 已固定：`e34844f`（`fix: green the full suite for the #49 human gates`）
- [ ] CI 全绿：run [31819015110](https://github.com/RookieZoe/skill-man/actions/runs/31819015110)（全步骤通过，含 Tauri no-bundle 构建）
- [ ] 旧 Skill Man 完全退出且无 SQLite writer：

```bash
pgrep -fl skill-man          # 期望空
lsof "$HOME/Library/Application Support/skill-man/skill-man.sqlite3"   # 期望无输出
```

- [ ] 本次验收只验证未签名/开发构建行为；签名、公证、Gatekeeper、真实 updater 升级由 #31/#17 跟踪，不在本 Gate（§10.4）

### 0.1 本机已知起点（2026-08-17 采集，只读）

真实 Home `~/Library/Application Support/skill-man/` 为**生产 fixture 足迹**，与 `FixtureFingerprintV1` 常量字节级一致：

| 证据 | 值 |
|---|---|
| schema_version | `4`（= `FIXTURE_CATALOG_SCHEMA_VERSION`） |
| 行数 | skills 3 / agents 3 / activations 0 / file_sources 0 / remote_sources 0 |
| skills | skill-authoring、media-xray、legacy-audit |
| agents | claude-code、codex、workbench |
| `fixture-entities/` | 仅 `skill-authoring/SKILL.md` 与 `media-xray/SKILL.md`（legacy-audit 悬空，seed 从未物化） |
| integrity / FK | `ok` / 无违规 |

预期分类：**Pure fixture → Fixture Recovery Lock**（RecoveryView）。若判定不同，先停，把分类证据贴回 issue 再继续。

## 1. 构建候选 app

```bash
cd /Users/zoe/Codes/AI/skill-man
git checkout e34844f          # 或确认 HEAD 已是它
npm ci
npm run tauri build -- --target aarch64-apple-darwin
# 产物：src-tauri/target/release/bundle/macos/Skill Man.app
```

- [ ] 构建成功，记录产物 sha256：`shasum -a 256 "src-tauri/target/release/bundle/macos/Skill Man.app/Contents/MacOS/skill-man"`
- [ ] 首次启动被 Gatekeeper 拦时：右键 → 打开（未签名/临时签名构建，§10.4 允许）
- [ ] 需要 DevTools（查 `document.lang` 等）时改用开发构建：`npm run tauri dev`（右键 → Inspect / ⌘⌥I）

## 2. 记录模板（每 Gate 一条，贴入 issue #49）

```
Gate X — <名称>
- 日期：YYYY-MM-DD
- 操作者：<姓名>
- build commit：e34844f
- 输入摘要：<Home 分类 / 候选技能与 lock / 窗口尺寸矩阵 / 系统语言序列>
- 结果：PASS / FAIL（FAIL 附错误面与复现）
- 资产链接：<截图/命令输出/gist 链接>
```

## 3. Gate A — 本机数据恢复

### 3.1 确认无 writer（启动前必做）

- [ ] `pgrep -fl skill-man` 为空
- [ ] `lsof "$HOME/Library/Application Support/skill-man/skill-man.sqlite3"` 无输出

### 3.2 只读证据（写操作前全部采集）

```bash
DB="$HOME/Library/Application Support/skill-man/skill-man.sqlite3"
sqlite3 "$DB" 'PRAGMA integrity_check;'                    # → ok
sqlite3 "$DB" 'PRAGMA foreign_key_check;'                  # → 空
sqlite3 "$DB" 'SELECT schema_version, snapshot_version FROM catalog_meta;'
# 注：`home_id` 列只在 v5+（Bound）schema 存在；v4 Legacy 库查询会报
# “no such column: home_id”——这是未绑定 Legacy 的预期证据，并非错误。
# 绑定/恢复完成后（v5+）再查 home_id 确认身份。
sqlite3 "$DB" "SELECT 'skills',count(*) FROM skills UNION ALL SELECT 'agents',count(*) FROM agents UNION ALL SELECT 'activations',count(*) FROM activations UNION ALL SELECT 'file_sources',count(*) FROM file_sources UNION ALL SELECT 'remote_sources',count(*) FROM remote_sources;"
```

- [ ] integrity `ok`、FK 无违规、row counts 已存档（已知起点见 §0.1——若与本机实际不符，以当次输出为准）

### 3.3 启动 app，记录 bootstrap 分类

- [ ] 截图首屏分类。按分类走对应分支：

| 分类 | 分支 |
|---|---|
| Fixture Recovery Lock | §3.4 恢复流 |
| LegacyDetected | §3.5 Legacy 过渡 |
| Bound | 正常进入 Library，记录 home_id |
| HomeUnavailable | Reconnect/Restore 流（§10.3 Gate A 的不可用分支） |
| Abandoned | 只读确认不可再绑定，走 §3.6 说明 |

### 3.4 Fixture Recovery Lock → 恢复流（本机预期路径）

- [ ] RecoveryView 展示 Pure 分类与完整证据（缺表/行/树不符逐项列出；本机应为空）
- [ ] 截图证据区
- [ ] **等用户确认后** 执行恢复（plan → apply）
- [ ] 记录 Safety Snapshot 路径：`~/Library/Application Support/skill-man.snapshot-<op-id>/`（同级目录）
- [ ] 记录 manifest hash（结果/快照列表展示）
- [ ] 恢复后重跑 §3.2 命令：integrity、FK、row counts 前后对比写入记录
- [ ] `list_safety_snapshots` 截图存证（SafetySnapshots 面板）
- [ ] 恢复完成后 bootstrap 转 LegacyDetected → 首次绑定流（§3.5）

### 3.5 Legacy 过渡（首次绑定）

- [ ] Legacy summary（含 path）截图
- [ ] Use Default 或 Choose → 确认页核对 `{path}` 与 explicit-confirm 文案
- [ ] 确认绑定后记录新 home_id 与绑定结果
- [ ] **Rescan 只报告真实 Untracked**；无自动 Adopt/Enable/Repair（勾选一项验证）
- [ ] 结果确认后才开放写；确认前不点任何写操作

### 3.6 纪律（全过程）

- [ ] 恢复前不手动删除 `fixture-entities/`、不跑 fixture 测试
- [ ] 不手动 Relocate/改址/普通 re-home；新建身份只能走 Abandon 流程
- [ ] locale 独立可用；Agent skills 目录不属于 Home

## 4. Gate B — 真实多跳 Adopt

### 4.1 候选选择（真实环境，各选一个）

- [ ] **Local Link 多跳/多 appearance**：`~/.agents/skills` 下经 symlink 链到达、或出现在多个 agent 目录的技能
- [ ] **lock-managed remote**：`~/.agents/.skill-lock.json` 中有声明的技能（建议 branch 或 tag，验证 anchor 规则）

### 4.2 账本核对（EvidenceLedger，逐项）

- [ ] 每一 hop：路径 + 类型（Symlink/Dir/…）与磁盘一致
- [ ] lock path：`~/.agents/.skill-lock.json`（或 XDG 变体）与账本一致
- [ ] Verification Anchor commit、requested ref disposition（head/branch/tag/commit）
- [ ] local/remote tree hash 与 `trees_match`
- [ ] 显式 Include 开关只在 selectable 候选上出现
- [ ] 最终 owner/verdict（Local/Verified/Modified/Conflict/Deferred/Blocked/Excluded）与预期一致

### 4.3 Apply 前后取证（正文/凭据不记录）

```bash
DB="$HOME/Library/Application Support/skill-man/skill-man.sqlite3"
# source 与 Home 副本字节一致（期望无输出 = 相同）：
cmp <source>/SKILL.md <home-copy>/SKILL.md
# Apply 前的 activation 记录：
sqlite3 "$DB" 'SELECT skill_id, agent_id, path FROM activations;'
# 对每个 activation path 记录 symlink target：
readlink "<path>"
```

- [ ] Apply 前：source/Home tree hash、activation symlink target 已记录
- [ ] Apply（勾选 Include）→ 记录 source/Home hash 与 symlink target 前后值
- [ ] 最终 owner 与预期一致（Local Link 注册 / remote 转 ownership）

## 5. Gate C — Tauri/WKWebView 视觉

### 5.1 窗口尺寸矩阵

```bash
# 需辅助功能权限；或用鼠标拖到目标尺寸
osascript -e 'tell application "System Events" to tell process "Skill Man" to set size of window 1 to {760, 520}'
```

- [ ] **760×520**（最小）截图
- [ ] **1059px 宽**截图
- [ ] **1060px 宽**截图
- [ ] **高窗口**（如 1180×1200）截图

每个尺寸下检查：

- [ ] 0/1/multiple Notice 三态
- [ ] empty 状态（无技能/无激活）与 error 状态
- [ ] dense English 与简体中文文案
- [ ] 无 page 横向溢出（底部无横向滚动条）

### 5.2 跨断点 overlay

- [ ] 打开 sheet/drawer：Preferences（工具栏齿轮）、Adopt 账本、Import
- [ ] 打开后跨断点 resize，逐个核对：
  - [ ] scroll owner：三栏/双栏/drawer 各自滚动、Toolbar 固定不滚
  - [ ] 无空白区、无横向溢出
  - [ ] focus trap：Tab 仅在 overlay 内循环
  - [ ] Escape 关闭
  - [ ] busy 态按钮禁用（如 Apply/Check 进行中）
  - [ ] 关闭后焦点回到触发器（如 Preferences 后焦点回齿轮按钮）
- [ ] resize 不 remount（内容状态保持，如已选 Include 不丢）

### 5.3 可访问性基础记录

- [ ] 每尺寸窗口截图存档（命名含尺寸）
- [ ] VoiceOver：⌘F5 开启 → Tab 走查 → 记录关键元素 role/label（至少标题、列表、按钮、dialog）

## 6. Gate D — 双语 native QA

### 6.1 系统语言与 override 序列

- [ ] 系统 English：系统设置 → 通用 → 语言与地区，重启 app，首帧英文截图
- [ ] 手动 override：LanguageControl（bootstrap/恢复页底部或 Library 工具栏）切简体中文 → 立即生效
- [ ] 切回 **System** → 跟随系统
- [ ] 系统切换为 简体中文：重启 app，首帧中文截图
- [ ] 重激活（焦点/重开窗口）与重启后 locale 持久

### 6.2 核对面（每个 locale）

- [ ] 首帧语言正确
- [ ] DevTools 查 `document.documentElement.lang`（`en` / `zh-Hans`）
- [ ] Preferences（齿轮 → 设置项文案）
- [ ] tray（菜单栏图标：最近技能/打开窗口/退出，文案随 locale 变）
- [ ] native menu（app 菜单文案、⌘Q 可用）
- [ ] ARIA labels / role（VoiceOver 复核）
- [ ] 日期/数字/单位格式化（如 `formatDateTime` 输出）
- [ ] 公开错误文案（制造一次可逆错误，如恢复/绑定失败路径）

### 6.3 byte equality 与 diagnostic 分区

- [ ] 同一 Source Content（如同一候选的 evidence 文本/chain 描述）在两种 locale 下各导出一份
- [ ] `shasum -a 256 file-en.txt file-zh.txt` 两 hash 一致（Source Content 不因 locale 翻译而变）
- [ ] raw diagnostic（错误详情原文）保持未翻译、与 UI 文案明确分区

## 7. 完成与关闭

- [ ] 四 Gate 全部 PASS，每项含 §2 模板字段与资产链接
- [ ] 在 issue #49 发布最终结果评论，`gh issue close 49`
- [ ] 签名/公证/Gatekeeper/真实 updater 升级继续由 #31/#17 跟踪