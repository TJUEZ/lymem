//! LLM 钩子：核心库保持零异步依赖，通过同步 trait 注入 LLM 能力。
//! 具体实现由 lymem-llm 提供（OpenAI 兼容协议，MiniMax 预设）。

/// LLM 判定器：同步完成一次问答。
/// 返回 None 表示 LLM 不可用，调用方走规则兜底。
pub trait LlmJudge: Send + Sync {
    fn complete(&self, system: &str, user: &str) -> Option<String>;
}

/// 空实现：任何环境下可用（规则兜底路径）。
pub struct NoLlm;

impl LlmJudge for NoLlm {
    fn complete(&self, _system: &str, _user: &str) -> Option<String> {
        None
    }
}
