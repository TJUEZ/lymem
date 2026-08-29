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
        .route("/api/v1/memories/{id}", get(get_memory))
        .route("/api/v1/knowledge", post(add_knowledge))
        .route("/api/v1/sensitive/scan", post(sensitive_scan))
        .route("/api/v1/ingest/chapters", post(ingest_chapters))
        .route("/api/v1/ask", post(ask_memory))
        .route("/api/v1/ingest/book_prepare", get(book_prepare))
        .route("/api/v1/consolidate", post(consolidate_now))
        .route("/api/v1/preferences/mine", post(mine_preferences))
        .route("/api/v1/preferences/extract", post(extract_preferences_from_sessions))
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

/// 会话陈述偏好捕捉（赛题条款 2：从会话数据源动态提取偏好，LLM 通道）
async fn extract_preferences_from_sessions(State(st): State<Arc<AppState>>) -> axum::response::Response {
    // 各分支 into_response()
    let st2 = Arc::clone(&st);
    let judge = st.judge.clone();
    let res = tokio::task::spawn_blocking(move || {
        // 取近期未处理的会话记录
        let records = st2.store.list(Some(lymem_core::model::Tier::Episodic), None, false, 60)?;
        let mut extracted = Vec::new();
        for rec in records.iter().filter(|r| r.kind == lymem_core::model::MemoryKind::Conversation) {
            let Some(analysis) = judge.complete(
                "你是 OS Agent 的偏好提取器。判断用户消息是否表达了可持续生效的偏好（工具选择/输出风格/安全策略/工作习惯）。\
                 只输出 JSON：{\"is_preference\": bool, \"key\": \"类别.名称\"（类别限 tool_choice/output_style/security/habit）, \"
                 + \"value\": \"偏好内容简述\"}。非偏好输出 {\"is_preference\": false}。",
                &rec.content,
            ) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(analysis.trim_start_matches("```json").trim_end_matches("```").trim()) else { continue };
            if v.get("is_preference").and_then(|x| x.as_bool()) != Some(true) { continue; }
            let (Some(key), Some(val)) = (v.get("key").and_then(|x| x.as_str()), v.get("value")) else { continue };
            st2.store.set_preference(&lymem_core::preference::PreferenceSet {
                key: format!("session.{key}"),
                value: val.clone(),
                evidence: vec![json!({"memory_id": rec.id, "text": rec.content.chars().take(200).collect::<String>()})],
                source: "llm".into(),
                confidence: 0.75,
                scenes: vec![rec.scene.clone()],
                force: false,
            })?;
            extracted.push(json!({"key": format!("session.{key}"), "value": val, "memory_id": rec.id}));
        }
        Ok::<Vec<Value>, lymem_core::CoreError>(extracted)
    })
    .await;
    match res {
        Ok(Ok(items)) => (StatusCode::OK, Json(json!({"ok": true, "extracted": items, "count": items.len()}))).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("任务失败: {e}")}))).into_response(),
    }
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

async fn get_memory(State(st): State<Arc<AppState>>, Path(id): Path<i64>) -> impl IntoResponse {
    match st.store.get(id) {
        Ok(Some(r)) => (StatusCode::OK, Json(serde_json::to_value(r).unwrap_or_default())),
        _ => (StatusCode::NOT_FOUND, Json(json!({"error": "不存在"}))),
    }
}

#[derive(Deserialize)]
struct KnowledgeReq {
    title: String,
    content: String,
    #[serde(default)]
    scene: String,
    #[serde(default)]
    entities: Vec<String>,
}

/// 知识层写入（走冲突四步管道：检测→分类→仲裁→版本化）
async fn add_knowledge(State(st): State<Arc<AppState>>, Json(req): Json<KnowledgeReq>) -> impl IntoResponse {
    let st2 = Arc::clone(&st);
    let judge = st.judge.clone();
    let res = tokio::task::spawn_blocking(move || {
        let mut rec = lymem_core::model::MemoryRecord::new(
            lymem_core::model::Tier::Knowledge,
            lymem_core::model::MemoryKind::Fact,
            req.title,
            req.content,
        );
        rec.scene = if req.scene.is_empty() { "general".into() } else { req.scene };
        rec.entities = req.entities;
        st2.store.put_knowledge_with_conflicts(rec, Some(judge.as_ref()), 0.8)
    })
    .await;
    match res {
        Ok(Ok((id, outcomes))) => (
            StatusCode::OK,
            Json(json!({
                "ok": true, "id": id,
                "conflicts": outcomes.iter().map(|o| json!({
                    "type": o.ctype.as_str(), "resolution": o.resolution.as_str(),
                    "decided_by": o.decided_by, "reason": o.reason,
                })).collect::<Vec<_>>(),
            })),
        ),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("任务失败: {e}")}))),
    }
}

#[derive(Deserialize)]
struct ScanReq {
    text: String,
}

/// 敏感信息识别测试台：返回命中与脱敏结果（不写库）
async fn sensitive_scan(Json(req): Json<ScanReq>) -> impl IntoResponse {
    let findings = lymem_core::sensitive::scan(&req.text);
    let level = lymem_core::sensitive::level_of(&findings);
    let redacted = lymem_core::sensitive::redact(&req.text);
    Json(json!({
        "level": level.as_str(),
        "findings": findings,
        "redacted": redacted,
    }))
}

/// 内置《三国演义》简体全文分章（供前端书山灌入一键拉取）。
/// 文件路径：LYMEM_BOOK_PATH 或 data/sanguo_simplified.txt
async fn book_prepare() -> impl IntoResponse {
    let path = std::env::var("LYMEM_BOOK_PATH").unwrap_or_else(|_| "data/sanguo_simplified.txt".into());
    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({"error": format!("书文本不存在（{}）：{}。可改用粘贴文本方式。", path, e)}))),
    };
    let mut marks: Vec<(usize, String)> = body
        .lines()
        .enumerate()
        .filter_map(|(li, l)| {
            let t = l.trim();
            (t.starts_with("第") && t.contains("回：") && t.chars().count() < 60).then(|| (li, t.to_string()))
        })
        .collect();
    if marks.len() < 50 {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("章节解析异常：仅 {} 个回目", marks.len())})));
    }
    let lines: Vec<&str> = body.lines().collect();
    let mut chapters = Vec::new();
    for (i, (li, title)) in marks.iter().enumerate() {
        let nxt = marks.get(i + 1).map(|(l, _)| *l).unwrap_or(lines.len());
        let content = lines[*li..nxt].join("\n");
        chapters.push(json!({
            "title": title, "index": i,
            "content": content.chars().take(20000).collect::<String>(),
        }));
    }
    let _ = &mut marks;
    (StatusCode::OK, Json(json!({ "title": "三国演义", "chapters": chapters })))
}

#[derive(Deserialize)]
struct Chapter {
    title: String,
    content: String,
    #[serde(default)]
    index: i64,
}

/// 书山灌入：整本长文本按章批量入库（知识层，规则管道）
async fn ingest_chapters(State(st): State<Arc<AppState>>, Json(chapters): Json<Vec<Chapter>>) -> impl IntoResponse {
    let st2 = Arc::clone(&st);
    let res = tokio::task::spawn_blocking(move || {
        let mut ids = Vec::new();
        let mut errs = 0;
        for ch in chapters.iter() {
            let mut rec = lymem_core::model::MemoryRecord::new(
                lymem_core::model::Tier::Knowledge,
                lymem_core::model::MemoryKind::Case,
                ch.title.clone(),
                ch.content.clone(),
            );
            rec.scene = "book".into();
            rec.source = "book-ingest".into();
            rec.confidence = 0.9;
            rec.entities = vec![ch.title.clone()];
            match st2.store.put(rec) {
                Ok(id) => ids.push(id),
                Err(_) => errs += 1,
            }
        }
        (ids, errs)
    })
    .await;
    match res {
        Ok((ids, errs)) => (StatusCode::OK, Json(json!({"ok": true, "ingested": ids.len(), "failed": errs, "ids": ids}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("任务失败: {e}")}))),
    }
}

#[derive(Deserialize)]
struct AskReq {
    question: String,
    #[serde(default = "default_ask_k")]
    top_k: usize,
    #[serde(default)]
    scene: Option<String>,
}

fn default_ask_k() -> usize {
    10
}

/// 智能问答：混合检索 → LLM 蒸馏推理（retrieve_and_reason）→ 生成。
/// 两段式对标 PlugMem：先让 LLM 把检索片段蒸馏为紧凑证据（降 token、提准确），
/// 再基于蒸馏证据作答。LLM 不可用时返回纯检索结果。
async fn ask_memory(State(st): State<Arc<AppState>>, Json(req): Json<AskReq>) -> impl IntoResponse {
    let t0 = std::time::Instant::now();
    // 查询扩展：LLM 生成同义/相关检索词（弥补词汇鸿沟，如"借东风"↔"祭风"）
    let judge0 = st.judge.clone();
    let q0 = req.question.clone();
    let expanded = tokio::task::spawn_blocking(move || {
        judge0.complete(
            "为检索系统扩展这个问题：输出与问题语义相关的关键词（同义词、别称、相关事件名），空格分隔，不要解释。",
            &q0,
        )
        .map(|r| format!("{} {}", q0, r))
        .unwrap_or_else(|| q0.clone())
    })
    .await
    .unwrap_or_else(|_| req.question.clone());
    let mut p = lymem_core::retrieval::SearchParams::new(expanded);
    p.top_k = req.top_k.clamp(10, 20); // RRF 融合下小 top_k 会挤掉单通道命中（如借东风段落仅向量通道召回）
    if let Some(sc) = &req.scene {
        p.scenes = Some(vec![sc.clone()]);
    }
    let hits = match st.store.search(&p) {
        Ok(h) => h,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    };
    let retrieve_ms = t0.elapsed().as_millis();
    let mut contexts: Vec<String> = hits
        .iter()
        .enumerate()
        .map(|(hi, h)| {
            // 前 3 条命中记录带全文（跨块答案需要完整章节）；其余用命中片段
            // 命中块级 snippet（chunk 文本，约 480 字）：紧凑且直指相关段落
            let body = if h.snippet.is_empty() {
                h.record.content.chars().take(800).collect::<String>()
            } else {
                h.snippet.clone()
            };
            if h.record.title.is_empty() {
                body
            } else {
                format!("（出处：{}）\n{}", h.record.title, body)
            }
        })
        .collect();
    // 块级向量补充：RRF 记录聚合会丢失"仅单通道强命中"的块（如文学作品中
    // 与问句无词面交集的情节段），此处把向量 KNN 的 top 块直接并入上下文
    if p.use_vec {
        let st2 = Arc::clone(&st);
        let qvec = p.query.clone();
        let knn_res = tokio::task::spawn_blocking(move || {
            let qv = st2.store.embedder.embed_one(&qvec)?;
            let mut ctx = Vec::new();
            for (chunk_id, dist, _mid) in st2.store.knn(&qv, 8)? {
                if dist > 1.2 {
                    continue; // 过滤完全不相关块
                }
                if let Some(Some(content)) = Some(st2.store.chunk_content(chunk_id)?) {
                    ctx.push(format!("（向量命中）\n{}", content));
                }
            }
            Ok::<Vec<String>, lymem_core::CoreError>(ctx)
        })
        .await;
        if let Ok(Ok(extra)) = knn_res {
            contexts.extend(extra);
        }
    }
    let judge = st.judge.clone();
    let question = req.question.clone();
    let ctx_join = contexts.iter().enumerate().map(|(i, c)| format!("[{}] {}", i + 1, c)).collect::<Vec<_>>().join("\n\n");
    let gen = tokio::task::spawn_blocking(move || {
        // 一段式直答：上下文为片段级（已紧凑），蒸馏预过滤反而会丢关键证据
        judge.complete(
            "你是知识问答助手。仅依据提供的记忆片段回答问题；片段不足以确定答案时回答：根据现有记忆无法确定。回答末尾用 [n] 标注引用的记忆编号。",
            &format!("记忆片段：\n{}\n\n问题：{}", ctx_join, question),
        )
    })
    .await;
    let answer = match gen {
        Ok(Some(a)) => a,
        _ => String::new(),
    };
    (
        StatusCode::OK,
        Json(json!({
            "question": req.question,
            "retrieve_ms": retrieve_ms,
            "answer": answer,
            "sources": hits.iter().map(|h| json!({
                "id": h.record.id, "title": h.record.title, "score": h.final_score,
                "snippet": h.snippet.chars().take(120).collect::<String>(),
            })).collect::<Vec<_>>(),
        })),
    )
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
) -> axum::response::Response {
    let upstream = std::env::var("LYMEM_LLM_UPSTREAM")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://api.minimaxi.com/v1".into());
    let server_key = std::env::var("LYMEM_LLM_API_KEY").unwrap_or_default();
    if server_key.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": {"message": "服务端未配置 LYMEM_LLM_API_KEY"}}))).into_response();
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

    let want_stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let url = format!("{}/chat/completions", upstream.trim_end_matches('/'));
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(600)).build().unwrap();
    match client.post(&url).bearer_auth(&api_key).json(&body).send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            if want_stream {
                // SSE 透传：DSH 等 agent 框架默认流式调用
                use axum::body::Body;
                use futures_util::StreamExt;
                let ct = resp
                    .headers()
                    .get("content-type")
                    .cloned()
                    .unwrap_or_else(|| "text/event-stream".parse().unwrap());
                let stream = resp.bytes_stream().map(|r| r.map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::Other, e)
                }));
                let mut resp = axum::response::Response::new(Body::from_stream(stream));
                resp.headers_mut().insert("content-type", ct);
                resp
            } else {
                let txt = resp.text().await.unwrap_or_default();
                let json_out: Value = serde_json::from_str(&txt).unwrap_or(json!({"raw": txt}));
                (status, Json(json_out)).into_response()
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": {"message": format!("上游请求失败: {e}")}})),
        )
            .into_response(),
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
