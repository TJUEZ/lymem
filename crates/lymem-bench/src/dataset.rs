//! 数据集抽象：统一的多轮会话 + 问答结构。
//! 各 benchmark 适配器负责把原始格式转成该结构。

use serde::{Deserialize, Serialize};

/// 一轮对话
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    /// 原始定位 id（如 LoCoMo 的 "D1:3"），用于证据比对
    pub dia_id: String,
    pub speaker: String,
    pub text: String,
}

/// 一个会话段（LoCoMo 的 session / LongMemEval 的轮次组）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub turns: Vec<Turn>,
}

/// 一个评测问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    pub answer: String,
    /// 金标准证据的 dia_id 集合
    pub evidence: Vec<String>,
    /// 类别标签（benchmark 自定义，如 LoCoMo 1-5）
    pub category: String,
}

/// 一段完整对话（多 session）+ 其问答集
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub sessions: Vec<Session>,
    pub questions: Vec<Question>,
}

/// 数据集 trait
pub trait Dataset {
    fn name(&self) -> &'static str;
    /// 从文件加载全部对话
    fn load(&self, path: &std::path::Path) -> anyhow_like::Result<Vec<Conversation>>;
}

/// 轻量错误别名（避免引入 anyhow 依赖）
pub mod anyhow_like {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
}
