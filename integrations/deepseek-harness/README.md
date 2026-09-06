# lymem × DeepSeek Harness（DSH）

把 lymem（麟忆尽智）作为 deepseek-harness 的记忆模块插件。

## 架构

```
DSH Agent ──(MCP Streamable HTTP)──> lymem-server :8801/mcp
                                        │
                          ┌─────────────┼──────────────┐
                    混合检索(向量+BM25+图)  偏好版本链   冲突仲裁/遗忘
                                        │
                          麒麟端侧嵌入（DBus/kytensor 自愈双通道）
```

## 启用步骤

1. 启动 lymem-server（本插件为 HTTP 形态，要求服务已运行）：

   ```bash
   cd lymem && cargo run -p lymem-server            # 默认 127.0.0.1:8801
   # 如需 LLM 增强（偏好慢通道/冲突仲裁/遗忘解析）：
   export LYMEM_LLM_API_KEY=...   # OpenAI 或 Anthropic 兼容（MiniMax 预设）
   ```

2. 以插件补丁方式启动 DSH：

   ```bash
   dsh web --patch /path/to/lymem.cordis.yml
   ```

3. 验证：agent 中模型应能调用 `mcp__lymem__memory_search` 等工具。

## Agent 侧推荐工作流（写入 AGENTS.md）

```
- 每轮任务开始前：mcp__lymem__memory_search 检索相关记忆与偏好
- 回复风格遵循 mcp__lymem__preference_list 的当前偏好
- 会话中的关键事实/工具结果/用户纠错：mcp__lymem__memory_add 写回
- 用户要求删除信息：mcp__lymem__memory_forget（先 preview 再执行）
```

## 故障排查

- `tools/list` 为空 → 确认 `curl -X POST http://127.0.0.1:8801/mcp -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'` 有响应
- 嵌入相关错误 → 麒麟 runtime 引擎偶发退化，lymem 会自动重试并降级 kytensor 直连；仍失败时重启 kylin-ai-runtime
- 持久化：把本文件的 insert 合并进 `$DSH_HOME/cordis.patch.yml`


## 功能演示动线（前端 `:8801/viewer`）

DSH 插件与 lymem viewer 各页对应，五分钟走完核心能力：

| viewer 页面 | DSH 侧操作 / 看什么 |
|---|---|
| 总览 | 首次体验先点「灌入演示数据」（隔离场景，一键可清）；看 KPI、记忆树与层级计数 |
| 记忆库 | 多源条目、清洗分级与来源（工具结果/行为/配置/会话，含 Agent 中枢汇聚导入） |
| 偏好中心 | 对 Agent 说"以后都用 PDF 导出"，Agent 调 `mcp__lymem__memory_add`，此页实时出现偏好版本链 |
| 知识与冲突 | 让 Agent 记一条工作流/模板；入库即冲突检测仲裁，台账可视；底部「经验复用」卡一键编译经验卡 |
| 遗忘与安全 | 对 Agent 说"忘掉 xx"，`mcp__lymem__memory_forget` 预览后执行；敏感扫描可视 |
| Agent 中枢 | 源发现 / 汇聚导入 / 记忆调度 / 总索引图 / 总管巡检 |
| 评测对比 | 一键评测内置样例或粘贴自有 JSONL，出 hit@5 与延迟分位数 |

演示顺序建议：总览灌入演示数据 → 偏好中心说一句偏好 → 知识与冲突记工作流并编译经验卡 →
记忆库看来源与质量 → 遗忘与安全做一次精准遗忘 → Agent 中枢跑一次巡检 → 评测对比出分。
