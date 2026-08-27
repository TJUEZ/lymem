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
