# Activation 直指最终实体,不做指针串联

Skill Man 的 Library 是 Catalog 索引与受管实体组成的逻辑边界。Install 来源的 Skill 实体在 Home 内，Link 来源的实体留在源目录且只由 Catalog 记录 canonical 最终实体路径；Home 不为 Link 再建一条 Library 指针软链。Enable 一个 Skill 时，Agent Activation Target 中的 Activation **直指 Catalog 解析出的最终实体**——Git Source Member 指向 [ADR-0018](0018-git-source-namespaces-and-immutable-members.md) 的 `<Home>/skills/git/<remote_id>/<skill_id>/`，其它 Install 指向各自 Home 实体，Link 直接指向 Local Source——不经过任何中间指针。

理由：指针不串联使 Broken 检测只需检查一跳，也避免 Catalog、冗余 Library 软链与 Activation 形成多个可分叉的 pointer authority；这与维护者机器上原有的两级链式布局(`~/.claude/skills` → `~/.agents/skills` → 源仓库)明确切割。
