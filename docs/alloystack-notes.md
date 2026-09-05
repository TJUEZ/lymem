# AlloyStack 适用性分析笔记

> 结论先行：**不引入 AlloyStack 作为 lymem 的依赖或运行平台**；其针对的两类开销（冷启动、函数间数据传递）恰好映射为 lymem 自身的两个优化维度（启动时延、检索热路径冗余），已纳入性能优化计划（见下文"思路移植"）。

## 1 AlloyStack 是什么

AlloyStack（EuroSys 2025，tanksys/AlloyStack）是一个面向 **Serverless Workflow 应用**的**库操作系统（LibOS）**，部分代码源自 RuxOS。两大核心优化：

1. **按需加载**：降低函数冷启动延迟（而非整体镜像加载）；
2. **引用传递**：工作流函数间以引用共享中间数据，避免拷贝/序列化开销。

环境要求：nightly-2023-12-01 工具链、gcc 11.4；论文复现在 Ubuntu 22.04；需要 **Intel MPK**（内存保护键）硬件特性；as-visor 及部分测试流程需要 root/KVM。

## 2 与 lymem 的形态对照（为何不适用）

| 维度 | AlloyStack 假设 | lymem 现实 | 结论 |
|---|---|---|---|
| 问题形态 | serverless 函数链：冷启动延迟与函数间数据传递是主要开销 | **常驻单进程服务**（axum HTTP），无函数链；冷启动每次登录仅一次，不在 ≤500ms 检索指标路径上 | 优化点正交，无收益面 |
| 集成成本 | 应用需改写为 AlloyStack functions（as_std），workflow JSON 编排 | 需放弃 tokio/axum 异步栈；SQLite/sqlite-vec/tantivy 等 C 依赖需移植进 LibOS 环境 | 相当于整体重构 |
| 硬件/权限 | Intel MPK；部分流程需 root/KVM | 本机实测：AMD 9700X **有 pku/ospke** ✓ 但**无 /dev/kvm**、**无 sudo**；麒麟桌面目标硬件（飞腾/鲲鹏/龙芯/兆芯）普遍**无 MPK** | 本机跑不了完整流程；与"麒麟桌面适配"交付形态冲突 |
| 工具链 | nightly-2023-12-01 | lymem 用 stable（1.98） | 双工具链维护负担 |
| 现有指标 | — | 检索 p50 27ms / p99 55ms（预算 500ms，超标 0/442） | 性能余量已很大，瓶颈在端侧嵌入调用与 LLM 网络，非进程间通信 |

## 3 思路移植（本笔记的可执行产出）

AlloyStack 的两个优化目标各自对应 lymem 的一个真实短板（探查确认），已列入性能优化计划：

### 3.1 "按需加载/冷启动" → lymem 启动时延

现状：`main()` 串行阻塞完成全部初始化才 bind 端口——嵌入通道探测最长 8s（DBus 8s 看门狗 + kytensor 模型加载），LLM 探测最长 60s。对"即开即用"桌面软件，这是点开菜单到可用的真实等待。

优化：LLM 探测后台化（原子标志，`/status` 惰性呈现）；启动到首请求成功的毫秒数纳入测量。

### 3.2 "引用传递/减少拷贝" → 检索热路径去无谓功

现状（retrieval.rs/store.rs，探查确认）：
- RRF 融合对每个候选单独 `get()`（每候选一次锁+全行读+2 次 JSON parse）、每命中一次 `chunk_content` 查库、每候选 3 次 `std::env::var`；
- 候选去重 O(n²) 线性扫描；`touch` 逐条 UPDATE 无事务包裹；
- 查询向量每请求重算（端侧嵌入 ~30ms，占 p50 大头）；`ask` 对同一查询重复嵌入两次；嵌入通道全局 Mutex 串行；
- SQLite 仅 WAL+NORMAL 两项 PRAGMA。

优化：候选批量查询（WHERE id IN）+ HashMap 分桶、env 权重读一次、touch 合并单事务、查询向量 LRU 缓存（CachingEmbedder 装饰器，零 core 侵入）、补 PRAGMA、高频 handler 包 spawn_blocking。**core 有条件解冻**：每批改完复跑七站评测，召回须逐题不变，延迟前后对比记录。

### 3.3 量化口径

以 benchmark 现有 442 查询管线为基准（优化前后各跑一轮），另测 lymem-server 启动到首请求成功毫秒数。产出前后对比表，作为评分标准"端侧轻量化程度"的量化论证。

## 4 相关工作定位（供论文引用）

论文 related work 可将 AlloyStack 归入"OS 层面的 agent/函数运行时优化"：其优化对象是函数粒度的执行平台，与 lymem 面向"记忆粒度"（存储/检索/演化）的端侧优化互补；lymem 选择常驻服务形态正是因为记忆需要跨会话累积（WikiSkill 式持久知识层），与 serverless 一次性执行模型相反。
