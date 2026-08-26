//! 记忆数据模型：四层记忆体系。
//!
//! 对应赛题条款的映射关系：
//! - 工作记忆（短期）：当前会话上下文，内存态，会话结束蒸馏进入情景层 —— 条款(6) 短期记忆；
//! - 情景记忆（中期）：会话/工具调用轨迹原文 —— 条款(6) 中期记忆；
//! - 知识记忆（长期）：工作流程/历史案例/可复用模板，带版本链与冲突处理 —— 条款(3)；
//! - 偏好记忆（长期）：操作习惯/输出风格/安全策略，带版本时间线 —— 条款(2)。

use serde::{Deserialize, Serialize};

/// 记忆层级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tier {
    Working,
    Episodic,
    Knowledge,
    Preference,
}

impl Tier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Working => "working",
            Tier::Episodic => "episodic",
            Tier::Knowledge => "knowledge",
            Tier::Preference => "preference",
        }
    }
    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "working" => Some(Tier::Working),
            "episodic" => Some(Tier::Episodic),
            "knowledge" => Some(Tier::Knowledge),
            "preference" => Some(Tier::Preference),
            _ => None,
        }
    }
}

/// 记忆种类：同时标记数据来源与语义类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryKind {
    /// 会话轮次
    Conversation,
    /// 工具调用执行结果（赛题核心数据源）
    ToolResult,
    /// 用户行为事件（跨场景行为数据）
    UserBehavior,
    /// 手动配置信息
    ManualConfig,
    /// 工作流程知识
    Workflow,
    /// 历史案例
    Case,
    /// 可复用模板
    Template,
    /// 一般事实知识
    Fact,
    /// 偏好观察（原始证据，提取后进入偏好版本链）
    PreferenceObs,
}

impl MemoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryKind::Conversation => "conversation",
            MemoryKind::ToolResult => "tool_result",
            MemoryKind::UserBehavior => "user_behavior",
            MemoryKind::ManualConfig => "manual_config",
            MemoryKind::Workflow => "workflow",
            MemoryKind::Case => "case",
            MemoryKind::Template => "template",
            MemoryKind::Fact => "fact",
            MemoryKind::PreferenceObs => "preference_obs",
        }
    }
    pub fn parse(s: &str) -> Option<MemoryKind> {
        Some(match s {
            "conversation" => MemoryKind::Conversation,
            "tool_result" => MemoryKind::ToolResult,
            "user_behavior" => MemoryKind::UserBehavior,
            "manual_config" => MemoryKind::ManualConfig,
            "workflow" => MemoryKind::Workflow,
            "case" => MemoryKind::Case,
            "template" => MemoryKind::Template,
            "fact" => MemoryKind::Fact,
            "preference_obs" => MemoryKind::PreferenceObs,
            _ => return None,
        })
    }
}

/// 敏感等级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sensitivity {
    None,
    /// 敏感：脱敏后入库
    Sensitive,
    /// 阻断：不进入长期层，仅审计
    Blocked,
}

impl Sensitivity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Sensitivity::None => "none",
            Sensitivity::Sensitive => "sensitive",
            Sensitivity::Blocked => "blocked",
        }
    }
    pub fn parse(s: &str) -> Sensitivity {
        match s {
            "sensitive" => Sensitivity::Sensitive,
            "blocked" => Sensitivity::Blocked,
            _ => Sensitivity::None,
        }
    }
}

/// 一条长期记忆（持久化在 SQLite 中）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: i64,
    pub tier: Tier,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    /// 来源标识（agent/app/渠道名）
    pub source: String,
    /// 场景（coding / office / system / general ...）
    pub scene: String,
    pub confidence: f64,
    pub sensitivity: Sensitivity,
    /// 结构化附加信息（工具名、参数、结果码等）
    pub meta: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
    pub access_count: i64,
    pub last_access_at: Option<i64>,
    /// 墓碑软删标记（遗忘用，可审计）
    pub tombstone: bool,
    pub tombstoned_at: Option<i64>,
    /// 版本链：本条替代的旧记忆 id
    pub version_of: Option<i64>,
    /// 是否为当前有效版本（被取代后置 0，历史仍可查）
    pub is_current: bool,
    /// 关联实体名列表（JSON 数组字符串存储）
    pub entities: Vec<String>,
}

impl MemoryRecord {
    /// 新建记录（id 由数据库分配，这里给占位 0）
    pub fn new(tier: Tier, kind: MemoryKind, title: impl Into<String>, content: impl Into<String>) -> Self {
        let now = chrono::Utc::now().timestamp();
        MemoryRecord {
            id: 0,
            tier,
            kind,
            title: title.into(),
            content: content.into(),
            source: String::new(),
            scene: "general".into(),
            confidence: 0.5,
            sensitivity: Sensitivity::None,
            meta: serde_json::json!({}),
            created_at: now,
            updated_at: now,
            access_count: 0,
            last_access_at: None,
            tombstone: false,
            tombstoned_at: None,
            version_of: None,
            is_current: true,
            entities: Vec::new(),
        }
    }
}

/// 检索命中的记忆（带融合得分与各通道排名）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredMemory {
    pub record: MemoryRecord,
    /// RRF 融合后得分（重排前）
    pub rrf_score: f64,
    /// 重排后最终得分
    pub final_score: f64,
    /// 各通道排名：向量 / BM25 / 图扩展（1 起）
    pub rank_vec: Option<usize>,
    pub rank_bm25: Option<usize>,
    pub rank_graph: Option<usize>,
    /// 命中片段（来自向量或全文通道的最佳片段）
    pub snippet: String,
}
