# #109 菜单栏搜索与阅读验证

规格：[实施 #109](https://github.com/RookieZoe/skill-man/issues/109)，[父规格 #108](https://github.com/RookieZoe/skill-man/issues/108)。

## 自动验证

- `TrayPanel.test.tsx`：通过 typed CatalogClient 验证全库搜索、Unicode 规范化、同名稳定排序和来源区分、真实 Markdown 复制及失败反馈、输入法 Enter、Esc、两种视图下的 Command K、返回时保留查询／选择／滚动、重新挂载时重置会话、Home／Catalog／locale 迟到响应及 Skill 移除。打开不调用 Rescan 或更新检查。
- `catalog_queries.rs`：公开 CatalogApi 配合真实临时 Home、存储和 SKILL.md，区分真实正文、空文件、文件缺失和 Broken Skill 的正文不可读。`documentAvailable` 保留既有主窗口详情语义，不再要求新面板猜测空字符串的含义。
- `LocaleProvider.test.tsx`：复用已有 locale 行为覆盖；所有快照入口遵循同一 generation 顺序。
- 本地 `npm run ci:local` 包含格式、能力、本地化、lint、类型、前后端测试及 Apple Silicon 原生构建。磁盘身份测试需能访问 macOS DiskManagement，不能在拒绝该框架的沙箱中判定产品失败。
- code-review 的 Standards 与 Spec 两轴已复查修正：结构化窗口错误、列表态 Command K、locale 竞态。

## 原生验收状态：待完成

已启动本轮 release 二进制组成的独立测试包并读取真实 Library。自动化无法可靠访问 macOS 菜单栏状态项；主窗口的读取或截图不算菜单栏验收。因此以下项目未标记通过，也不据此关闭 #109：

1. 关闭主窗口后，点击图标打开 370 CSS px 面板，直接输入；主窗口持续隐藏。
2. 搜索真实 Skill，Enter 阅读，滚动长正文并复制；Esc 返回，查询、选择与列表位置保持。
3. 再次点击图标、外部点击、列表 Esc 均收起；重新打开空查询且位于顶部。
4. Tab、上下键、详情和列表中的 Command K，以及中文输入法候选确认。
5. Dock 显示和隐藏两种偏好各完成上述流程，完成后恢复原偏好。
6. 有限屏幕空间内关键入口仍可见；列表与阅读同宽。
7. 打开主窗口和 Quit 入口真实工作；en／zh-Hans 首帧及运行时切换正确。

界面采用已确认 A 的紧凑系统字体和现有色彩变量。生产入口读取真实 Catalog，不展示固定项或健康模拟数据；#110、#111 的持久化与异常摘要不在本票内。
