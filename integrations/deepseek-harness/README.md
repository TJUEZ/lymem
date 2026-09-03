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


## 赛题功能演示动线（前端 `:8801/viewer#sq`）

DSH 插件与 lymem viewer 的「赛题演示」页一一对应，覆盖赛题五项功能要求：

| 前端模块 | DSH 侧操作 | 对应条款 |
|---|---|---|
| ① 多源接入与整合 | 在 DSH 中让 Agent 执行工具调用 / 上传文档导入 | 1（清洗/标准化/质量校验在「质量报告」可视） |
| ② 偏好记忆动态捕捉 | 对话中说"以后都用 PDF 导出"，Agent 调 `mcp__lymem__memory_add` | 2（偏好库实时呈现，版本回溯） |
| ③ 知识记忆提取与整合 | Agent 主动询问工作流/模板并写入；或页面直接粘贴提取 | 3（入库即冲突检测仲裁，台账可视） |
| ④ 记忆流转 | 会话结束在页面点「执行一轮蒸馏」 | 6（工作→情景→知识管道计数实时） |
| ⑤ 记忆竞技场 | 一键评测内置样例 / 粘贴用户自有 JSONL 数据 | 7（hit@5 + 延迟分位数 + 五站×四系统矩阵） |

演示顺序建议：② 说一句偏好 → ③ 让 Agent 记一条工作流 → ① 导入一份文档看质量报告 →
④ 蒸馏看层级计数变化 → ⑤ 跑一次竞技场出分。
