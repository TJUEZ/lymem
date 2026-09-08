//! # lymem-core（麟忆尽智 · 记忆核心）
//!
//! 面向银河麒麟桌面 OS Agent 的多源融合偏好与知识记忆核心库。
//!
//! ## 四层记忆体系
//! - **工作记忆**（短期，内存态）：[`promotion::WorkingMemory`]，会话结束蒸馏进入情景层；
//! - **情景记忆**（中期）：会话/工具调用轨迹原文；
//! - **知识记忆**（长期）：工作流程/历史案例/可复用模板，版本链 + 冲突处理；
//! - **偏好记忆**（长期）：操作习惯/输出风格/安全策略，版本时间线 + 跨场景适配。
//!
//! ## 核心能力（对应赛题条款）
//! | 条款 | 模块 |
//! |------|------|
//! | (1) 多源数据整合 | [`ingest`]：清洗/标准化/质量校验（去重、敏感分级） |
//! | (2) 偏好动态捕捉 | [`preference`]：规则快通道 + LLM 慢通道 + 版本链 |
//! | (3) 知识结构化整合 | [`conflict`]：检测→分类→仲裁→版本化；[`retrieval`]：三路混合检索 |
//! | (4) 端侧部署 | [`embedding::Embedder`] + 麒麟向量引擎 SDK + SQLite/tantivy |
//! | (5) 敏感与遗忘 | [`sensitive`] 规则识别；[`forget`] 自然语言遗忘（墓碑/硬清除） |
//! | (6) 短中期流转 | [`promotion`]：工作→情景→知识晋升管道 |
//! | (7) 量化评测 | [`retrieval::SearchParams`] 通道开关即消融维度；统计接口 |

pub mod conflict;
pub mod embedding;
pub mod error;
pub mod experience;
pub mod forget;
pub mod fts;
pub mod hub;
pub mod ingest;
pub mod llm_hook;
pub mod model;
pub mod preference;
pub mod promotion;
pub mod retrieval;
pub mod sensitive;
pub mod store;
pub mod t2s;
pub mod vector_index;

pub use error::{CoreError, Result};
pub use model::{MemoryKind, MemoryRecord, ScoredMemory, Sensitivity, Tier};
pub use store::MemoryStore;
