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
            "description": "混合检索长期记忆（向量+BM25+图三通道 RRF 融合）。agent 每轮应先调用以注入相关记忆。经验卡（做法/避坑）自动提权，命中时优先参考其 trigger 与要点。",
            "inputSchema": { "type": "object", "properties": {
                "query": { "type": "string", "description": "检索问题/主题" },
                "top_k": { "type": "integer", "default": 5 },
                "scene": { "type": "string", "description": "场景过滤：coding/office/system 等" }
            }, "required": ["query"] }
        },
        {
            "name": "memory_add",
            "description": "写入一条记忆（多源：会话轮次/工具结果/行为/配置；workflow/case/template 直接入知识层并走冲突仲裁）。敏感信息自动识别脱敏，重复自动去重。",
            "inputSchema": { "type": "object", "properties": {
                "text": { "type": "string", "description": "记忆内容" },
                "kind": { "type": "string", "enum": ["conversation", "tool_result", "user_behavior", "manual_config", "workflow", "case", "template"],
                          "description": "记忆种类；tool_result 请带 tool/task/ok 以便日后提炼经验" },
                "title": { "type": "string", "description": "workflow/case/template 的标题（可选，默认取 text 前 30 字）" },
                "tool": { "type": "string", "description": "kind=tool_result 时的工具名" },
                "task": { "type": "string", "description": "kind=tool_result 时的任务类目" },
                "ok": { "type": "boolean", "description": "工具执行是否成功" },
                "scene": { "type": "string", "default": "general" }
            }, "required": ["text"] }
        },
        {
            "name": "experience_list",
            "description": "列出已编译的经验卡（做法/避坑，带适用条件、要点、复用成败计数与 verified/draft 状态）。接到相似任务时应先参考。",
            "inputSchema": { "type": "object", "properties": {
                "status": { "type": "string", "enum": ["all", "draft", "verified"], "default": "all" }
            } }
        },
        {
            "name": "memory_feedback",
            "description": "经验采纳反馈：实际复用某条经验后回报成败。累计复用≥2 次且无失败会使其转为 verified（检索提权）；出现失败则回退 draft。应在完成任务后调用。",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "integer", "description": "经验卡的记忆 id" },
                "ok": { "type": "boolean", "description": "该经验是否帮你把任务做成了" },
                "scene": { "type": "string", "description": "任务场景（tool_result 轨迹用），便于后续经验提炼" },
                "task": { "type": "string", "description": "本次任务类目（tool_result 轨迹用）" },
                "tool": { "type": "string", "description": "本次使用的工具（tool_result 轨迹用）" },
                "text": { "type": "string", "description": "本次执行结果简述（tool_result 轨迹用，喂给经验提炼）" }
            }, "required": ["id", "ok"] }
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
                    let hits = crate::experience_boost(hits);
                    json!(hits
                        .iter()
                        .map(|h| {
                            let exp = lymem_core::experience::experience_of(&h.record)
                                .map(|e| json!({"status": e.status, "type": e.exp_type,
                                    "trigger": e.trigger, "steps": e.steps,
                                    "wins": e.outcome.wins, "fails": e.outcome.fails}));
                            json!({
                                "content": h.record.content,
                                "tier": h.record.tier.as_str(),
                                "kind": h.record.kind.as_str(),
                                "scene": h.record.scene,
                                "score": h.final_score,
                                "id": h.record.id,
                                "experience": exp,
                            })
                        })
                        .collect::<Vec<_>>())
                })
                .map_err(|e| e.to_string())
        }
        "memory_add" => {
            let text = args.get("text").and_then(|v| v.as_str()).ok_or("缺少 text")?;
            let scene = args.get("scene").and_then(|v| v.as_str()).unwrap_or("general").to_string();
            let kind_str = args.get("kind").and_then(|v| v.as_str()).unwrap_or("conversation");

            // workflow/case/template：直接入知识层并走冲突仲裁（新旧知识碰撞→取代/共存/合并）
            if matches!(kind_str, "workflow" | "case" | "template") {
                let kind = match kind_str {
                    "workflow" => lymem_core::model::MemoryKind::Workflow,
                    "case" => lymem_core::model::MemoryKind::Case,
                    _ => lymem_core::model::MemoryKind::Template,
                };
                let title = args
                    .get("title")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| text.chars().take(30).collect());
                let mut rec = lymem_core::model::MemoryRecord::new(lymem_core::model::Tier::Knowledge, kind, title.clone(), text);
                rec.scene = scene;
                rec.source = "mcp".into();
                rec.entities = args
                    .get("entities")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_else(|| vec![title.chars().take(12).collect()]);
                let st2 = Arc::clone(st);
                let judge = st.judge.clone();
                return tokio::task::spawn_blocking(move || {
                    st2.store
                        .put_knowledge_with_conflicts(rec, Some(judge.as_ref()), 0.8)
                        .map(|(id, conflicts)| {
                            json!({ "ok": true, "id": id,
                                "conflicts": conflicts.iter().map(|c| json!({
                                    "type": c.ctype.as_str(), "resolution": c.resolution.as_str(),
                                })).collect::<Vec<_>>() })
                        })
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| format!("任务失败: {e}"))?;
            }

            let event = match kind_str {
                "tool_result" => lymem_core::ingest::IngestEvent::ToolResult {
                    tool: args.get("tool").and_then(|v| v.as_str()).unwrap_or("unknown").into(),
                    task: args.get("task").and_then(|v| v.as_str()).unwrap_or("general").into(),
                    args: json!({}),
                    ok: args.get("ok").and_then(|v| v.as_bool()).unwrap_or(true),
                    output: text.into(),
                    scene,
                    source: "mcp".into(),
                },
                "user_behavior" => lymem_core::ingest::IngestEvent::UserBehavior {
                    event: "agent_observed".into(),
                    detail: text.into(),
                    scene,
                    source: "mcp".into(),
                },
                "manual_config" => lymem_core::ingest::IngestEvent::ManualConfig {
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
                    window_context: None,
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
        "experience_list" => {
            let filter = args.get("status").and_then(|v| v.as_str()).unwrap_or("all");
            st.store
                .list_experiences()
                .map(|cards| {
                    let cards: Vec<_> = cards
                        .into_iter()
                        .filter(|c| filter == "all" || c.status == filter)
                        .collect();
                    json!(cards)
                })
                .map_err(|e| e.to_string())
        }
        "memory_feedback" => {
            let id = args.get("id").and_then(|v| v.as_i64()).ok_or("缺少 id")?;
            let ok = args.get("ok").and_then(|v| v.as_bool()).ok_or("缺少 ok")?;
            // 顺带把这次复用落成工具轨迹，供下一轮经验提炼消费
            if let (Some(tool), Some(task), Some(text)) = (
                args.get("tool").and_then(|v| v.as_str()),
                args.get("task").and_then(|v| v.as_str()),
                args.get("text").and_then(|v| v.as_str()),
            ) {
                let ev = lymem_core::ingest::IngestEvent::ToolResult {
                    tool: tool.into(),
                    task: task.into(),
                    args: json!({ "experience_id": id }),
                    ok,
                    output: text.into(),
                    scene: args.get("scene").and_then(|v| v.as_str()).unwrap_or("general").into(),
                    source: "mcp".into(),
                };
                let _ = lymem_core::ingest::ingest_events(&st.store, &[ev]);
            }
            let _ = st.store.audit("mcp_experience_feedback", &json!({ "id": id, "ok": ok }));
            st.store
                .experience_feedback(id, ok)
                .map(|c| json!({ "ok": true, "card": c }))
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
