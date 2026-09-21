# 技能编译与 Agent 接入

lymem 对 Agent 的暴露分成两条互补路径：MCP 提供实时记忆读写，技能目录提供低成本、可迁移的过程性知识。

## 1. 编译链路

```text
原始轨迹（不可变）
  → 情景记忆（窗口/会话压缩）
  → 经验卡（做法 / 避坑，含证据链和复用门控）
  → SKILL.md（Agent 可直接加载）
```

成功工具轨迹会提炼为「做法」，失败轨迹会提炼为「避坑」。经验卡默认是 `draft`；被 Agent 复用成功至少 2 次且没有失败反馈后变为 `verified`。一次失败会回退为 `draft`，不会抹掉原始证据或版本历史。

## 2. Web 发布

启动服务后进入 `http://127.0.0.1:8801/viewer`，打开「知识与冲突」页：

1. 在「经验复用」中点击「从近期轨迹编译经验」；
2. 选择目标 Agent，点击「发布技能」；
3. 发布动作会再次执行一次幂等编译，然后写入 `SKILL.md` 和 `resources/manifest.json`；
4. 发布前自动把旧版保存到 `.lymem-versions/`，可以回滚或移除。

## 3. CLI 发布

```bash
lymem experience compile
lymem experience list
lymem skill targets
lymem skill publish codex
lymem skill rollback codex
lymem skill remove codex
```

目标目录由当前桌面用户的 `HOME` 决定：

| 目标 | 默认技能目录 |
|---|---|
| Codex | `~/.codex/skills/lymem-experience/` |
| Claude Code | `~/.claude/skills/lymem-experience/` |
| OpenCode | `~/.config/opencode/skills/lymem-experience/` |
| DeepSeek Harness | `~/.agents/skills/lymem-experience/` |

不会写入系统目录；技能文件由当前用户管理。发布后重新启动对应 Agent，按其技能发现机制加载该目录。

## 4. MCP 实时接入

服务端点为 `http://127.0.0.1:8801/mcp`，支持 `initialize`、`tools/list` 和 `tools/call`。除记忆检索、写入、遗忘、偏好和冲突工具外，还提供：

- `experience_compile`：从情景轨迹编译经验卡；
- `skill_targets`：列出目标 Agent 技能目录；
- `skill_publish`：编译并发布技能；
- `memory_feedback`：反馈经验复用成败。

Agent 推荐顺序：任务开始调用 `memory_search` 和 `preference_list`；执行中把关键工具结果写入 `memory_add`；任务结束调用 `memory_feedback`，必要时调用 `experience_compile` 或 `skill_publish`。

## 5. 版本与安全边界

- 技能发布只允许服务内置的四个目标，不接受任意用户传入路径；
- `SKILL.md` 和清单采用临时文件写入后原子替换，避免 Agent 读到半个文件；
- 旧版本只保留在目标技能目录的 `.lymem-versions/`，回滚不会改动 lymem 数据库；
- 经验卡正文中的来源 ID 用于追溯，实时事实与技能冲突时以实时事实为准，并通过 MCP 写回新记忆；
- 所有编译、反馈、发布、回滚和移除动作都会进入审计日志。

## 6. DSH 严格后端对照

使用 DeepSeek Harness headless、DeepSeek-V4-Flash 和同一 ToolBench 工具选择任务集进行严格 A/B：两臂提示词完全一致，只在实验臂挂载 lymem MCP。2026 年 9 月 13 日的 30 题结果为：裸 Agent EM **90.0%**，挂载 lymem 后 **93.3%**；金标召回 **96.1% → 98.3%**，F1 **95.6% → 97.8%**，p50 时延 **7.77s → 13.13s**。

逐题 JSON、原始输出和复现命令见相邻提交目录的 `benchmark/results/AGENT_EVAL_DEEPSEEK.md`。该轮在容器中使用 sqlite-vec 开发降级；银河麒麟正式验收应切换 `LYMEM_VECTOR_BACKEND=kylin` 后按同一协议复测。
