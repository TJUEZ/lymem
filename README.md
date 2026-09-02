# 麟忆尽智 lymem

> 面向银河麒麟桌面 OS Agent 的多源融合偏好与知识记忆优化方案（Rust 实现）
>
> 参赛选题：XA-202612「OS Agent 记忆优化及高效应用研究」

## 定位

对标 memmy-agent 的**记忆管理模块**，以纯 Rust 实现端侧轻量化部署，深度适配银河麒麟 AI 端侧能力（文本嵌入 SDK / kytensor 推理后端 / 向量引擎）。

## 记忆中枢（多 Agent 记忆汇聚 · 调度 · 总管）

借鉴 Wake 的「终端多 agent 记忆汇聚」与 Semantica 的「离线索引图规则」，`lymem-core/src/hub.rs` 提供：

- **源发现**：零配置扫描本机终端 agent 记忆载体（zcode / claude-code / codex（含记忆 SQLite）/ gemini / opencode / dsh / 通用 AGENTS.md·CLAUDE.md，`LYMEM_HUB_EXTRA` 可注册自定义源）；
- **汇聚导入**：解析为离散条目 → 清洗/敏感分级/实体抽取 → 入库，`hub_imports` 映射表支持增量重导（幂等）；
- **记忆调度**：把 lymem 记忆/偏好/实体摘要分发到任意 agent 的记忆/规则文件——只写入 `<!-- lymem:begin/end -->` 受管标记块，不触碰用户手写内容，敏感自动脱敏，全程审计；
- **全局总索引**：全库纯规则离线构建（实体抽取 → 共现建边权重累计 → 标签传播社区检测 → 跨源实体统计），数千条记忆毫秒级；
- **跨源矛盾**：按实体聚合跨 agent 记忆，数值矛盾成对检测 + LLM 复核，仲裁（保留新/保留旧/共存）落版本链与冲突台账；
- **总管巡检**：重复指纹 / 失活记忆 / 墓碑积压 / 来源体量，一键执行维护计划。

前端 `/viewer` →「多 Agent 中枢」页；REST：`/api/v1/hub/sources|import|dispatch|index|index/build|index/graph|conflicts/scan|conflicts/resolve|maintenance`。场景演示页第 ④ 场景一键串演全链路。

## 架构

```
┌─────────────────────────────────────────────────────────────┐
│                     接入层（lymem-server / lymem-cli）        │
│   REST API · MCP(规划) · /v1/embeddings 代理 · Web 管理界面  │
├─────────────────────────────────────────────────────────────┤
│                       lymem-core 记忆核心                    │
│                                                              │
│  四层记忆体系          管道                    检索            │
│  ┌──────────┐   多源接入 ingest      三路混合检索            │
│  │ 工作记忆  │──flush──┐  (清洗/去重    ┌ 向量 ANN (sqlite-vec)│
│  │ (短期)    │         ▼   /敏感分级)   │ BM25    (tantivy)   │
│  │ 情景记忆  │──consolidate──┐         │ 图扩展  (实体/边)    │
│  │ (中期)    │               ▼         └──RRF 融合→偏好重排   │
│  │ 知识记忆  │◄──── 蒸馏 ──┘ 冲突管道                          │
│  │ (长期)    │        (检测→分类→仲裁→版本化)                  │
│  │ 偏好记忆  │   偏好双通道(规则+LLM) + 版本时间线             │
│  └──────────┘   遗忘(墓碑/硬清除) + 审计                       │
├─────────────────────────────────────────────────────────────┤
│  lymem-kylin 麒麟端侧绑定       lymem-llm (OpenAI 兼容)      │
│  DBus SDK 通道 + kytensor 直连   MiniMax 预设，未配置则规则兜底│
├─────────────────────────────────────────────────────────────┤
│        SQLite (WAL) + sqlite-vec + tantivy 单机存储          │
└─────────────────────────────────────────────────────────────┘
```

## 赛题条款映射

| 赛题条款 | 实现 | 指标 |
|---|---|---|
| (1) 多源数据整合 | `ingest`：工具结果/行为/配置/会话统一接入，清洗+去重+敏感分级 | — |
| (2) 偏好动态捕捉 | `preference`：规则快通道 + LLM 慢通道 + 版本链 + 跨场景适配 | 提取准确率 ≥85% |
| (3) 知识结构化整合 | `conflict` 四步管道 + `retrieval` 三路混合检索 | 冲突正确率 ≥88%，召回率 ≥85% |
| (4) 端侧部署 | 麒麟嵌入 SDK 直连 + SQLite 单文件存储 | 检索响应 ≤500ms |
| (5) 敏感与遗忘 | `sensitive` 规则识别 + `forget` 自然语言遗忘（软删/硬清除） | — |
| (6) 短中期记忆流转 | `promotion`：工作→情景→知识晋升管道 | — |
| (7) 量化评测 | 评测框架（bench/ 目录，迭代中）+ 检索通道消融开关 | 完整测试报告 |

## 快速开始

```bash
# 构建（首次约数分钟）
cargo build --release

# 状态
./target/release/lymem status

# 接入示例事件
echo '[{"type":"tool_result","data":{"tool":"libreoffice","task":"doc_edit","ok":true,"output":"导出 pdf 成功","scene":"office"}}]' \
  | ./target/release/lymem ingest

# 检索
./target/release/lymem search "pdf 导出"

# 偏好
./target/release/lymem pref mine-rules
./target/release/lymem pref list

# 遗忘（先预览再执行）
./target/release/lymem forget "忘掉关于doc_edit的一切" --preview
./target/release/lymem forget "忘掉关于doc_edit的一切" --exec

# 蒸馏
./target/release/lymem consolidate

# HTTP 服务 + 管理界面（浏览器打开 http://127.0.0.1:8801/viewer）
LYMEM_PORT=8801 ./target/release/lymem-server
```

## 环境变量

| 变量 | 说明 | 默认 |
|---|---|---|
| `LYMEM_DATA_DIR` | 数据目录 | `~/.local/share/lymem` |
| `LYMEM_EMBEDDER` | `auto`（麒麟）/ `hash`（离线） | `auto` |
| `LYMEM_KYLIN_RUNTIME_SOCK` | 麒麟 runtime DBus 地址 | 按当前 uid 推导 |
| `LYMEM_KYLIN_KYTENSOR_URL` | kytensor Triton 基址 | `http://127.0.0.1:8000` |
| `LYMEM_KYLIN_MODEL` | 嵌入模型 | `ensemble-embd_gte-base_uint8-text` |
| `LYMEM_LLM_BASE_URL` | LLM OpenAI 兼容基址 | `https://api.minimaxi.com/v1` |
| `LYMEM_LLM_API_KEY` | LLM 密钥（缺省则规则兜底） | — |
| `LYMEM_LLM_MODEL` | 模型名 | `MiniMax-M2` |
| `LYMEM_PORT` | 服务端口 | `8801` |

## Workspace 结构

```
crates/
├── lymem-core/    记忆核心（模型/存储/检索/管道，零异步依赖）
├── lymem-kylin/   麒麟端侧绑定（DBus SDK + kytensor 直连双通道）
├── lymem-llm/     LLM 提供方（OpenAI 兼容，MiniMax 预设）
├── lymem-server/  HTTP 服务（REST + 嵌入代理 + 管理界面）
└── lymem-cli/     命令行工具（lymem）
```

## 评测结果（LoCoMo 全量真机，麒麟 V11 端侧嵌入）

| 系统（架构） | hit@5 | 证据召回率 | 延迟 p50 |
|---|---|---|---|
| **lymem（BM25+窗口索引）** | **59.0%** | **54.6%** | **1ms** |
| iwe（结构化 BM25） | 55.9% | 51.2% | 198ms |
| mem0（LLM抽取+向量） | 20.5% | 25.7% | 74ms |
| memmy-agent（L1-L3 分层） | 5.1% | 4.3% | 38ms |

**赛题硬指标定稿（v3，2026-09-02）**：检索延迟 ✅（全站 1~173ms，书山中文 p50 39ms）｜
冲突处理 ✅ 93.3%｜MemBench 记忆能力 ✅ 94.0%｜偏好提取 ✅ 98.0%（PrefEval 存储协议）｜
检索召回 ✅ 96.1%（自建书山·三国 120 回中文知识库，test@5；公开 LoCoMo 严格协议 59.0% 同步如实报告）。

- 检索优化链：47.8% → 52.5%（停用词+Snowball 词干化）→ **59.0%**（邻近轮次窗口索引+会话日期注入）；
- 消融发现：麒麟 gte 向量通道在英文短对话语料的融合中为负贡献（通道贡献按语料自适应），
  图通道在对话语料上需门控；完整消融矩阵见 `benchmark/results/REPORT.md`；
- 环境学结论：Letta（强制 PostgreSQL）、Cognee（150 并发嵌入打崩端侧后端）、
  memorax-code（云 API 依赖）在端侧均不可用——lymem 全链路端侧运行且具双通道自愈能力。

## License

MIT
