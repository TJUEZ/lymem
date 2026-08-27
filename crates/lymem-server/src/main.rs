//! lymem-server：麟忆尽智 HTTP 服务。
//!
//! - REST API：记忆/检索/偏好/冲突/遗忘/审计；
//! - `/v1/embeddings`：OpenAI 兼容嵌入代理（后端为麒麟端侧嵌入），
//!   供 mem0 / Letta / memmy 等 Python 基线复用同一嵌入模型，保证横向对比公平；
//! - `/viewer`：中文管理界面（记忆总管）占位页，正式前端后续迭代。

mod mcp;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use lymem_core::forget::{ForgetMode, ForgetScope};
use lymem_core::llm_hook::LlmJudge;
use lymem_core::model::Tier;
use lymem_core::preference::PreferenceSet;
use lymem_core::retrieval::SearchParams;
use lymem_core::MemoryStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 共享应用状态
struct AppState {
    store: MemoryStore,
    judge: Arc<dyn LlmJudge>,
    embedder_name: String,
    llm_name: String,
}

fn default_data_dir() -> PathBuf {
    std::env::var("LYMEM_DATA_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".local/share/lymem")
        })
}

fn build_store() -> Result<MemoryStore, Box<dyn std::error::Error>> {
    let dir = default_data_dir();
    let embedder: Box<dyn lymem_core::embedding::Embedder> =
        match std::env::var("LYMEM_EMBEDDER").ok().as_deref() {
            Some("hash") => Box::new(lymem_core::embedding::HashEmbedder::new(64)),
            _ => Box::new(lymem_kylin::KylinEmbedder::connect_auto()?),
        };
    Ok(MemoryStore::open(&dir.join("lymem.db"), &dir.join("fts"), embedder)?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let store = build_store()?;
    let judge: Arc<dyn LlmJudge> = Arc::from(lymem_llm::judge_from_env());
    let embedder_name = match std::env::var("LYMEM_EMBEDDER").ok().as_deref() {
        Some("hash") => "hash".to_string(),
        _ => "kylin(auto)".to_string(),
    };
    // 探测含阻塞 LLM 调用，须移出 async 运行时
    let probe = judge.clone();
    let llm_ok = tokio::task::spawn_blocking(move || judge_complete_probe(probe.as_ref()))
        .await
        .unwrap_or(false);
    let llm_name = if llm_ok { "llm-ready".into() } else { "rules-only".into() };
    let state = Arc::new(AppState {
        store,
        judge,
        embedder_name,
        llm_name,
    });

    let app = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/status", get(status))
        .route("/api/v1/memories", post(add_memories).get(list_memories))
        .route("/api/v1/search", post(search))
        .route("/api/v1/preferences", get(list_preferences).post(set_preference))
        .route("/api/v1/preferences/{key}", get(get_preference))
        .route("/api/v1/preferences/{key}/history", get(preference_history))
        .route("/api/v1/conflicts", get(list_conflicts))
        .route("/api/v1/consolidate", post(consolidate_now))
        .route("/api/v1/preferences/mine", post(mine_preferences))
        .route("/api/v1/forget/parse", post(forget_parse))
        .route("/api/v1/forget/preview", post(forget_preview))
        .route("/api/v1/forget/exec", post(forget_exec))
        .route("/api/v1/audit", get(list_audit))
        .route("/v1/embeddings", post(openai_embeddings))
        .route("/v1/chat/completions", post(chat_completions_gateway))
        .route("/mcp", post(crate::mcp::mcp_post))
        .route("/viewer", get(viewer))
        .with_state(state);

    let port: u16 = std::env::var("LYMEM_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8801);
    let addr = format!("127.0.0.1:{port}");
    tracing::info!("麟忆尽智 lymem-server 监听 http://{addr}（viewer: http://{addr}/viewer）");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn judge_complete_probe(judge: &dyn LlmJudge) -> bool {
    judge.complete("回复 ok", "ping").is_some()
}

// ---------------- 基础 ----------------

async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok", "service": "lymem", "version": env!("CARGO_PKG_VERSION")}))
}

async fn status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let counts: BTreeMap<String, i64> = {
        let conn = st.store.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT tier, COUNT(*) FROM memories WHERE tombstone = 0 AND is_current = 1 GROUP BY tier")
            .unwrap();
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).unwrap();
        rows.filter_map(|r| r.ok()).collect()
    };
    let prefs = st.store.effective_preferences(None).map(|p| p.len()).unwrap_or(0);
    Json(json!({
        "service": "lymem",
        "embedder": st.embedder_name,
        "embed_dim": st.store.vec_dim,
        "llm": st.llm_name,
        "tiers": counts,
        "preferences": prefs,
    }))
}

// ---------------- 记忆 ----------------

#[derive(Deserialize)]
struct AddMemoriesReq {
    events: Vec<lymem_core::ingest::IngestEvent>,
}

async fn add_memories(State(st): State<Arc<AppState>>, Json(req): Json<AddMemoriesReq>) -> impl IntoResponse {
    match lymem_core::ingest::ingest_events(&st.store, &req.events) {
        Ok(ids) => (StatusCode::OK, Json(json!({"ok": true, "ids": ids}))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": e.to_string()}))),
    }
}

async fn list_memories(
    State(st): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<Value>,
) -> impl IntoResponse {
    let tier = q.get("tier").and_then(|v| v.as_str()).and_then(Tier::parse);
    let scene = q.get("scene").and_then(|v| v.as_str());
    let limit = q.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    let history = q.get("history").and_then(|v| v.as_str()) == Some("1");
    match st.store.list(tier, scene, history, limit) {
        Ok(recs) => (StatusCode::OK, Json(json!(recs))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

async fn search(State(st): State<Arc<AppState>>, Json(mut p): Json<SearchParams>) -> impl IntoResponse {
    if p.query.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "query 为空"})));
    }
    if p.top_k == 0 {
        p.top_k = 8;
    }
    let t0 = std::time::Instant::now();
    match st.store.search(&p) {
        Ok(hits) => {
            let ms = t0.elapsed().as_millis();
            (StatusCode::OK, Json(json!({"took_ms": ms, "hits": hits})))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

// ---------------- 偏好 ----------------

async fn list_preferences(State(st): State<Arc<AppState>>, axum::extract::Query(q): axum::extract::Query<Value>) -> impl IntoResponse {
    let scene = q.get("scene").and_then(|v| v.as_str());
    match st.store.effective_preferences(scene) {
        Ok(ps) => (StatusCode::OK, Json(json!(ps))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

async fn set_preference(State(st): State<Arc<AppState>>, Json(p): Json<PreferenceSet>) -> impl IntoResponse {
    if p.key.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "key 为空"})));
    }
    match st.store.set_preference(&p) {
        Ok(v) => (StatusCode::OK, Json(serde_json::to_value(v).unwrap_or_default())),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

async fn get_preference(State(st): State<Arc<AppState>>, Path(key): Path<String>) -> impl IntoResponse {
    let hist = st.store.preference_history(&key).unwrap_or_default();
    let cur = hist.iter().rev().find(|v| v.valid_to.is_none()).cloned();
    match cur {
        Some(v) => (StatusCode::OK, Json(serde_json::to_value(v).unwrap())),
        None => (StatusCode::NOT_FOUND, Json(json!({"error": "偏好不存在"}))),
    }
}

async fn preference_history(State(st): State<Arc<AppState>>, Path(key): Path<String>) -> impl IntoResponse {
    match st.store.preference_history(&key) {
        Ok(h) if !h.is_empty() => (StatusCode::OK, Json(json!(h))),
        Ok(_) => (StatusCode::NOT_FOUND, Json(json!({"error": "偏好不存在"}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

// ---------------- 冲突 / 遗忘 / 审计 ----------------

#[derive(Deserialize)]
struct MineResp {
    #[serde(default)]
    updates: Vec<serde_json::Value>,
}

#[derive(serde::Serialize)]
struct MinedPref {
    key: String,
    value: serde_json::Value,
    version: i64,
    confidence: f64,
}

/// 偏好规则快通道挖掘：从近期工具调用统计偏好（纯 Rust 统计，无 LLM）
async fn mine_preferences(State(st): State<Arc<AppState>>) -> axum::response::Response {
    let st2 = Arc::clone(&st);
    let res = tokio::task::spawn_blocking(move || st2.store.preference_rules_from_tools(200, 3, 0.6))
        .await;
    match res {
        Ok(Ok(updates)) => {
            let mined: Vec<MinedPref> = updates
                .iter()
                .map(|u| MinedPref {
                    key: u.key.clone(),
                    value: u.value.clone(),
                    version: u.version,
                    confidence: u.confidence,
                })
                .collect();
            (StatusCode::OK, Json(json!({"ok": true, "updates": mined}))).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": format!("任务失败: {e}")})),
        )
            .into_response(),
    }
}

/// 一轮情景→知识蒸馏（LLM 可用时摘要蒸馏）
async fn consolidate_now(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let judge = st.judge.clone();
    let st2 = Arc::clone(&st);
    let res = tokio::task::spawn_blocking(move || {
        st2.store.consolidate_episodic(Some(judge.as_ref()), 3, 4)
    })
    .await;
    match res {
        Ok(Ok(created)) => (StatusCode::OK, Json(json!({"ok": true, "created": created.len(), "ids": created}))),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("任务失败: {e}")}))),
    }
}

async fn list_conflicts(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    match st.store.conflicts_all() {
        Ok(c) => (StatusCode::OK, Json(json!(c))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

#[derive(Deserialize)]
struct ForgetParseReq {
    instruction: String,
}

async fn forget_parse(State(st): State<Arc<AppState>>, Json(req): Json<ForgetParseReq>) -> impl IntoResponse {
    match st.store.parse_forget_scope(&req.instruction, Some(st.judge.as_ref())) {
        Ok(scope) => (StatusCode::OK, Json(json!({"scope": scope}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

async fn forget_preview(State(st): State<Arc<AppState>>, Json(scope): Json<ForgetScope>) -> impl IntoResponse {
    match st.store.forget_preview(&scope, 200) {
        Ok(t) => (StatusCode::OK, Json(json!({"targets": t}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

#[derive(Deserialize)]
struct ForgetExecReq {
    scope: ForgetScope,
    #[serde(default)]
    mode: Option<String>,
}

async fn forget_exec(State(st): State<Arc<AppState>>, Json(req): Json<ForgetExecReq>) -> impl IntoResponse {
    let mode = match req.mode.as_deref() {
        Some("purge") => ForgetMode::Purge,
        _ => ForgetMode::Tombstone,
    };
    match st.store.forget_execute(&req.scope, mode) {
        Ok(n) => (StatusCode::OK, Json(json!({"ok": true, "affected": n, "mode": format!("{mode:?}").to_lowercase()}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

async fn list_audit(State(st): State<Arc<AppState>>, axum::extract::Query(q): axum::extract::Query<Value>) -> impl IntoResponse {
    let limit = q.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    match st.store.audit_recent(limit) {
        Ok(a) => (StatusCode::OK, Json(json!(a))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

// ---------------- OpenAI 兼容嵌入代理 ----------------

#[derive(Deserialize)]
struct EmbeddingsReq {
    input: Value,
    #[serde(default)]
    model: String,
}

#[derive(Serialize)]
struct EmbeddingsResp {
    object: &'static str,
    data: Vec<EmbeddingsData>,
    model: String,
    usage: Value,
}

#[derive(Serialize)]
struct EmbeddingsData {
    object: &'static str,
    index: usize,
    embedding: Vec<f32>,
}

async fn openai_embeddings(State(st): State<Arc<AppState>>, Json(req): Json<EmbeddingsReq>) -> impl IntoResponse {
    let inputs: Vec<String> = match req.input {
        Value::String(s) => vec![s],
        Value::Array(a) => a
            .into_iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    if inputs.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "input 必须为字符串或字符串数组"}})),
        );
    }
    // 嵌入为同步阻塞调用（麒麟端侧 ~30ms/条），放到阻塞线程池
    let refs: Vec<String> = inputs.clone();
    let dim0 = st.store.vec_dim;
    let state = st.clone();
    let res = tokio::task::spawn_blocking(move || {
        let rs: Vec<&str> = refs.iter().map(|s| s.as_str()).collect();
        state.store.embedder.embed(&rs)
    })
    .await;
    match res {
        Ok(Ok(vectors)) => {
            let est_tokens: u64 = inputs.iter().map(|s| s.chars().count() as u64 / 2).sum();
            let resp = EmbeddingsResp {
                object: "list",
                data: vectors
                    .into_iter()
                    .enumerate()
                    .map(|(i, e)| EmbeddingsData {
                        object: "embedding",
                        index: i,
                        embedding: e,
                    })
                    .collect(),
                model: req.model,
                usage: json!({"prompt_tokens": est_tokens, "total_tokens": est_tokens}),
            };
            let _ = dim0;
            (StatusCode::OK, Json(serde_json::to_value(resp).unwrap()))
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": {"message": e.to_string()}})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": {"message": format!("任务失败: {e}")}})),
        ),
    }
}

// ---------------- LLM 网关（OpenAI 兼容透传） ----------------
///
/// 供 mem0 等基线使用：剥离目标服务不支持的参数（response_format 等），
/// 把 JSON 约束转写为系统指令后转发上游（默认 MiniMax）。
/// 环境变量：LYMEM_LLM_UPSTREAM（默认 https://api.minimaxi.com/v1），
/// LYMEM_LLM_API_KEY / LYMEM_LLM_MODEL 同 lymem-llm。
async fn chat_completions_gateway(
    State(_st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(mut body): Json<Value>,
) -> impl IntoResponse {
    let upstream = std::env::var("LYMEM_LLM_UPSTREAM")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://api.minimaxi.com/v1".into());
    let server_key = std::env::var("LYMEM_LLM_API_KEY").unwrap_or_default();
    if server_key.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": {"message": "服务端未配置 LYMEM_LLM_API_KEY"}})));
    }
    // 允许调用方自带 Bearer key（脚本直连场景）
    let api_key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| !s.is_empty())
        .unwrap_or(&server_key)
        .to_string();

    // 参数清洗：response_format / strict 转 JSON 指令
    let mut json_required = false;
    if let Some(obj) = body.as_object_mut() {
        if obj.remove("response_format").is_some() {
            json_required = true;
        }
        obj.remove("strict");
        let model_empty = obj.get("model").and_then(|m| m.as_str()).map(|m| m.is_empty()).unwrap_or(true);
        if model_empty {
            obj.insert("model".into(), json!(std::env::var("LYMEM_LLM_MODEL").unwrap_or_else(|_| "MiniMax-Text-01".into())));
        }
        if json_required {
            if let Some(msgs) = obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
                msgs.insert(0, json!({"role": "system", "content": "只输出合法 JSON，不要输出任何其他文字或代码块标记。"}));
            }
        }
    }

    let url = format!("{}/chat/completions", upstream.trim_end_matches('/'));
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(300)).build().unwrap();
    match client.post(&url).bearer_auth(&api_key).json(&body).send().await {
        Ok(resp) => {
            let status = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let json_out: Value = serde_json::from_str(&txt).unwrap_or(json!({"raw": txt}));
            (code, Json(json_out))
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": {"message": format!("上游请求失败: {e}")}})),
        ),
    }
}

// ---------------- 管理界面占位 ----------------

async fn viewer() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        VIEWER_HTML,
    )
}

const VIEWER_HTML: &str = include_str!("viewer.html");
