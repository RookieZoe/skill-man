# vNext 人工验收 Gate 本地测试清单（issue #49）

> 对应票：[验收:vNext 本机恢复、真实 Adopt、窗口与双语 Gate #49](https://github.com/RookieZoe/skill-man/issues/49)
> 权威依据：[vnext-implementation-spec.md §10.3 至 §10.4](vnext-implementation-spec.md)
> 这是一份人工 Gate 的执行清单。自动化测试、CI、真实 macOS/Tauri 运行时证据分别成立，任何一项都不能替代另一项。不要在 issue、截图文件名或日志中放入 Skill 正文、token、凭据或 remote response body。

## 0. 前置条件

- [ ] 从 issue #49 最新评论取得已通过 CI 的候选 SHA，记为 `CANDIDATE_COMMIT`。
- [ ] 同一 `CANDIDATE_COMMIT` 的 CI 已全绿，且包含 Tauri no-bundle 构建。
- [ ] 在干净 clone 或专用 worktree 构建。当前 checkout 有 WIP 时，不要在其中切换 candidate SHA。
- [ ] Gate A 开始前完全退出旧 Skill Man，确认没有 SQLite writer。
- [ ] Gate C 和 Gate D 的截图只截 Skill Man 窗口，避免把聊天、文件列表或其他桌面内容带入验收资产。
- [ ] 本票只验证未签名或开发构建的产品行为。Developer ID 签名、公证、Gatekeeper、真实 updater 升级和回滚属于发布 Gate，不是本票的通过条件。

### 0.1 选择本轮测试模式

| 当前磁盘状态                                 | 应执行的测试                     | 不应做的事                                      |
| -------------------------------------------- | -------------------------------- | ----------------------------------------------- |
| 默认 Home 中已有真实 Legacy 或 Bound Catalog | Gate A，先收集只读证据再考虑恢复 | 不要先删除 Home、SQLite、marker 或 app-state    |
| 默认 Home 和 `skill-man.sqlite3` 都不存在    | 首次启动 smoke，不是 Gate A      | 不要手工创建 SQLite 或 fixture 伪造 Gate A 输入 |

首次启动的产品契约是 **Unconfigured**：在操作者确认 Home Candidate 前，app 不创建 Home、Catalog SQLite、Home marker 或 binding。没有真实 Legacy/Bound Home 时，Gate A 应记录为 BLOCKED；只能从可信的现有备份或 Safety Snapshot 恢复真实输入，不能用新建数据库替代。

### 0.2 本机 Gate A 已知基线

以下是本轮首次只读采集的基线，不是可重复注入的 fixture。每次写操作前仍必须重跑 §3.2，并以当次结果为准。

| 证据                | 首次采集值                                                                     |
| ------------------- | ------------------------------------------------------------------------------ |
| Catalog schema      | v4，`snapshot_version = 9`                                                     |
| 行数                | skills 3，agents 3，activations 0，file_sources 0，remote_sources 0            |
| skills              | skill-authoring、media-xray、legacy-audit                                      |
| agents              | claude-code、codex、workbench                                                  |
| `fixture-entities/` | 仅 `skill-authoring/SKILL.md` 与 `media-xray/SKILL.md`，没有 legacy-audit 实体 |
| SQLite              | `integrity_check = ok`，`foreign_key_check` 无输出                             |

预期 bootstrap 分类是 **Pure fixture -> Fixture Recovery Lock**。若 app 显示 Mixed、Unknown 或任何其他分类，先记录完整 Preview 和原因，在任何写操作前停止本 Gate。不要通过删除 `fixture-entities/`、改数据库、修改 lock 或运行 fixture 测试来让分类“变对”。

## 1. 构建候选 app

在干净 clone 或专用 worktree 执行。若 `git status --short` 有输出，保留该 WIP，改用另一个 clone 或 worktree。

```bash
cd /path/to/clean/skill-man
export CANDIDATE_COMMIT=<issue-49-latest-comment-sha>
git fetch origin
git switch --detach "$CANDIDATE_COMMIT"
test "$(git rev-parse HEAD)" = "$CANDIDATE_COMMIT"

npm ci
export TARGET=aarch64-apple-darwin
export VERSION="$(node -p "require('./package.json').version")"
npm run tauri build -- --target "$TARGET"

export DMG="src-tauri/target/$TARGET/release/bundle/dmg/Skill Man_${VERSION}_aarch64.dmg"
test -f "$DMG"
shasum -a 256 "$DMG"
open "$DMG"
```

- [ ] 在 issue 记录 `CANDIDATE_COMMIT`、DMG SHA-256、构建日期和操作者。
- [ ] 从挂载的 DMG 打开 `Skill Man.app`。需要安装时，在 Finder 中拖到 Applications 并选择替换旧副本。
- [ ] `tauri build` 创建 DMG 后会清理 `bundle/macos/Skill Man.app`。不要把该临时路径当成构建后必须存在的验收产物。
- [ ] 需要检查 `document.lang` 时，单独运行 `npm run tauri dev`。先退出 release app，避免两个实例同时访问同一个 Home。
- [ ] Gatekeeper 弹窗不是本 Gate 的结果。若本机要启动未签名构建，可右键 app 后选择“打开”。

## 2. issue #49 记录模板

每个 Gate 单独发一条记录。失败或阻塞也要记录，不要以自动化结果代替人工结论。

```text
Gate <A/B/C/D>: <名称>
- 日期：YYYY-MM-DD
- 操作者：<姓名>
- build commit：<CANDIDATE_COMMIT>
- DMG SHA-256：<hash>
- 输入摘要：<Home 分类、候选、窗口 viewport、系统语言序列>
- 结果：PASS / FAIL / BLOCKED
- 证据：<已脱敏的截图、命令输出或文件链接>
- 未记录内容：Skill 正文、token、凭据、remote response body
```

## 3. Gate A：本机数据恢复（仅已有 Legacy/Bound Home）

### 3.1 已有 Home 时，启动前证明无 writer

Gate A 的 writer 检查只在 Home 和 Catalog 已存在时执行。对干净首次启动，DB 路径不存在是预期状态，`lsof` 因而会报“no such file or directory”，这不是 app 错误。

```bash
export HOME_ROOT="$HOME/Library/Application Support/skill-man"
export DB="$HOME/Library/Application Support/skill-man/skill-man.sqlite3"
test -d "$HOME_ROOT"
test -f "$DB"
pgrep -fl skill-man
lsof "$DB"
```

- [ ] `test -d "$HOME_ROOT"` 和 `test -f "$DB"` 都通过后，才执行 `lsof "$DB"`。
- [ ] `pgrep -fl skill-man` 无输出。
- [ ] `lsof "$DB"` 无输出。若有进程，先退出该进程，重新执行这两条命令。
- [ ] 记录命令执行时间。不要在自己的 SQLite 只读检查仍打开时宣称“无 writer”。
- [ ] 任一 `test` 失败时：不要运行 `lsof` 或 §3.2 的 SQLite 查询。记录“no existing Home/Catalog”，转到 §3.1a；若本轮目标是 Gate A，则记录 BLOCKED。

### 3.1a 干净首次启动 smoke（补充测试，不是 Gate A）

此分支验证首次绑定边界，不产生可替代 Gate A 的恢复证据。

- [ ] 确认默认 Home 和 `skill-man.sqlite3` 都不存在后启动 app。
- [ ] app 首屏应为 Unconfigured，不能显示 LegacyDetected、Bound 或 Fixture Recovery Lock。
- [ ] 在尚未确认 Home Candidate 前，再次检查默认 Home 和 `skill-man.sqlite3` 仍不存在。
- [ ] 可进入 Use Default 或 Choose 的候选确认页，验证取消后仍保持 Unconfigured、零 Home/SQLite artifact。
- [ ] 只有准备验证首次绑定时，才由操作者确认 Candidate。该确认会创建 Home 和 Catalog，改变当前测试环境。
- [ ] 不要在此干净状态手工创建数据库、复制 fixture 或修改 locator 来继续 Gate A。

### 3.2 写操作前的只读证据

仅在 §3.1 的 Home/Catalog 存在检查通过后执行。所有 SQLite 命令都使用 `-readonly`，避免测试本身创建 WAL 或改变状态。

```bash
sqlite3 -readonly "$DB" 'PRAGMA integrity_check;'
sqlite3 -readonly "$DB" 'PRAGMA foreign_key_check;'
sqlite3 -readonly "$DB" 'SELECT schema_version, snapshot_version FROM catalog_meta;'
sqlite3 -readonly "$DB" "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name;"
```

按上一步的 schema version 选择 row count 查询，并保留原始输出到本地脱敏证据中。

```bash
# schema v4 或 v5：remote_sources 仍存在
sqlite3 -readonly "$DB" "
SELECT 'skills', count(*) FROM skills
UNION ALL SELECT 'agents', count(*) FROM agents
UNION ALL SELECT 'activations', count(*) FROM activations
UNION ALL SELECT 'file_sources', count(*) FROM file_sources
UNION ALL SELECT 'remote_sources', count(*) FROM remote_sources;"

# schema v6：remote_sources 已被 remote source parent/binding 表取代
sqlite3 -readonly "$DB" "
SELECT 'skills', count(*) FROM skills
UNION ALL SELECT 'agents', count(*) FROM agents
UNION ALL SELECT 'activations', count(*) FROM activations
UNION ALL SELECT 'file_sources', count(*) FROM file_sources
UNION ALL SELECT 'remote_source_parents', count(*) FROM remote_source_parents
UNION ALL SELECT 'remote_source_aliases', count(*) FROM remote_source_aliases
UNION ALL SELECT 'remote_bindings', count(*) FROM remote_bindings;"
```

- [ ] `integrity_check` 为 `ok`，`foreign_key_check` 无输出。
- [ ] row counts、schema version 和 table set 已记录。恢复后先重新读取 schema version 和 table set，再选择对应的计数查询，不能机械复用恢复前的 v4/v5 块。
- [ ] `catalog_meta.home_id` 仅在 v5 及以上存在。即使它非空，也不能单独证明 Bound，只有 locator、Home marker 和 Catalog 三方一致才是 Bound。让 app 的 bootstrap 分类作最终判断。

### 3.3 启动 app，记录 bootstrap 分类

- [ ] 启动 §1 的候选 `.app`，截取 Skill Man 窗口首屏。
- [ ] 不点击写操作，先记录 route、路径和 diagnostic。

| app 分类                               | 人工操作边界                                                                       |
| -------------------------------------- | ---------------------------------------------------------------------------------- |
| Unconfigured                           | 干净首次启动 smoke，不是 Gate A。按 §3.1a 验证或记录 Gate A BLOCKED                |
| Fixture Recovery Lock                  | 进入 §3.4，只能使用 RecoveryView 的确认流程                                        |
| LegacyDetected                         | 进入 §3.5，只有在没有 Fixture Recovery Lock 时才可绑定                             |
| Bound                                  | 记录 home_id 与当前可读状态，不重复首次绑定                                        |
| HomeUnavailable / HomeIdentityMismatch | 记录 diagnostic，只使用该 route 允许的 Reconnect、Restore 或 Abandon 流程          |
| Abandoned                              | 旧 Home 不会重新绑定。Default 应保持不可用，只能通过 Choose 选择新的合法 Home 候选 |
| AppStateUnavailable                    | 不写、不绑，记录 diagnostic 后停止                                                 |

### 3.4 Fixture Recovery Lock：恢复流

本机预期走这条路径。

- [ ] RecoveryView 的 Preview 显示分类、Catalog evidence、tree evidence 和所有原因。
- [ ] 截取已脱敏的证据区。Pure 分类不应通过手工修改数据库或目录获得。
- [ ] 由操作者明确确认后才执行页面上的 recovery plan、apply、confirm 流程。
- [ ] 记录 Safety Snapshot 路径，格式为 `<Home>.snapshot-<op-id>/`，以及 manifest hash。
- [ ] Recovery 完成后重跑 §3.2 的完整只读证据，记录前后 row counts。
- [ ] 在 SafetySnapshots 页面记录 snapshot 的存在。不要手动删除 snapshot。
- [ ] Legacy recovery 成功确认后，bootstrap 才应进入 LegacyDetected，再继续 §3.5。

### 3.5 Legacy 首次绑定与 Rescan

- [ ] 记录 Legacy summary 和原始 Home path。
- [ ] 选择 Use Default 时，确认它是 Legacy 原位绑定。选择 Choose 时，确认它走的是唯一一次 copy 过渡。
- [ ] 在确认页核对 path、模式和 explicit confirmation 文案后，再由操作者确认。
- [ ] 绑定后记录新 home_id、Home marker 和 Catalog 状态。
- [ ] 在 Bound Home 的 WriteGate 已 Open 后手动运行 Rescan，只允许它报告真实 Untracked。不得出现自动 Adopt、Enable 或 Activation Repair。
- [ ] 记录 Complete/Incomplete、Root coverage 与 Report generation；只有 Complete Report 可满足 Safety Snapshot 后续删除资格。
- [ ] Rescan 结果经操作者确认前，不执行其它普通产品写操作。

### 3.6 全程禁止项

- [ ] 不删除 `fixture-entities/`，不运行会向真实 Home 注入 fixture 的测试。
- [ ] 不手动 Relocate、改址或普通 re-home。绑定后只能 Reconnect、Restore 或 Abandon。
- [ ] locale 是 Home 外状态，Agent skills 目录不是 Home 内容。

## 4. Gate B：真实多跳 Adopt

仅在 Gate A 已完成、app 处于允许写入的 Bound 状态后执行。候选必须来自真实环境的 Evidence Ledger，不能通过新建 symlink、修改 `.skill-lock.json` 或插入数据库行来伪造。

### 4.1 候选选择

- [ ] 选一个 Ledger 中的 **Local Link** 候选，要求有多 hop 或多 appearance。
- [ ] 选一个 Ledger 中的真实 **lock-managed remote** 候选。
- [ ] Agent roots 包含 app 已配置的各 Agent skills path，另加共享 `~/.agents/skills`。不要把共享目录误当成唯一扫描根。
- [ ] lock path 以 Ledger 实际显示的路径为准。默认 lock 是 `~/.agents/.skill-lock.json`，也可能显示 XDG 变体。
- [ ] 没有符合条件的真实候选时，记录 BLOCKED。不要把 fixture 或临时目录包装成候选。

### 4.2 Evidence Ledger 核对表

每个候选逐项记录，不贴 lock 正文或 Skill 正文。

- [ ] appearance、每个 hop 的路径和类型与磁盘一致。
- [ ] lock path、entry name、lock fingerprint、requested ref 和 ref disposition 一致。
- [ ] Verification Anchor、local tree hash、remote tree hash、`trees_match` 已记录。
- [ ] Include 只出现在 selectable 候选，Blocked、Deferred、Excluded 不可 Include。
- [ ] verdict 和最终 owner 与证据一致。

### 4.3 Apply 前后证据

`tree-sha256-v1` 由 Evidence Ledger 提供，是 source/Home 树比对的权威值。不要用单个 `cmp SKILL.md` 代替整棵树验证，也不要复制 Skill 正文作为证据。

- [ ] Apply 前记录 source path、Home path、source/Home tree hash、lock fingerprint 和 Activation 状态。
- [ ] 从 app 的 Activation 信息取得绝对 activation path，记录它是否存在及当前 symlink target。

```bash
# 仅对 app 显示的实际 path 执行。不要猜测 activations 表有 path 列。
export ACTIVATION_PATH="/absolute/path/shown-by-the-app"
if [ -L "$ACTIVATION_PATH" ]; then
  readlink "$ACTIVATION_PATH"
else
  printf '%s\n' 'activation absent or not a symlink'
fi

# 只记录 lock 的 hash，不显示其内容。
shasum -a 256 "/absolute/path/to/the-ledger-lock-file"
```

- [ ] 操作者勾选 Include 后执行 Apply。
- [ ] Apply 后重新记录 source/Home tree hash、activation target、最终 owner 和 Ledger verdict。
- [ ] 只在 issue 中记录 hash、路径摘要和结果，不上传正文、token、凭据或 lock 内容。

## 5. Gate C：Tauri/WKWebView 视觉

Gate C 的证据必须来自 §1 构建的真实 `.app`。`npm run matrix:layout` 只是在浏览器中运行的辅助预检，不能代替 Tauri/WKWebView 截图或 VoiceOver 记录。

### 5.1 尺寸与状态矩阵

布局断点读取 `window.innerWidth`：760 至 1059 为 mid，1060 及以上为 wide。窗口外框尺寸和 WebView viewport 可能不同，使用 DevTools 时以 `window.innerWidth`、`window.innerHeight` 为准。

```bash
# 仅对从 DMG 打开的 Skill Man.app 有效。
# Terminal 或 iTerm 需要在“系统设置 > 隐私与安全性 > 辅助功能”中获准控制电脑。
osascript -e 'tell application "System Events" to tell process "Skill Man" to set size of window 1 to {760, 520}'
```

- [ ] 若上述命令报 `-1719`，这是终端没有辅助功能权限，不是 app 失败。授权后重试，或手动 resize 并记录实际 viewport。
- [ ] 在 760×520、1059px、1060px、高窗口分别截图。每张图写明实际 `window.innerWidth × window.innerHeight`。
- [ ] 760 至 1059 宽度核对 mid 布局和 Agent drawer，1060 宽度核对 wide 三栏布局。
- [ ] 每个尺寸核对 0、1、multiple Notice，empty、error、dense English、dense 简体中文，以及没有 page 横向溢出。

不要在真实 Home 中插 fixture、改数据库或制造损坏来产生 Notice、empty 或 error。只能使用已有的可逆 UI 路径或独立、可丢弃的 QA Home。若某状态不能安全复现，记录 BLOCKED 和原因。

### 5.2 跨断点 overlay

- [ ] 打开 Preferences、Adopt Evidence Ledger 或 Import sheet/drawer。
- [ ] 保持 overlay 打开，跨 1059/1060 断点 resize。
- [ ] 核对三栏、双栏/drawer、Notice tray 各自的 scroll owner，Toolbar 固定。
- [ ] 核对没有空白区和横向溢出。
- [ ] Tab 焦点被限制在 dialog/drawer；Escape 关闭；关闭后焦点回触发器。
- [ ] busy 只用已经允许且可逆的操作触发。不要为了观察 disabled 态执行 Fixture Recovery 或未确认的 Apply。
- [ ] resize 后已选 Include、表单输入和 overlay DOM 不 remount、不丢失。

### 5.3 VoiceOver 与截图

- [ ] 用 ⌘F5 开启 VoiceOver，记录标题、列表、按钮、dialog 的 role 和 label。
- [ ] 每个截图只含 app 窗口，文件名包含 Gate、locale、viewport 和时间。
- [ ] 截图中出现本地路径、远程 URL 或 diagnostic 时，上传前审查是否含敏感信息。

## 6. Gate D：双语 native QA

### 6.1 测试顺序

系统语言切换会影响整台 Mac。优先使用测试 macOS 用户，或在开始前记录如何恢复原系统语言。

- [ ] 系统语言设为 English，冷启动 release app，记录首帧。
- [ ] 在当前 route 的 LanguageControl 选择简体中文，确认即时生效。
- [ ] 选择 System，确认回到系统首选语言。
- [ ] 系统语言改为简体中文，关闭并重新启动 app，记录首帧。
- [ ] 重激活 app 后再重启一次，确认 selection 和 effective locale 一致。

LanguageControl 的位置取决于 route：Fixture Recovery、Binding、Lifecycle route 内直接可见；Bound 状态在 Settings 齿轮打开的 Preferences sheet 内。不要假定它固定在页面底部或工具栏。

### 6.2 每个 locale 的核对项

- [ ] WebView 首帧、LanguageControl 和 Preferences。
- [ ] 在单独的 `npm run tauri dev` 会话中打开 Inspector，执行 `document.documentElement.lang`，记录 `en` 或 `zh-Hans`。完成后退出 dev app，再继续 release app 验收。
- [ ] menu-bar tray：Open Window、Quit、最近启用技能和 count/health 文案。
- [ ] macOS native menu：Window、Help、Quit（⌘Q）。locale 切换后关闭并重新打开菜单核对更新。
- [ ] VoiceOver 的 ARIA role/label。
- [ ] 日期、数字、byte size 等格式化。
- [ ] 公开错误的 UI 文案与 raw diagnostic 的技术内容明确分区。不要破坏 Home、SQLite 或 lock 来制造错误。

### 6.3 Source Content byte equality

选择同一个 Evidence Ledger 候选，比较两种 locale 下的 **原始字段值**，不是翻译后的 App Copy。建议固定比较 final entity、每个 hop path/target、lock path/fingerprint、requested ref、Verification Anchor、local/remote tree hash。

- [ ] 在 English 与简体中文下分别打开同一个候选，不改变 Scan generation 或候选。
- [ ] 逐项核对上述 raw fields 字符串完全一致，只有 verdict、标题、按钮等 App Copy 可以翻译。
- [ ] 若需要本地 hash 证据，只把这些非正文 raw fields 写进两个临时 UTF-8 文件，执行 `cmp -s en.txt zh.txt`，期望 exit code 为 0；记录两个 SHA-256 和比较结果后删除临时文件。
- [ ] 不把临时文件、Skill 正文、token、凭据、完整 lock 或 remote response body 上传到 issue。

## 7. 完成与关闭

- [ ] Gate A 至 D 全部为 PASS，每项都有 §2 所需字段和可审计资产链接。
- [ ] 某项 FAIL 或 BLOCKED 时保持 #49 打开，写明可复现条件和缺失证据。
- [ ] 只有人类操作者确认四项全过后，才在 #49 发布最终结果并关闭 issue。
- [ ] 签名、公证、Gatekeeper、真实 updater 升级和回滚继续由发布 Gate 跟踪。
