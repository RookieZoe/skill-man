# 各 AI Coding Agent 的 Skills 发现/加载机制对比调研

> 调研日期:2026-07-20。证据优先级:官方源码 > 官方文档 > 官方 issue/CHANGELOG。源码引用均使用当日各仓库 `main` 分支 HEAD 的 permalink(SHA 见文末)。本调研重点回答硬指标:**各 agent 的技能发现是否跟随符号链接(symlink)**,含嵌套软链与 dangling(目标缺失)软链行为。

## 1. 对比总表

| Agent | Skills 目录(个人级 / 项目级 / 内建) | 清单文件 | 发现机制 | 启用/禁用 | 同名冲突处理 | **是否跟随软链(含嵌套与 dangling)** | 证据强度 |
|---|---|---|---|---|---|---|---|
| **Claude Code** | 个人 `~/.claude/skills/<name>/`;项目 `.claude/skills/<name>/`(含父目录上溯与嵌套 monorepo 发现);plugin `<plugin>/skills/`;另有 enterprise(managed settings)与 bundled | `SKILL.md`(YAML frontmatter + Markdown) | 启动扫描 + 目录 watch 热加载(新增顶层 skills 目录需重启);嵌套 `.claude/skills` 按需发现 | frontmatter `disable-model-invocation` / `user-invocable`;settings `skillOverrides`;权限规则 `Skill(name)` | enterprise > personal > project > bundled;plugin 用 `plugin:skill` 命名空间隔离;skill 与 command 同名时 skill 优先 | **条目级软链官方明确支持并去重**;历史上有整目录软链回归(#38051)与自动更新删软链(#50052,未修复确认);嵌套/dangling 行为无官方说明 | 官方文档 + CHANGELOG + 多个 issue + 本机观察(闭源,无源码) |
| **Codex (OpenAI)** | 用户 `~/.agents/skills`(新)+ `$CODEX_HOME/skills`(即 `~/.codex/skills`,deprecated 但兼容);项目 `$CWD` 至仓库根每级 `.agents/skills`;管理员 `/etc/codex/skills`;内建 `~/.codex/skills/.system`(启动时自动安装) | `SKILL.md`(name/description 必需)+ 可选 `agents/openai.yaml` | 启动时遍历各 root:深度 ≤6、≤2000 目录、≤20000 条目;progressive disclosure | `config.toml` 中 `[[skills.config]] path=… enabled=false`;`openai.yaml` 的 `allow_implicit_invocation` | 按 canonicalize 后路径做身份去重(同一目标多处可达只加载一次) | **跟随:User/Repo/Admin 作用域 `DirectorySymlinkPolicy::Follow`,System 作用域 Ignore**;遍历错误(含 dangling)收集为 warning 不中断;官方文档明示支持软链 skill 目录;嵌套链未单独验证 | **官方源码**(loader.rs/discovery.rs)+ 官方文档 + issue #8369/#8943 |
| **Gemini CLI** | 用户 `~/.gemini/skills/` 或别名 `~/.agents/skills/`;工作区 `.gemini/skills/` 或别名 `.agents/skills/`(需信任文件夹);extension 附带;builtin(包内) | `SKILL.md`(name/description) | 会话开始扫描;glob 模式 `['SKILL.md','*/SKILL.md']`,即**只下钻一层**(深度 ≤2) | `/skills disable\|enable`(默认 user scope,`--scope workspace`)、`/skills list`、`/skills reload`;CLI `gemini skills install/uninstall`;激活时有 consent 弹窗 | builtin < extension < user < workspace;同 tier 内 `.agents` 别名优先于 `.gemini` | **跟随(事实支持)**:官方 `/skills link` 命令本身就是往 skills 目录建 `fs.symlink(dir)`;本地实验(glob 13.0.6)证实条目软链、嵌套链、指向树外目标均可发现,dangling 静默跳过,skills 根目录本身可为软链 | 官方源码(skillLoader/skillUtils)+ 本地库行为实验(嵌套/dangling 为实验结论,非官方承诺) |
| **Cursor** | 项目 `.cursor/skills/`、`.agents/skills/`;用户 `~/.cursor/skills/`、`~/.agents/skills/`;并兼容加载 `.claude/skills/`、`.codex/skills/`(含用户级) | `SKILL.md`(name 须与父目录同名,description 必填;可选 `paths`、`disable-model-invocation`、`metadata`) | 启动时自动发现;**递归** walk skills root,任意深度的 `SKILL.md` 都会拾取;嵌套 skill 自动限定作用范围 | frontmatter `disable-model-invocation`(退化为显式 `/skill-name`);`paths` 限定触发文件 | 文档未明确跨目录同名优先级 | **实证跟随**:项目级条目软链、嵌套链与 skills 根目录软链均加载;dangling 静默跳过;名称错配时以 frontmatter `name` 注册 | 官方文档 + Cursor 3.12.17 本机 UI 实验(ticket #13) |
| **opencode** | 项目 `.opencode/skill(s)/<name>/`;全局 `~/.config/opencode/skill(s)/<name>/`;**外部自动加载** `~/.claude/skills`、`~/.agents/skills`(全局)及从 cwd 上溯至 worktree 的项目级 `.claude/skills`、`.agents/skills`;config `skills.paths` 自定义目录、`skills.urls` 远程 | `SKILL.md`(name/description 必填 + license/compatibility/metadata;name 须匹配目录名) | `Glob.scan`(npm glob);external 模式 `skills/**/SKILL.md`,opencode 模式 `{skill,skills}/**/SKILL.md`,自定义 `**/SKILL.md` | runtime flags 可禁用 external/claude skills;未发现 per-skill 开关 | 同名记 warning,**后扫描者覆盖先扫描者**(扫描顺序:external → opencode config → 自定义 paths) | **意图上跟随**(源码恒传 `follow: true`);但 issue #18848(截至调研日 open 未修复)报告 git worktree sandbox 下 `.claude/skills` 为软链时项目级 skills 不发现(根因:glob 不下钻 + 沙盒会话状态隔离) | **官方源码**(index.ts/glob.ts)+ 未修复 issue + 本地库行为实验 |

补充说明:`~/.agents/skills/` 已成为跨 agent 事实标准共享层 —— Codex(用户层主目录)、Gemini CLI(别名)、Cursor(原生目录)、opencode(外部自动加载)均读取它(见各节引用)。本机该目录同时存在真实目录与指向各仓库的软链(见「本机只读观察」)。

## 2. 各 agent 详情

### 2.1 Claude Code(闭源)

**目录与优先级.** 四个层级:Enterprise(managed settings)> Personal `~/.claude/skills/<skill-name>/SKILL.md` > Project `.claude/skills/<skill-name>/SKILL.md` > bundled;同名时高层级覆盖低层级,任一层级都可覆盖同名 bundled skill。Plugin skills 使用 `plugin-name:skill-name` 命名空间,不与其他层级冲突;`.claude/commands/` 旧式命令与同名 skill 并存时 skill 优先。([官方文档 Where skills live](https://code.claude.com/docs/en/skills))

**发现机制.** 启动时扫描,并 watch 技能目录实现会话内热加载;但「会话启动后才新建的顶层 skills 目录」需重启才能被 watch。项目 skills 从启动目录向上加载到仓库根;工作进入子目录时按需发现嵌套 `.claude/skills/`(monorepo 场景),嵌套同名 skill 以目录限定名 `apps/web:deploy` 共存。`--add-dir`/`/add-dir` 是配置发现的例外:被加目录下的 `.claude/skills/` 会自动加载。([官方文档 Live change detection / Automatic discovery](https://code.claude.com/docs/en/skills))

**SKILL.md 格式.** YAML frontmatter 全部字段可选,推荐 `description`(与 `when_to_use` 合计截断于 1536 字符);`name` 仅作展示名,命令名来自目录名(plugin 根 SKILL.md 例外)。扩展字段包括 `disable-model-invocation`、`user-invocable`、`allowed-tools`/`disallowed-tools`、`context: fork`、`paths`、`hooks` 等。([官方文档 Frontmatter reference](https://code.claude.com/docs/en/skills))

**软链行为(硬指标).**

- **条目级软链:官方明确支持。** 文档原文:enterprise/personal/project 位置的 `<skill-name>` 条目「can be a symlink to a directory elsewhere on disk. Claude Code follows the symlink and reads SKILL.md from the target directory, and if the same target is reachable from more than one location, Claude Code loads the skill once」([官方文档](https://code.claude.com/docs/en/skills))。
- **Plugin/marketplace 内软链规则不同**:插件目录内部软链在缓存中保留为相对软链;指向同一 marketplace 内其他位置的软链被解引用(内容拷入缓存);指向 marketplace 之外的软链**出于安全被跳过**;`--plugin-dir`/本地路径安装的插件只保留插件内部软链。([plugins-reference: Share files within a marketplace with symlinks](https://code.claude.com/docs/en/plugins-reference#share-files-within-a-marketplace-with-symlinks))
- **CHANGELOG 中的软链修复史**([anthropics/claude-code CHANGELOG.md](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)):
  - v2.0.62:「Fixed an issue where skill files inside symlinked skill directories could become circular symlinks」;同版修复 `~/.claude` 为软链时 slash command 重复。
  - v2.1.69:「Fixed symlink bypass where writing new files through a symlinked parent directory could escape the working directory in acceptEdits mode」——正是这个安全修复被指引发后续回归。
  - v2.1.178:「Fixed Linux sandbox failing to start when .claude/skills or .claude/hooks is a symlink」。
  - v2.1.198:修复 `.claude/rules/` 条件规则经软链路径不加载。
- **相关 issue**:
  - [#38051](https://github.com/anthropics/claude-code/issues/38051):`~/.claude/skills` **整目录**为软链时用户级 skills 不加载,报告称自 ~v2.1.69 回归(2.1.81 仍在);状态 closed,页面无官方回应/修复说明。workaround:skills 目录用真实目录、内部逐条目建软链。
  - [#50052](https://github.com/anthropics/claude-code/issues/50052):**自动更新静默删除 `~/.claude/skills/` 中的用户软链**,真实目录不受影响;closed as not planned(stale),无官方回应 —— 对「app 建软链管理 skills」方案是直接风险。
  - [#14836](https://github.com/anthropics/claude-code/issues/14836)(/skills 列表不显示软链目录但可执行)、[#25367](https://github.com/anthropics/claude-code/issues/25367)(软链 skill 报 "Unknown skill" 但执行正常)、[#36659](https://github.com/anthropics/claude-code/issues/36659)(`.claude` 为软链时自动补全缺失)——均为发现/展示层与执行层不一致的历史表现。
  - [#20755](https://github.com/anthropics/claude-code/issues/20755):递归发现 `.claude/skills/` 的 feature request(现状:只认直接子目录)。
- **嵌套软链/dangling**:无官方一手说明,**未验证**。v2.0.62 修复过「循环软链」,侧面说明存在过相关边界处理。
- **本机观察(2026-07-20,只读)**:`~/.claude/skills/` 下 27 个条目是指向 `~/.agents/skills/*` 的软链(含一个相对路径软链 `mmx-cli -> ../../.agents/skills/mmx-cli`);本会话可用技能列表中包含其中多个软链 skill(如 `tdd`、`research`、`kami`、`mmx-cli`、`better-skill-creator` 等),证实**当前版本条目级软链(含相对软链)可正常发现并加载**。

### 2.2 Codex(OpenAI,开源 Rust)

**目录(源码一手).** `skill_roots` 枚举的 root([loader.rs L303-L387](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L303-L387)):

- 用户层:`$CODEX_HOME/skills`(即 `~/.codex/skills`,注释明示「Deprecated user skills location, kept for backward compatibility」,[L331-L341](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L331-L341))+ **新主位置 `$HOME/.agents/skills`**([L343-L353](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L343-L353));
- 内建 system:缓存于 `$CODEX_HOME/skills/.system`,由嵌入资源在启动时安装([L355-L364](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L355-L364);安装实现 [codex-rs/skills/src/lib.rs](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/skills/src/lib.rs));
- 管理员:`/etc/codex/skills`([L366-L377](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L366-L377));
- 项目层:从 `$CWD` 到 project root 之间每一级的 `.agents/skills`([L389-L431](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L389-L431))。

官方文档对应描述为 REPO / USER / ADMIN / SYSTEM 四层([Codex skills 文档](https://learn.chatgpt.com/docs/build-skills),由 developers.openai.com/codex/skills 301 跳转)。本机 `~/.codex/skills/` 实测存在 `.system/` 子目录(内含 `skill-creator`、`imagegen` 等内建 skill 与 `.codex-system-skills.marker`),与源码一致。

**发现机制.** 对每个 root 做目录遍历:`MAX_SCAN_DEPTH = 6`、`MAX_SKILLS_DIRS_PER_ROOT = 2000`、`MAX_SKILLS_ENTRIES_PER_ROOT = 20000`([loader.rs L154-L155](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L154-L155)、[discovery.rs L63-L81](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader/discovery.rs#L63-L81));截断或逐路径错误只产生 warning([discovery.rs L94-L109](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader/discovery.rs#L94-L109))。root 不存在(NotFound)返回空结果不报错([discovery.rs L83-L91](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader/discovery.rs#L83-L91))。隐藏目录默认跳过([loader.rs L557-L562](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L557-L562))。

**清单格式.** 目录内含 `SKILL.md`(`SKILLS_FILENAME`,[loader.rs L137](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L137)),frontmatter 需 `name`/`description`;可选 `agents/openai.yaml` 元数据([loader.rs L139-L140](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L139-L140))。与 Claude Code 同属 Agent Skills 开放标准([文档](https://learn.chatgpt.com/docs/build-skills)),frontmatter 基础字段兼容。

**启用/禁用.** `~/.codex/config.toml` 中 `[[skills.config]] path = "…/SKILL.md"  enabled = false`([文档](https://learn.chatgpt.com/docs/build-skills))。

**软链行为(硬指标)—— 源码一手.**

- 按作用域决定目录软链策略:`SkillScope::User | SkillScope::Repo | SkillScope::Admin => DirectorySymlinkPolicy::Follow, SkillScope::System => DirectorySymlinkPolicy::Ignore`([loader.rs L546-L549](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L546-L549)),该标志以 `follow_directory_symlinks` 传入遍历器([discovery.rs L66-L78](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader/discovery.rs#L66-L78))。即**用户/项目/管理员目录跟随软链;`.system` 内建目录不跟随**。
- 身份去重:对 skill 路径做 `canonicalize_for_skill_identity`([loader.rs L522-L543](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L522-L543)),root 级别也按路径去重([L517](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader.rs#L517))——同一真实目录经多个软链/多个 root 可达时只加载一次。
- **dangling 行为(源码推断)**:遍历逐路径错误被收集进 `walk.errors` 再转为 warning,扫描继续、不崩溃([discovery.rs L94-L109](https://github.com/openai/codex/blob/3dd3c5d08ac811f0270d47eff75e42356484e193/codex-rs/core-skills/src/loader/discovery.rs#L94-L109))。嵌套软链链未单独验证。
- 官方文档确认:「Codex supports symlinked skill folders and follows the symlink target when scanning these locations」([文档](https://learn.chatgpt.com/docs/build-skills))。
- 历史:[#8369](https://github.com/openai/codex/issues/8369)(2025-12,请求支持软链 skill,closed)、[#8943](https://github.com/openai/codex/issues/8943)(0.79.0 时代「loader does not follow symlinks」,作为 #8369 重复关闭)——说明软链支持是后加的,当前源码已实现 Follow。

### 2.3 Gemini CLI(开源 TypeScript)

**是否有 skills**:有(2026 年已内置),基于 Agent Skills 开放标准,区别于 GEMINI.md 常驻上下文与 extensions/commands([官方文档 docs/cli/skills.md](https://github.com/google-gemini/gemini-cli/blob/acae7124bdd849e554eaa5e090199a0cf08cd782/docs/cli/skills.md))。

**目录与优先级.** 四级(从低到高):builtin(随包发布)→ extension skills → 用户 `~/.gemini/skills/` 或别名 `~/.agents/skills/` → 工作区 `.gemini/skills/` 或别名 `.agents/skills/`(未信任文件夹下 workspace skills 禁用)。同名时高优先级层覆盖;同层内 `.agents` 别名优先于 `.gemini`。([文档 Discovery tiers](https://github.com/google-gemini/gemini-cli/blob/acae7124bdd849e554eaa5e090199a0cf08cd782/docs/cli/skills.md);源码 [skillManager.ts L54-L98](https://github.com/google-gemini/gemini-cli/blob/acae7124bdd849e554eaa5e090199a0cf08cd782/packages/core/src/skills/skillManager.ts#L54-L98))

**发现机制.** 会话开始扫描,把启用 skill 的 name/description 注入 system prompt;激活经 `activate_skill` 工具 + 用户 consent([文档 How it works](https://github.com/google-gemini/gemini-cli/blob/acae7124bdd849e554eaa5e090199a0cf08cd782/docs/cli/skills.md))。加载器对每个 skills 目录执行 `glob(['SKILL.md', '*/SKILL.md'], { cwd, absolute: true, nodir: true, ignore: ['**/node_modules/**','**/.git/**'] })`([skillLoader.ts L115-L159](https://github.com/google-gemini/gemini-cli/blob/acae7124bdd849e554eaa5e090199a0cf08cd782/packages/core/src/skills/skillLoader.ts#L115-L159))——**只下钻一层子目录**(深度 ≤2),且 glob 调用未传 `follow` 选项。

**启用/禁用.** `/skills list` / `/skills disable <name>` / `/skills enable <name>`(默认 user scope,`--scope workspace`)、`/skills reload`;终端 `gemini skills install <git-url|dir>` / `gemini skills uninstall`;`/skills link <path> [--scope user|workspace]` 从本地目录链接 skill。([文档 Managing skills](https://github.com/google-gemini/gemini-cli/blob/acae7124bdd849e554eaa5e090199a0cf08cd782/docs/cli/skills.md))

**软链行为(硬指标).**

- 加载器源码没有任何显式软链处理([skillLoader.ts L127-L133](https://github.com/google-gemini/gemini-cli/blob/acae7124bdd849e554eaa5e090199a0cf08cd782/packages/core/src/skills/skillLoader.ts#L127-L133))。
- **但官方自己的 `/skills link` 就是建目录软链**:`linkSkill` 对每个 skill 调 `fs.symlink(skillSourceDir, destPath, 'dir')`(Windows 用 junction)把它挂进 user/workspace skills 目录([skillUtils.ts L211-L287](https://github.com/google-gemini/gemini-cli/blob/acae7124bdd849e554eaa5e090199a0cf08cd782/packages/cli/src/utils/skillUtils.ts#L211-L287),symlink 调用在 L278-L282;`installSkill` 则走 `fs.cp` 拷贝,[L196](https://github.com/google-gemini/gemini-cli/blob/acae7124bdd849e554eaa5e090199a0cf08cd782/packages/cli/src/utils/skillUtils.ts#L196))。官方功能以软链为安装方式,是「条目级软链可被发现」的强佐证。
- **本地实验(非官方承诺)**:macOS 上用 npm `glob@13.0.6` 复刻其调用参数实测 —— 条目级目录软链、嵌套软链链(link→link→real)、指向 skills 树外目标的软链均能被 `*/SKILL.md` 发现;dangling 软链静默跳过(无报错);skills 根目录本身是软链也可发现。
- 本机无 `~/.gemini`(未安装 Gemini CLI),目录结构未能本机验证。

### 2.4 Cursor(闭源)

**是否有 skills**:有 Agent Skills(与 `.cursor/rules` 并存:rules 是常开上下文,skills 按需加载;内置 `/migrate-to-skills` 可把动态 rules 迁移为 skills)。([Cursor 官方文档](https://cursor.com/docs/context/skills))

**目录.** 自动加载四处:项目 `.agents/skills/`、`.cursor/skills/`;用户 `~/.agents/skills/`、`~/.cursor/skills/`;并为兼容额外加载 `.claude/skills/`、`.codex/skills/` 及其用户级对应目录。([Cursor 文档](https://cursor.com/docs/context/skills))

**发现机制.** 「When Cursor starts, it automatically discovers skills from skill directories」;**递归**遍历:「Cursor walks the skills root recursively and picks up any SKILL.md it finds」,嵌套 skill 自动限定作用范围;聊天中输入 `/` 可手动调用。([Cursor 文档](https://cursor.com/docs/context/skills))

**清单格式.** 目录 + `SKILL.md`,YAML frontmatter:`name`(必须与父文件夹同名)与 `description` 必填;可选 `paths`、`disable-model-invocation`、`metadata`。([Cursor 文档](https://cursor.com/docs/context/skills))

**软链行为(硬指标).** 官方文档仍未提软链,Cursor 闭源也无源码证据;但已在 **Cursor 3.12.17(arm64)** 上用项目级 `.cursor/skills` 夹具完成 UI 实证(ticket [#13](https://github.com/RookieZoe/skill-man/issues/13),详见 §6.5):条目级软链与嵌套链均能出现在 `/` 技能列表并成功调用;`.cursor/skills` 根目录本身为软链时也能加载;dangling 条目静默跳过且不影响控制组;软链条目目录名与 frontmatter `name` 错配时,以 **frontmatter `name`** 注册。该结论是特定版本实测,不是 Cursor 的稳定 API 承诺。

### 2.5 opencode(开源 TypeScript,仓库 anomalyco/opencode)

**目录.** 文档列出六处([skills 文档源 packages/web/src/content/docs/skills.mdx](https://github.com/anomalyco/opencode/blob/67caf894e0843ee370e72839e8265e483233479b/packages/web/src/content/docs/skills.mdx)):

- 项目配置 `.opencode/skills/<name>/SKILL.md`;全局配置 `~/.config/opencode/skills/<name>/SKILL.md`;
- Claude 兼容:项目 `.claude/skills/<name>/SKILL.md`、全局 `~/.claude/skills/<name>/SKILL.md`;
- Agent 兼容:项目 `.agents/skills/<name>/SKILL.md`、全局 `~/.agents/skills/<name>/SKILL.md`。

源码层面另有:`{skill,skills}` 单复数同收([index.ts L24](https://github.com/anomalyco/opencode/blob/67caf894e0843ee370e72839e8265e483233479b/packages/opencode/src/skill/index.ts#L24));config `skills.paths` 自定义目录(模式 `**/SKILL.md`,递归)与 `skills.urls` 远程拉取([index.ts L210-L226](https://github.com/anomalyco/opencode/blob/67caf894e0843ee370e72839e8265e483233479b/packages/opencode/src/skill/index.ts#L210-L226))。项目级外部目录从 cwd 上溯至 git worktree 逐层收集([index.ts L196-L204](https://github.com/anomalyco/opencode/blob/67caf894e0843ee370e72839e8265e483233479b/packages/opencode/src/skill/index.ts#L196-L204))。内置一个 `customize-opencode` skill,磁盘同名 skill 可覆盖它([index.ts L270-L279](https://github.com/anomalyco/opencode/blob/67caf894e0843ee370e72839e8265e483233479b/packages/opencode/src/skill/index.ts#L270-L279))。

**清单格式.** `SKILL.md`,frontmatter 只认 `name`(必填,须匹配目录名,小写连字符正则)/`description`(必填,1-1024 字符)/`license`/`compatibility`/`metadata`,未知字段忽略([skills.mdx](https://github.com/anomalyco/opencode/blob/67caf894e0843ee370e72839e8265e483233479b/packages/web/src/content/docs/skills.mdx))。

**发现机制与冲突.** `Glob.scan` 封装 npm `glob`([packages/core/src/util/glob.ts](https://github.com/anomalyco/opencode/blob/67caf894e0843ee370e72839e8265e483233479b/packages/core/src/util/glob.ts));同名冲突记 warning 后**后者覆盖前者**([index.ts L125-L139](https://github.com/anomalyco/opencode/blob/67caf894e0843ee370e72839e8265e483233479b/packages/opencode/src/skill/index.ts#L125-L139));扫描顺序 external(全局→项目)→ opencode 配置目录 → 自定义 paths/urls,即后扫描的来源优先级更高。可用 runtime flags 关闭外部/Claude skills([index.ts L257-L268](https://github.com/anomalyco/opencode/blob/67caf894e0843ee370e72839e8265e483233479b/packages/opencode/src/skill/index.ts#L257-L268))。

**软链行为(硬指标).**

- 所有扫描调用恒传 `symlink: true`([index.ts L142-L160](https://github.com/anomalyco/opencode/blob/67caf894e0843ee370e72839e8265e483233479b/packages/opencode/src/skill/index.ts#L142-L160)),映射为 npm glob 的 `follow: true`([glob.ts L18](https://github.com/anomalyco/opencode/blob/67caf894e0843ee370e72839e8265e483233479b/packages/core/src/util/glob.ts#L18))——**意图上明确跟随软链**。本地实验(glob 13.0.6)显示其使用的各模式(含 `skills/**/SKILL.md` 中间段为软链的情形)均可发现软链目标,dangling 静默跳过。
- **但存在未修复缺陷**:[issue #18848](https://github.com/anomalyco/opencode/issues/18848)(截至 2026-07-20 open、无关联 PR)报告在 **git worktree sandbox** 中,当 `.claude/skills` 是 git 跟踪的软链(mode 120000)时项目级 skills 不被发现;根因分析为(1) glob 未下钻目录软链,(2) 每个 worktree 会话有独立 skill 状态,主仓扫到的 skills 沙盒会话不可用。
- 本机 `~/.config/opencode/` 存在(含 `opencode.json`、`plugins/`),当前未建 `skills/` 目录 —— 与「目录不存在即跳过」的发现逻辑相容。

### 2.6 未纳入对比的 agent

- **Amp / Windsurf / Aider**:截至 2026-07-20 未找到可引用的一手来源(官方文档/源码)确认其 skills 目录约定(Web 搜索无有效结果),按「严禁猜测」原则不纳入。Aider 历史上只有 conventions 文件机制,无 skills 概念。

## 3. 对「自定义 agent(名字+目录)」机制设计的启示

**建模一个自定义 agent 最少需要的字段:**

1. `name` / 标识(agent 显示名与内部 key)。
2. `skillsDirs`:目录列表,每项含 `scope`(personal / project / builtin / admin)、`path`(支持 `~` 与 per-project 相对路径)、`enabled`。注意多家有**别名双目录**(如 `.gemini/skills` 与 `.agents/skills`)与**多层共存**,单字段单目录不够。
3. `manifestFile`:清单文件名(本调研中全部为 `SKILL.md`)+ frontmatter 约束(`name` 是否必须与目录同名:Cursor、opencode 要求;Claude/Gemini 不强制)。
4. `discovery` 语义:启动扫描 or 文件 watch(影响「改完是否要提示重启」);**扫描深度**(Codex ≤6 层、Gemini 仅 1 层子目录、Cursor 递归、Claude 直接子目录 + 嵌套按需)—— 决定 Skill Man 应把 skill 放在第几层。
5. `symlinkPolicy` 枚举:`follows`(官方支持或实证跟随)/ `not-followed`(确认不跟随)/ `unknown`(未验证),并附**证据等级**(源码/文档/issue/实验/无)与**嵌套链、dangling 行为**两个子项。
6. `installMethod` 能力:`symlink` | `copy` —— 由 5 推导:`unknown` 或 `not-followed` 时只能拷贝安装;实证但无官方承诺(当前 Cursor)可默认软链,同时通过 Activation 健康检查兜底并在适配器中保留版本证据。
7. `conflictRule`:同名覆盖方向(层级优先级/后扫描覆盖/命名空间隔离),用于预测安装后果。
8. `nativeToggle`:agent 原生启用/禁用机制(Codex `config.toml [[skills.config]] enabled`、Gemini `/skills disable`、Claude `skillOverrides`)—— 有原生开关时,Skill Man 的「禁用」应优先写原生配置而非删软链。

**对「app 建删软链来启用/禁用 skill」方案的风险清单:**

- **Claude Code:软链可用但有运维风险。** 条目级软链是官方文档支持的能力且本机实测工作;但 [#50052](https://github.com/anthropics/claude-code/issues/50052) 报告**自动更新会静默删除 `~/.claude/skills/` 下的软链**(未修复确认,closed as not planned)。Skill Man 需要「软链健康检查 + 一键重建」兜底,且**不要**把 `~/.claude/skills` 整目录做成软链([#38051](https://github.com/anthropics/claude-code/issues/38051) 整目录软链回归史),只建条目级软链。
- **Codex:软链友好。** 源码明确 User/Repo/Admin 跟随软链、按 canonicalize 去重、dangling 只告警;可直接用软链方案。注意 `~/.codex/skills/.system` 是 Codex 每次启动自动重装的内建缓存,**不要往里装东西**;用户层应优先装 `~/.agents/skills`(新主位置)。
- **Gemini CLI:软链即官方安装方式。** `/skills link` 本身就建 `fs.symlink`,软链方案最稳妥;但发现**只下钻一层**,软链必须直接放在 skills 根目录下(不要嵌套到子目录里);workspace 层有 trust 门槛。
- **Cursor:3.12.17 实证软链可用。** 项目级条目软链、嵌套链与根目录软链均加载,dangling 静默跳过;因此 Skill Man 可对 Cursor 使用与其他 Agent 一致的条目级 Activation,无需默认拷贝。注意 Cursor 以 frontmatter `name` 注册,软链条目名不能重命名 Skill;且结论无官方承诺,仍需 Activation 健康检查与适配器版本证据兜底。
- **opencode:软链跟随,但 worktree sandbox 场景有未修复缺陷。** 常规场景可软链;若用户项目用 git worktree + 提交到仓库的 `.claude/skills` 软链,需提示 [#18848](https://github.com/anomalyco/opencode/issues/18848) 风险。
- **通用建议:** `~/.agents/skills/` 已被 Codex/Gemini/Cursor/opencode 共同读取,是「一次安装、多 agent 可见」的天然共享层;但 Claude Code **不读** `~/.agents/skills`(需往 `~/.claude/skills` 建条目软链)。四家对该层条目软链均有源码/官方命令/本机实验支持;但为了按 Agent 独立 Enable / Disable,ADR-0005 仍把共享层仅作为 legacy 扫描源,受管 Activation 分别落在各 Agent 私有目录。

## 4. 本机只读观察(2026-07-20,未做任何修改)

- `~/.claude/skills/`:真实目录与软链共存;27 个软链指向 `~/.agents/skills/*`(其中一个为相对软链)。本会话技能列表包含多个软链 skill → Claude Code 当前版本条目级软链工作。
- `~/.codex/skills/`:存在 `.system/`(内建:`skill-creator`、`imagegen`、`plugin-creator`、`openai-docs`、`skill-installer` + `.codex-system-skills.marker`)、真实目录(`codex-primary-runtime/`、`kimi-webbridge/`)与 7 个指向 `~/.agents/skills/*` 的软链。
- `~/.agents/skills/`:跨 agent 共享层,真实目录与指向各代码仓库的软链共存。
- `~/.gemini/`:不存在(未安装 Gemini CLI)。
- `~/.config/opencode/`:存在(`opencode.json`、`plugins/` 等),尚无 `skills/` 目录。

## 5. 信息时效与未验证项

**信息时效.** 全部内容截至 2026-07-20。源码证据取自当日各仓库 `main` HEAD:openai/codex `3dd3c5d`,google-gemini/gemini-cli `acae712`,anomalyco/opencode `67caf89`。Claude Code 文档引用 code.claude.com 当日版本(文中特性标注至 v2.1.20x)。npm `glob` 实验版本 13.0.6(Node v22.20.0,macOS)。

**未验证/存疑清单(2026-07-20 晚更新:第 1、2、4、6 项已由 §6 实证覆盖,第 3 项产出人类验证清单):**

1. ~~Claude Code 嵌套软链链与 dangling 软链行为~~ → **已实证(§6.2)**:嵌套链跟随;dangling 静默跳过。
2. **Claude Code #38051(整目录软链回归)与 #50052(自动更新删软链)的最终修复状态** —— 项目级整目录软链已实证**可加载**(§6.2,#38051 形状在项目级不复现);**用户级 `~/.claude/skills` 整目录软链未实测**(实验约束:不动用户真实目录);#50052 自动更新删软链只能长时间观察,仍开。
3. ~~Cursor 的一切软链行为~~ → **已由人类实证(§6.5,ticket #13)**:Cursor 3.12.17 的项目级条目软链、嵌套链和 skills 根目录软链均加载;dangling 静默跳过;名称错配时按 frontmatter `name` 注册。用户级目录未单独实测,且行为没有官方承诺。
4. ~~Codex 嵌套软链链的精确行为、dangling 的 warning 文案~~ → **已实证(§6.3)**:嵌套链跟随;**dangling 静默跳过,headless 下未观察到 warning**(与源码推断的 warning 不一致,或仅 TUI 展示)。
5. **Gemini CLI 的 dangling/嵌套软链结论来自本地 glob 13.0.6 实验** —— gemini-cli 实际依赖的 glob 版本未逐一核对(其 `package.json` 未在本调研中锁定),实验结论非官方承诺;`/skills link` 建软链这一事实为源码证据。
6. ~~opencode 在 Bun 运行时下 npm glob 的实际行为~~ → **已实证(§6.4)**:本机 opencode 1.18.3 条目软链/嵌套链/根目录软链均跟随;**#18848 的 worktree 场景在该版本手工复现未命中**(issue 所述 opencode 自建沙盒会话隔离场景未单独验证)。
7. **各 agent 同名冲突的完整优先级矩阵**(如 Claude Code enterprise 层细节、Codex 多 root 同名展示规则)—— 只验证了主要规则。
8. **Amp / Windsurf / Aider 的 skills 机制** —— 无可靠一手来源,未纳入。

## 6. 实证实验记录(2026-07-20,验证 ticket [#11](https://github.com/RookieZoe/skill-man/issues/11))

> 对第 5 节未验证清单的第 1、2、4、6 项做实证补齐。全部实验在 `/tmp` 临时目录进行,**未改动** `~/.claude/skills`、`~/.codex`。实验脚本与原始输出当日存于 `/tmp/wf-symlink-exp/`(临时目录,重启即失;本节为结论性记录)。

**环境.** macOS(Darwin 25.5.0);Claude Code **2.1.215**;codex-cli **0.136.0-alpha.2**(取自 `/Applications/Codex.app/Contents/Resources/codex`);opencode **1.18.3**(homebrew)。Cursor 未安装(本机无 .app / CLI)。

**观测方法.** Claude Code:在临时项目 `.claude/skills` 下布置夹具,`claude -p "/<skill>"` 直接调用,skill 内容为「只回复唯一 token」,以 token/「Unknown command」判定加载与否。Codex:临时 `CODEX_HOME` + `codex debug prompt-input`,读模型可见 `<skills_instructions>` 的 Available skills 清单。opencode:`opencode debug skill` 列全部可用 skill(name + location),配 `--print-logs` 看 WARN。

### 6.1 结果总表

| Agent | 条目软链 | 嵌套链(link→link→real) | 相对软链 | dangling | skills 根目录整目录软链 | 同目标去重规则 |
|---|---|---|---|---|---|---|
| Claude Code 2.1.215 | ✅ 加载 | ✅ 加载(中间环在 skills 根内外均可) | ✅ 加载 | 静默跳过;调用报 Unknown command;不影响其他条目 | ✅ 加载(**项目级**;用户级未测) | 按 canonical 目标去重,**字典序靠前的条目名保留**(2/2 观察),其余报 Unknown command |
| Codex 0.136.0-alpha.2 | ✅ 跟随 | ✅ 跟随 | (未单测) | 静默跳过;headless 未见任何 warning(stderr 0 行) | (未单测) | 按 canonical 目标去重;**展示名取自 frontmatter `name`,条目(软链)名无关紧要**;清单中路径显示为解析后真实路径 |
| opencode 1.18.3 | ✅ 跟随(外部 `.claude/skills` 与原生 `.opencode/skills` 均) | ✅ 跟随(内容被读取,撞名 WARN 佐证) | (未单测) | 静默跳过 | ✅ 跟随(主仓与手工 git worktree 均正常,**#18848 不复现**) | 按 **frontmatter `name`** 去重,后扫描者覆盖,WARN `duplicate skill name`;**name≠目录名被静默容忍** |
| Cursor 3.12.17 | ✅ 加载 | ✅ 加载 | (未单测) | 静默跳过;不影响其他条目 | ✅ 加载(项目级) | 名称错配时以 **frontmatter `name`** 注册;跨目录同名赢家未测 |

### 6.2 Claude Code 详录(项目级,临时目录)

夹具:临时项目 `proj/.claude/skills/` 下 —— `wf-real`(真实目录,控制组)、`wf-entry`→树外真实目录、`wf-chain`→`wf-entry`(嵌套链)、`wf-dangling`→不存在目标;另设 `proj2/.claude/skills` 本身为软链(内含 `wf-rooted`);`proj3` 复测干净目标与去重对(`wf-aaa`/`wf-zzz` 同指一个目标)、相对软链 `wf-rel`。

- **A1 控制组**:`/wf-real` → 返回 token,加载正常;同目录存在 dangling 条目不碍事。
- **A2/A3(撞上去重的意外发现)**:`/wf-entry` → `Unknown command`,`/wf-chain` → 返回目标 token。同目标两个条目只保留一个 —— 官方文档原话「if the same target is reachable from more than one location, Claude Code loads the skill once」在此命中。
- **C3 去重方向复测**:`wf-aaa`/`wf-zzz` 同指一个目标 → 仅 `wf-aaa` 可用。两次观察(另一次 wf-chain 胜 wf-entry)均为**字典序靠前者保留**(小样本经验法则,非官方承诺)。被去重的条目调用表现为 `Unknown command`,与「不存在」无法区分。
- **C1/C2 干净复测**:条目软链(独占目标)、嵌套链(中间环在 skills 根之外,`wf-chain2`→`mid-link`→目标)均加载 ✅。
- **C4 相对软链**:加载 ✅。
- **B1 dangling**:`/wf-dangling` → `Unknown command`,无报错、无崩溃,扫描不中断(同项目其他 skill 正常)。
- **B2 skills 根目录整目录软链(项目级)**:`proj2/.claude/skills` → 真实目录,`/wf-rooted` 正常加载 —— **#38051 的形状在项目级于 2.1.215 不复现**。注意:#38051 原报是**用户级** `~/.claude/skills`;用户级受实验约束未测,Skill Man 仍不应把任何 agent 的 skills 根目录做成软链(条目级足矣,且规避回归史)。

**对 Skill Man 的含义(更新 §3)**:
- Activation 建条目级软链即可,嵌套链/相对软链都能被跟随 —— 但按 ADR-0001 直指实体,不主动造链。
- **去重规则是新风险**:若同一目标经两个条目名可达(如 Skill Man 的 Activation 与既有 Untracked 软链同指一源),保留的未必是 Skill Man 建的那个名 —— **Conflict 检测要按 canonical 目标判重,不能只看条目名**;且被去重者「Unknown command」,用户感知为「启用失败」。
- dangling 无害但静默:Broken 的 Activation 在 Claude Code 里只是「叫不出来」,不会报错 —— 健康检查要靠 Skill Man 自检,不能指望 agent 提醒。

### 6.3 Codex 详录(临时 CODEX_HOME,headless)

夹具:`$CODEX_HOME/skills/` 下 `wf-real`(控制)、`wf-entry`→树外目标、`wf-chain`→`wf-entry`、`wf-dangling`;补充 `wf-chain2`→中间环→独占目标;去重对 `wf-aaa`/`wf-zzz` 同指目标(其 frontmatter name=wf-dup)。

- **条目软链:跟随。** Available skills 列出 `wf-entry`,路径显示为**解析后真实路径**(`holding/wf-entry/SKILL.md`)。
- **嵌套链:跟随。** 首轮 `wf-chain` 未列出,但与 `wf-entry` 同目标 —— 补独占目标的 `wf-chain2` 后正常列出,确认缺席原因是**去重**而非不跟随链。
- **去重:按 canonical 目标路径,与源码一致;展示名取自 frontmatter `name`。** 去重对只产出一个 skill,名字是 `wf-dup`(目标 frontmatter),条目名 `wf-aaa`/`wf-zzz` 均不出现于清单 —— **软链条目名对 Codex 的技能身份无影响**。
- **dangling:静默跳过。** `RUST_LOG=debug` 下 stderr 0 行,无 warning 落盘 —— 与源码推断(遍历错误收集为 warning)在 headless 观测面上不一致;warning 可能仅 TUI 会话内展示。**对 Skill Man:Broken Activation 在 Codex 同样无感知,需自检。**
- 附带验证:全新 `CODEX_HOME` 首跑自动安装 `.system` 内建 skills,与源码/§2.2 一致;`~/.agents/skills` 用户层照常扫描(本机存量 skill 全部列出,软链条目显示解析后路径)。

### 6.4 opencode 详录(1.18.3)

夹具:`proj/.claude/skills/`(外部目录)控制/条目/嵌套链/dangling/**name≠目录名**错配;`proj2/.opencode/skills/`(原生目录)控制/条目;git 仓库提交 `.claude/skills` 为软链(mode 120000)并 `git worktree add` 出独立工作树。

- **条目软链:跟随**(外部与原生目录均),location 显示为**未解析的条目路径**。
- **嵌套链:跟随。** `wf-chain` 内容被读取 —— WARN `duplicate skill name ... existing=.../wf-chain/SKILL.md duplicate=.../wf-entry/SKILL.md` 直接佐证;撞名后「后扫描者覆盖」,`wf-entry` 保留。
- **去重按 frontmatter `name`**:同 name 即 WARN + 覆盖,与路径无关 —— 本机全局环境实测大量此类 WARN(`~/.claude/skills` 与 `~/.agents/skills` 同指造成的「同名不同路径」),同时佐证**全局层软链也被跟随**。
- **name≠目录名:静默容忍。** `wf-mismatch`(条目名)→ 目标(frontmatter name=real-other)以 `real-other` 注册,无警告 —— 文档「name 须匹配目录名」在 1.18.3 加载期未强制执行。**对 Skill Man:以软链条目名重命名 skill(如 Adopt 改名)对 opencode 无效 —— 它以 frontmatter name 为准。**
- **dangling:静默跳过**,无 WARN。
- **skills 根目录软链 + git worktree(#18848 场景):不复现。** 主仓与 `git worktree` 中 `wf-wt` 均正常发现(location 在各自检出路径下)。#18848 所述「opencode 自建沙盒 worktree 会话」变体未单独验证 —— 保守做法:对依赖该场景的用户仍提示该 issue 未关闭,但常规 worktree 使用在 1.18.3 已无障碍。

### 6.5 Cursor 详录(3.12.17,人类 UI 实证,ticket [#13](https://github.com/RookieZoe/skill-man/issues/13))

环境:Cursor **3.12.17**,commit `0fb762053c34788bb7760d5673f8a6d4c8589d50`,arm64。夹具全部位于 `/tmp/cursor-skill-exp/`,使用项目级 `.cursor/skills`,未改动用户目录。观测方式:用 Cursor 打开临时项目,在 Agent 聊天输入 `/` 检查技能列表,再显式调用并确认唯一 token。

- **控制组:**真实目录 `wf-control` 正常出现在列表并返回 `CURSOR_CONTROL_OK`。
- **条目级软链:**`wf-entry` → 树外真实目录,正常出现在列表并返回 `CURSOR_ENTRY_OK`。结论:Cursor 跟随项目级条目软链。
- **嵌套链:**`wf-chain` → `mid-chain` → 树外真实目录,正常出现并返回 `CURSOR_CHAIN_OK`。结论:Cursor 跟随 `link → link → real`。
- **名称错配:**软链条目名 `wf-visible`,目标 frontmatter `name: wf-target`。列表显示 **`wf-target`**,调用返回 `CURSOR_MISMATCH_OK`;`wf-visible` 不作为技能名。结论:Cursor 的注册身份取 frontmatter `name`,而非软链条目目录名。
- **dangling:**`wf-dangling` 指向不存在目标,列表中不出现、无可见报错,控制组仍正常。结论:dangling 静默跳过且不影响其他 Skill。
- **skills 根目录软链:**另一个临时项目的 `.cursor/skills` 整体指向树外 `root-skills`,其中 `wf-rooted` 正常出现并返回 `CURSOR_ROOT_OK`。结论:Cursor 3.12.17 跟随项目级 skills 根目录软链。

**边界:**本实验未改 `~/.cursor/skills` 或 `~/.agents/skills`,因此用户级行为未单独验证;未测试相对软链、循环链及跨作用域同名优先级。Cursor 闭源且官方文档未承诺软链行为,升级后仍应靠 Skill Man 的 Activation 健康检查发现回归。

**对 Skill Man 的含义:**Cursor 适配器可由“默认拷贝”改为**条目级 Activation 软链**;名称校验必须同时检查目录名与 frontmatter `name`,不能试图仅靠软链条目名重命名 Skill;Broken Activation 仍需 Skill Man 自检,Cursor 不会主动报错。

### 6.6 对 §3 风险清单的修订点

- **Claude Code 去重按目标、赢家按名字典序** → §3「软链健康检查」之外,Conflict 检测必须按 canonical 目标判重。
- **Codex 身份=canonical 路径 + frontmatter name**;**opencode/Cursor 身份=frontmatter name**;**Claude Code 身份=条目目录名** —— 各家「skill 身份」语义不同,CONTEXT.md 的「身份=目录名」在映射到 agent 时需按本表翻译,并对 Cursor/opencode 的 name 错配给兼容性警告。
- **Cursor 软链风险下调**:3.12.17 实证条目/嵌套/根目录软链可用,可由默认拷贝改为条目级 Activation;因无官方承诺,保留版本证据与健康检查。
- **opencode worktree 风险下调**:1.18.3 常规 worktree 可用,仅沙盒会话变体存疑。
- **dangling 四家(Claude Code/Codex/opencode/Cursor)全部静默** → Broken 检测与「一键重建」只能由 Skill Man 自检,无 agent 侧信号可依赖。
