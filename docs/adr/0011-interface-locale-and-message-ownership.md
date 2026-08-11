# 界面 locale、内容与消息所有权

Skill Man 的 App Copy 支持 `en` 与 `zh-Hans`,首次启动默认跟随系统首选语言,同时在首次绑定流程的 App shell 和 Preferences 提供独立的 `System / English / 简体中文` 选择;该控件不属于 Home candidate、Home Binding 或只读 Home path 表单。持久值为 `system | en | zh-Hans`:它属于 Home 外的 App 级状态,因此在 Home 尚未绑定、HomeUnavailable、Catalog ReadOnly 或 Fixture Recovery Lock 期间仍可读写,该写入也不属于 Home 产品写操作;locale 选择不随 Abandon 或 Restore 改变。Preferences 保留既有四个 boolean 开关,另加一个非 switch 的 Language 选择。

native composition root 中唯一的 App-level locale authority 负责持久化选择、读取系统首选语言、解析有效 locale 并发布 typed snapshot/event;React、tray 与原生菜单只消费该状态,不得各自协商。没有持久值时按 `system` 处理;值损坏、未知或 store 不可读时使用 `en` 安全基线并记录 diagnostic。用户切换采用 persist-then-publish:持久化失败时保持此前选择与有效 locale snapshot,React、native surface 和 document `lang` 均不改变,并返回结构化、可本地化的错误,不得只在当前 session 静默生效。有效 locale 必须在首个 React 或 native 可见 surface 创建前确定,首个可见帧不得先显示错误 locale 或混合语言;Web document 的 `lang` 始终同步为 `en` 或 `zh-Hans`。

有效 locale 的优先级为显式选择、系统语言协商、English fallback。系统模式按首选语言列表依次匹配:`en` 及其变体映射到 `en`;`zh`、`zh-Hans` 及其变体、`zh-CN`、`zh-SG` 映射到 `zh-Hans`;`zh-Hant` 及其变体和 `zh-TW`、`zh-HK`、`zh-MO` 当前不映射为简体中文,而是继续匹配后续首选语言,最终无匹配才回退 `en`。选择 System、App 再次激活及下次启动时重新协商;手动切换立即更新 React、Web document 与 native surface,不得重载 Catalog、重挂当前 sheet 或丢失筛选、选择、表单和焦点状态。

内容所有权遵循 [CONTEXT.md](../../CONTEXT.md):所有 App Copy 都必须本地化,Source Content 一律原样展示。React UI、ARIA、placeholder 中的自然语言、日期、数字、单位、tray、原生菜单、通知和更新流程使用同一有效 locale;Skill 名称、frontmatter、`SKILL.md`、路径、URL、Git 标识、用户自定义 Agent 名称、release notes、外部命令输出及 placeholder 中的 technical token 不进入翻译目录。外部错误以本地化摘要和操作建议呈现,必要时另行显示明确标注、可展开的原始技术详情;客户端不对 release notes 做运行时机器翻译。

Core 和业务操作只产生稳定语义,不得接收 locale 或拼接用户文案。Tauri command DTO 使用稳定 closed error code、有类型的结构化参数及可选 diagnostic;路径、Skill 或 Agent 名称等 Source Content 只作为原样参数。React 与 native presentation 从同一消息 key 集合按 locale authority 发布的有效 locale 呈现。现有自由文本字段必须先按所有权拆分:App 语义不得以自由 String 穿越 DTO,而应变成 closed code/key 加 typed params;Source Content 保留为独立原始字段;`sourceLabel` 拆为 source kind 与原样 source fields 后由 presentation 组合。这条边界同时适用于顶层错误以及成功 DTO 中的 `warning`、`reason`、`sourceLabel` 等混合字段。

## Consequences

English 是完整基线,`zh-Hans` 必须保持相同 key、插值参数和复数参数集合;缺 key 或参数不匹配阻断 CI,运行时仅以 English 同 key fallback 作为防止空白界面的安全网。静态门禁阻止 App Copy 散落在消息目录之外,只允许明确维护的品牌、technical token 与 Source Content allowlist。行为测试使用 role 和稳定语义标识,另以双 locale 契约验证可见文案、accessible name、格式化、Source Content 逐字不变及运行时切换不丢状态;resolver 矩阵覆盖首选语言顺序、Hant 跳过、后续语言命中、无效持久值及最终 English fallback,每个 closed error code 都有双语消息并为未知 code 提供安全 fallback,首帧和运行时切换均断言 document `lang`。tray 与原生菜单也进入同一覆盖门禁。具体消息库、资源文件格式、App 级存储介质、React API 和 native locale adapter 属于后续实现 spec。

本 ADR 取代 ADR-0007 与现行 MVP implementation spec 中“Preferences 严格只有四项”及“语言不提供设置”的结论,并取代 ADR-0009 对“严格四项”的继承;四个既有开关及其行为保持不变,最终 vNext spec 汇总必须删除相反的旧表述。Home 的首次绑定和不可用语义仍由对应 Home 决策负责,本 ADR 只要求 locale 状态独立于 Home。
