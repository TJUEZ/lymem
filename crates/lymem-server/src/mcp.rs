//! MCP（Model Context Protocol）Streamable HTTP 端点。
//!
//! 供 deepseek-harness（dsh --patch lymem.cordis.yml）、opencode 等
//! agent 接入 lymem 记忆模块。无会话状态实现：每个 POST 独立处理，
//! 支持 initialize / tools/list / tools/call（JSON-RPC 2.0）。

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::AppState;

pub async fn mcp_post(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(msg): Json<Value>,
) -> axum::response::Response {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));

    // 已声明的协议版本回显，否则用较新的稳定版
    let client_proto = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("2025-06-18")
        .to_string();

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": client_proto,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "lymem",
                "title": "麟忆尽智 · OS Agent 记忆模块",
                "version": env!("CARGO_PKG_VERSION"),
            }
        })),
        "notifications/initialized" | "notifications/cancelled" => {
            // 通知：无响应体
            return (StatusCode::ACCEPTED, axum::response::Html("")).into_response();
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools_spec() })),
        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            dispatch(&st, name, &args).await
        }
        other => Err(format!("未知方法: {other}")),
    };

    let _ = headers;
    match result {
        Ok(r) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            Json(json!({ "jsonrpc": "2.0", "id": id, "result": r })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            Json(json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": e }
            })),
        )
            .into_response(),
    }
}

fn tools_spec() -> Value {
    json!([
        {
            "name": "memory_search",
            "description": "混合检索长期记忆（向量+BM25+图三通道 RRF 融合）。agent 每轮应先调用以注入相关记忆。",
            "inputSchema": { "type": "object", "properties": {
                "query": { "type": "string", "description": "检索问题/主题" },
                "top_k": { "type": "integer", "default": 5 },
                "scene": { "type": "string", "description": "场景过滤：coding/office/system 等" }
            }, "required": ["query"] }
        },
        {
            "name": "memory_add",
            "description": "写入一条记忆（多源：会话轮次/工具结果/行为/配置）。敏感信息自动识别脱敏，重复自动去重。",
            "inputSchema": { "type": "object", "properties": {
                "text": { "type": "string", "description": "记忆内容" },
                "kind": { "type": "string", "enum": ["conversation", "tool_result", "user_behavior", "manual_config"],
                          "description": "记忆种类" },
                "tool": { "type": "string", "description": "kind=tool_result 时的工具名" },
                "task": { "type": "string", "description": "kind=tool_result 时的任务类目" },
                "ok": { "type": "boolean", "description": "工具执行是否成功" },
                "scene": { "type": "string", "default": "general" }
            }, "required": ["text"] }
        },
        {
            "name": "preference_list",
            "description": "列出当前生效的用户偏好（操作习惯/输出风格/安全策略，带版本与置信度）。回答问题前应参考以保持个性化一致。",
            "inputSchema": { "type": "object", "properties": {
                "scene": { "type": "string", "description": "按场景过滤" }
            } }
        },
        {
            "name": "memory_forget",
            "description": "自然语言遗忘：解析指令→预览命中→执行（默认墓碑软删，可硬清除）。",
            "inputSchema": { "type": "object", "properties": {
                "instruction": { "type": "string", "description": "如：忘掉关于项目部署路径的一切" },
                "mode": { "type": "string", "enum": ["preview", "tombstone", "purge"], "default": "preview" }
            }, "required": ["instruction"] }
        },
        {
            "name": "conflict_list",
            "description": "列出最近的知识冲突及仲裁结果（取代/合并/共存）。",
            "inputSchema": { "type": "object", "properties": { "limit": { "type": "integer", "default": 10 } } }
        },
        {
            "name": "memory_status",
            "description": "记忆库概况：各层条目数、偏好数、嵌入通道。",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

async fn dispatch(st: &Arc<AppState>, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "memory_search" => {
            let query = args.get("query").and_then(|v| v.as_str()).ok_or("缺少 query")?;
            let mut p = lymem_core::retrieval::SearchParams::new(query);
            p.top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
            if let Some(scene) = args.get("scene").and_then(|v| v.as_str()) {
                p.scenes = Some(vec![scene.to_string(), "*".to_string()]);
            }
            st.store
                .search(&p)
                .map(|hits| {
                    json!(hits
                        .iter()
                        .map(|h| json!({
                            "content": h.record.content,
                            "tier": h.record.tier.as_str(),
                            "kind": h.record.kind.as_str(),
                            "scene": h.record.scene,
                            "score": h.final_score,
                        }))
                        .collect::<Vec<_>>())
                })
                .map_err(|e| e.to_string())
        }
        "memory_add" => {
            let text = args.get("text").and_then(|v| v.as_str()).ok_or("缺少 text")?;
            let scene = args.get("scene").and_then(|v| v.as_str()).unwrap_or("general").to_string();
            let event = match args.get("kind").and_then(|v| v.as_str()) {
                Some("tool_result") => lymem_core::ingest::IngestEvent::ToolResult {
                    tool: args.get("tool").and_then(|v| v.as_str()).unwrap_or("unknown").into(),
                    task: args.get("task").and_then(|v| v.as_str()).unwrap_or("general").into(),
                    args: json!({}),
                    ok: args.get("ok").and_then(|v| v.as_bool()).unwrap_or(true),
                    output: text.into(),
                    scene,
                    source: "mcp".into(),
                },
                Some("user_behavior") => lymem_core::ingest::IngestEvent::UserBehavior {
                    event: "agent_observed".into(),
                    detail: text.into(),
                    scene,
                    source: "mcp".into(),
                },
                Some("manual_config") => lymem_core::ingest::IngestEvent::ManualConfig {
                    key: args.get("key").and_then(|v| v.as_str()).unwrap_or("app.config").into(),
                    value: json!(text),
                    scene,
                    source: "mcp".into(),
                },
                _ => lymem_core::ingest::IngestEvent::Conversation {
                    role: args.get("role").and_then(|v| v.as_str()).unwrap_or("user").into(),
                    text: text.into(),
                    scene,
                    source: "mcp".into(),
                    meta: None,
                },
            };
            lymem_core::ingest::ingest_events(&st.store, &[event])
                .map(|ids| json!({ "ok": true, "ids": ids }))
                .map_err(|e| e.to_string())
        }
        "preference_list" => {
            let scene = args.get("scene").and_then(|v| v.as_str());
            st.store
                .effective_preferences(scene)
                .map(|ps| json!(ps))
                .map_err(|e| e.to_string())
        }
        "memory_forget" => {
            let instr = args.get("instruction").and_then(|v| v.as_str()).ok_or("缺少 instruction")?;
            let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("preview");
            let scope = st
                .store
                .parse_forget_scope(instr, Some(st.judge.as_ref()))
                .map_err(|e| e.to_string())?;
            match mode {
                "preview" => st.store.forget_preview(&scope, 50).map(|t| json!({ "scope": scope, "targets": t })).map_err(|e| e.to_string()),
                "purge" => st
                    .store
                    .forget_execute(&scope, lymem_core::forget::ForgetMode::Purge)
                    .map(|n| json!({ "ok": true, "purged": n }))
                    .map_err(|e| e.to_string()),
                _ => st
                    .store
                    .forget_execute(&scope, lymem_core::forget::ForgetMode::Tombstone)
                    .map(|n| json!({ "ok": true, "tombstoned": n }))
                    .map_err(|e| e.to_string()),
            }
        }
        "conflict_list" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            st.store
                .conflicts_all()
                .map(|mut c| {
                    c.truncate(limit);
                    json!(c)
                })
                .map_err(|e| e.to_string())
        }
        "memory_status" => {
            let counts: std::collections::BTreeMap<String, i64> = {
                let conn = st.store.conn.lock().unwrap();
                let mut stmt = conn
                    .prepare("SELECT tier, COUNT(*) FROM memories WHERE tombstone = 0 AND is_current = 1 GROUP BY tier")
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                    .map_err(|e| e.to_string())?;
                rows.filter_map(|r| r.ok()).collect()
            };
            let prefs = st.store.effective_preferences(None).map(|p| p.len()).unwrap_or(0);
            Ok(json!({
                "tiers": counts, "preferences": prefs,
                "embedder": st.embedder_name, "embed_dim": st.store.vec_dim,
                "llm": st.llm_name,
            }))
        }
        other => Err(format!("未知工具: {other}")),
    }
}
