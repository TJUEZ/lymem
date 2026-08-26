//! lymem-server：麟忆尽智 HTTP 服务。
//!
//! - REST API：记忆/检索/偏好/冲突/遗忘/审计；
//! - `/v1/embeddings`：OpenAI 兼容嵌入代理（后端为麒麟端侧嵌入），
//!   供 mem0 / Letta / memmy 等 Python 基线复用同一嵌入模型，保证横向对比公平；
//! - `/viewer`：中文管理界面（记忆总管）占位页，正式前端后续迭代。

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
        .route("/api/v1/forget/parse", post(forget_parse))
        .route("/api/v1/forget/preview", post(forget_preview))
        .route("/api/v1/forget/exec", post(forget_exec))
        .route("/api/v1/audit", get(list_audit))
        .route("/v1/embeddings", post(openai_embeddings))
        .route("/v1/chat/completions", post(chat_completions_gateway))
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
    match st.store.list(tier, scene, false, limit) {
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

const VIEWER_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh">
<head><meta charset="utf-8"><title>麟忆尽智 · 记忆总管</title>
<style>
body{font-family:"Noto Sans CJK SC",sans-serif;margin:0;background:#f5f6fa;color:#2f3542}
header{background:#1e90ff;color:#fff;padding:18px 28px}header h1{margin:0;font-size:22px}
main{padding:20px 28px;max-width:1100px;margin:0 auto}
.card{background:#fff;border-radius:10px;box-shadow:0 1px 4px rgba(0,0,0,.08);padding:16px 20px;margin-bottom:16px}
button{background:#1e90ff;color:#fff;border:none;border-radius:6px;padding:6px 16px;cursor:pointer}
input,select{padding:6px 10px;border:1px solid #dcdde1;border-radius:6px;margin-right:8px}
table{width:100%;border-collapse:collapse;font-size:14px}td,th{padding:6px 10px;border-bottom:1px solid #f1f2f6;text-align:left}
.tag{display:inline-block;padding:1px 8px;border-radius:10px;font-size:12px;background:#e8f3ff;color:#1e90ff;margin-right:4px}
#log{white-space:pre-wrap;font-family:monospace;background:#f8f9fa;padding:12px;border-radius:8px;font-size:13px}
</style></head>
<body>
<header><h1>麟忆尽智 · 记忆总管 <span style="font-size:13px;opacity:.8">lymem viewer（占位版，正式前端迭代中）</span></h1></header>
<main>
<div class="card"><b>系统状态</b><div id="status" style="margin-top:8px">加载中…</div></div>
<div class="card"><b>检索</b><div style="margin-top:8px">
<input id="q" placeholder="输入查询，如：部署路径" style="width:360px">
<select id="ablation"><option value="">全通道</option><option value="nov">仅 BM25+图</option><option value="nob">仅向量+图</option><option value="nog">仅向量+BM25</option><option value="nop">关偏好重排</option></select>
<button onclick="doSearch()">检索</button></div>
<div id="hits" style="margin-top:10px"></div></div>
<div class="card"><b>最近记忆</b><div style="margin-top:8px"><select id="tier"><option value="">全部层级</option><option value="episodic">情景（中期）</option><option value="knowledge">知识（长期）</option><option value="preference">偏好观察</option></select><button onclick="loadMemories()">刷新</button></div>
<div style="margin-top:10px;overflow:auto"><table id="mem"><thead><tr><th>ID</th><th>标题</th><th>层级</th><th>场景</th><th>敏感</th></tr></thead><tbody></tbody></table></div></div>
<div class="card"><b>当前偏好</b><div style="margin-top:8px"><button onclick="loadPrefs()">刷新</button></div>
<div style="margin-top:10px;overflow:auto"><table id="prefs"><thead><tr><th>键</th><th>值</th><th>版本</th><th>来源</th><th>场景</th></tr></thead><tbody></tbody></table></div></div>
<div class="card"><b>遗忘指令</b><div style="margin-top:8px">
<input id="fi" placeholder="如：忘掉关于项目部署路径的一切" style="width:360px">
<button onclick="previewForget()">解析并预览</button><button onclick="execForget()">执行（软删）</button></div>
<div id="ft" style="margin-top:10px"></div></div>
</main>
<script>
const $=id=>document.getElementById(id);
async function api(p,o){const r=await fetch(p,o?{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(o)}:undefined);return r.json()}
(async()=>{const s=await api('/api/v1/status');$('status').textContent=JSON.stringify(s,null,2)})();
async function doSearch(){const b={query:$('q').value,top_k:8};const a=$('ablation').value;
if(a==='nov')b.use_vec=false;if(a==='nob')b.use_bm25=false;if(a==='nog')b.use_graph=false;if(a==='nop')b.preference_rerank=false;
const r=await api('/api/v1/search',b);const h=(r.hits||[]).map(x=>`<tr><td>${x.record.id}</td><td>${x.record.title}</td><td>${x.final_score.toFixed(4)}</td><td>${x.snippet.slice(0,80)}</td></tr>`).join('');
$('hits').innerHTML=`<div>耗时 ${r.took_ms}ms，命中 ${(r.hits||[]).length} 条</div><table><thead><tr><th>ID</th><th>标题</th><th>得分</th><th>片段</th></tr></thead><tbody>${h}</tbody></table>`}
async function loadMemories(){const t=$('tier').value;const r=await api('/api/v1/memories'+(t?'?tier='+t:''));
$('mem').querySelector('tbody').innerHTML=(r||[]).map(m=>`<tr><td>${m.id}</td><td>${m.title}</td><td><span class="tag">${m.tier}</span></td><td>${m.scene}</td><td>${m.sensitivity}</td></tr>`).join('')}
loadMemories();
async function loadPrefs(){const r=await api('/api/v1/preferences');
$('prefs').querySelector('tbody').innerHTML=(r||[]).map(p=>`<tr><td>${p.key}</td><td>${JSON.stringify(p.value)}</td><td>v${p.version}</td><td>${p.source}</td><td>${p.scenes.join(',')}</td></tr>`).join('')}
loadPrefs();
async function previewForget(){const r=await api('/api/v1/forget/parse',{instruction:$('fi').value});
const t=await api('/api/v1/forget/preview',r.scope);
$('ft').innerHTML=`<div>解析范围：${JSON.stringify(r.scope)}</div><div>命中 ${（t.targets||[]).length} 条</div>`}
async function execForget(){const r=await api('/api/v1/forget/parse',{instruction:$('fi').value});
const t=await api('/api/v1/forget/exec',{scope:r.scope,mode:'tombstone'});
$('ft').innerHTML+=`<div>执行结果：${JSON.stringify(t)}</div>`;loadMemories()}
</script>
</body></html>"#;
