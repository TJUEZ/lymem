//! # lymem-llm（LLM 提供方）
//!
//! 双协议 chat 客户端，为 lymem-core 的同步 [`LlmJudge`] 钩子提供实现：
//! - **anthropic** 协议（默认）：MiniMax 的 Anthropic 兼容端点
//!   `https://api.minimaxi.com/anthropic/v1/messages`，模型 `MiniMax-M3`；
//! - **openai** 协议：任何 OpenAI 兼容 `/chat/completions` 服务。
//!
//! 配置（环境变量，均可缺省后降级为不可用状态，核心走规则兜底）：
//! - `LYMEM_LLM_PROTOCOL`：`anthropic`（默认）/ `openai`
//! - `LYMEM_LLM_BASE_URL`：默认 `https://api.minimaxi.com/anthropic`
//! - `LYMEM_LLM_API_KEY`：**必填**，未设置时客户端整体禁用
//! - `LYMEM_LLM_MODEL`：默认 `MiniMax-M3`
//! - `LYMEM_LLM_TIMEOUT_SECS`：默认 60
//! - `LYMEM_LLM_MAX_TOKENS`：默认 1024

use std::time::Duration;

use lymem_core::llm_hook::LlmJudge;
use serde::{Deserialize, Serialize};

/// 协议类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    #[default]
    Anthropic,
    OpenAi,
}

impl Protocol {
    pub fn parse(s: &str) -> Protocol {
        match s.to_ascii_lowercase().as_str() {
            "openai" => Protocol::OpenAi,
            _ => Protocol::Anthropic,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub protocol: Protocol,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout: Duration,
    pub max_tokens: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        LlmConfig {
            protocol: Protocol::parse(&std::env::var("LYMEM_LLM_PROTOCOL").unwrap_or_default()),
            base_url: std::env::var("LYMEM_LLM_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "https://api.minimaxi.com/anthropic".into()),
            api_key: std::env::var("LYMEM_LLM_API_KEY").unwrap_or_default(),
            model: std::env::var("LYMEM_LLM_MODEL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "MiniMax-M3".into()),
            timeout: Duration::from_secs(
                std::env::var("LYMEM_LLM_TIMEOUT_SECS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(60),
            ),
            max_tokens: std::env::var("LYMEM_LLM_MAX_TOKENS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1024),
        }
    }
}

impl LlmConfig {
    pub fn available(&self) -> bool {
        !self.api_key.is_empty()
    }
}

// ---------------- OpenAI 协议载荷 ----------------

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: [ChatMessage<'a>; 2],
    temperature: f32,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: RespMessage,
}

#[derive(Deserialize)]
struct RespMessage {
    content: String,
}

// ---------------- Anthropic 协议载荷 ----------------

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: [AnthropicMessage<'a>; 1],
}

#[derive(Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicBlock>,
}

#[derive(Deserialize)]
struct AnthropicBlock {
    #[serde(rename = "type")]
    _type: String,
    #[serde(default)]
    text: String,
}

/// 双协议同步客户端（内部异步 + 独立运行时）
pub struct OpenAiCompat {
    cfg: LlmConfig,
    rt: tokio::runtime::Runtime,
    client: reqwest::Client,
}

impl OpenAiCompat {
    pub fn new(cfg: LlmConfig) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let client = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(OpenAiCompat { cfg, rt, client })
    }

    pub fn from_env() -> Result<Self, String> {
        Self::new(LlmConfig::default())
    }

    pub fn config(&self) -> &LlmConfig {
        &self.cfg
    }

    pub async fn complete_async(&self, system: &str, user: &str) -> Result<String, String> {
        match self.cfg.protocol {
            Protocol::Anthropic => self.complete_anthropic(system, user).await,
            Protocol::OpenAi => self.complete_openai(system, user).await,
        }
    }

    async fn complete_anthropic(&self, system: &str, user: &str) -> Result<String, String> {
        let url = format!("{}/v1/messages", self.cfg.base_url.trim_end_matches('/'));
        let req = AnthropicRequest {
            model: &self.cfg.model,
            max_tokens: self.cfg.max_tokens,
            system,
            messages: [AnthropicMessage {
                role: "user",
                content: user,
            }],
        };
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.cfg.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("请求失败: {e}"))?;
        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("服务返回 {code}: {body}"));
        }
        let parsed: AnthropicResponse = resp.json().await.map_err(|e| format!("解析失败: {e}"))?;
        parsed
            .content
            .into_iter()
            .filter(|b| b._type == "text")
            .map(|b| b.text)
            .next()
            .ok_or_else(|| "空响应".to_string())
    }

    async fn complete_openai(&self, system: &str, user: &str) -> Result<String, String> {
        let url = format!("{}/chat/completions", self.cfg.base_url.trim_end_matches('/'));
        let req = ChatRequest {
            model: &self.cfg.model,
            messages: [
                ChatMessage {
                    role: "system",
                    content: system,
                },
                ChatMessage {
                    role: "user",
                    content: user,
                },
            ],
            temperature: 0.1,
        };
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.cfg.api_key)
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("请求失败: {e}"))?;
        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("服务返回 {code}: {body}"));
        }
        let parsed: ChatResponse = resp.json().await.map_err(|e| format!("解析失败: {e}"))?;
        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| "空响应".to_string())
    }
}

impl LlmJudge for OpenAiCompat {
    fn complete(&self, system: &str, user: &str) -> Option<String> {
        if !self.cfg.available() {
            return None;
        }
        self.rt.block_on(async { self.complete_async(system, user).await.ok() })
    }
}

/// 按环境构造 LLM 判定器：未配置 key 时返回不可用的空实现。
pub fn judge_from_env() -> Box<dyn LlmJudge> {
    let cfg = LlmConfig::default();
    if !cfg.available() {
        tracing::info!("未配置 LYMEM_LLM_API_KEY，LLM 增强路径禁用（规则兜底）");
        return Box::new(lymem_core::llm_hook::NoLlm);
    }
    match OpenAiCompat::new(cfg) {
        Ok(c) => {
            tracing::info!(
                "LLM 就绪: {} @ {}（{:?} 协议）",
                c.config().model,
                c.config().base_url,
                c.config().protocol
            );
            Box::new(c)
        }
        Err(e) => {
            tracing::warn!("LLM 初始化失败（{e}），规则兜底");
            Box::new(lymem_core::llm_hook::NoLlm)
        }
    }
}
