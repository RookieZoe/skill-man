# Activation 直指最终实体,不做指针串联

Skill Man 的 Library 是「统一目录树 + 索引」:Install 来源的 skill 实体在目录树内,Link 来源的实体留在源目录、Library 里放指针条目。我们决定:Enable 一个 skill 时,agent 目录里的 Activation 符号链接**直指最终实体** —— Install 来源的指向 Library 目录树内,Link 来源的直接指向源目录 —— 而不是指向 Library 的指针条目再间接一跳。

理由:指针不串联使任何一环的 Broken 检测都只需检查一跳;与维护者机器上原有的两级链式布局(`~/.claude/skills` → `~/.agents/skills` → 源仓库)明确切割,避免"Library 条目"与"Activation"两个概念在文件系统层面纠缠。
