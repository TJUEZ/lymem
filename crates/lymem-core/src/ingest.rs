//! 多源数据接入：工具执行结果 / 用户行为 / 手动配置 / 会话轮次（赛题条款 1）。
//!
//! 统一清洗（空白规范化、长度上限、去重指纹）→ 敏感识别（分级）→
//! 实体抽取 → 入库（含向量/全文/图边）→ 偏好快通道联动。

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::model::{MemoryKind, MemoryRecord, Sensitivity, Tier};
use crate::sensitive;
use crate::store::MemoryStore;

/// 多源输入事件
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum IngestEvent {
    /// 工具调用执行结果
    ToolResult {
        tool: String,
        task: String,
        #[serde(default)]
        args: serde_json::Value,
        ok: bool,
        #[serde(default)]
        output: String,
        #[serde(default)]
        scene: String,
        #[serde(default)]
        source: String,
    },
    /// 用户行为事件（跨场景行为数据）
    UserBehavior {
        event: String,
        #[serde(default)]
        detail: String,
        #[serde(default)]
        scene: String,
        #[serde(default)]
        source: String,
    },
    /// 手动配置
    ManualConfig {
        key: String,
        value: serde_json::Value,
        #[serde(default)]
        scene: String,
        #[serde(default)]
        source: String,
    },
    /// 会话轮次
    Conversation {
        role: String,
        text: String,
        #[serde(default)]
        scene: String,
        #[serde(default)]
        source: String,
        /// 附加元数据（评测用：dia_id 等）
        #[serde(default)]
        meta: Option<serde_json::Value>,
        /// 窗口上下文（邻近轮次+会话日期，检索召回增强；正式检索路径）
        #[serde(default)]
        window_context: Option<String>,
    },
}

/// 清洗结果
pub struct Cleaned {
    pub title: String,
    pub content: String,
    pub kind: MemoryKind,
    pub meta: serde_json::Value,
    pub scene: String,
    pub source: String,
    pub sensitivity: Sensitivity,
    pub fingerprint: String,
}

/// 内容规范化：折叠空白、去控制字符、限长
pub fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_ws = false;
    for c in text.chars() {
        if c.is_control() && c != '\n' && c != '\t' {
            continue;
        }
        if c.is_whitespace() && c != '\n' {
            if !last_ws {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(c);
            last_ws = false;
        }
    }
    let trimmed: String = out.trim().to_string();
    // 行首尾空格规范化
    let trimmed = trimmed
        .split('\n')
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join("\n");
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() > 8000 {
        chars[..8000].iter().collect()
    } else {
        trimmed
    }
}

/// 简易指纹（去重）：内容前 512 字符的 FNV 哈希
pub fn fingerprint(text: &str) -> String {
    let head: String = text.chars().take(512).collect();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in head.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// 实体抽取（轻量规则，无外置模型）：
/// - 书名号/引号包裹的词；
/// - 反引号/路径中的标识符；
/// - 配置键（xxx.yyy 形式）。
pub fn extract_entities(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    // 引号主题：仅保留含 CJK 的（排除 JSON 键等 ASCII 短串）
    out.extend(
        sensitive::scan_free_quoted(text)
            .into_iter()
            .filter(|q| q.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))),
    );
    // 反引号 `code`
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            if let Some(end) = text[i + 1..].find('`') {
                let seg = &text[i + 1..i + 1 + end];
                let t = seg.trim();
                if t.chars().count() >= 2 && t.chars().count() <= 48 && !t.contains(char::is_whitespace) {
                    if let Some(p) = out.iter().position(|x| x == t) {
                        let _ = p;
                    } else {
                        out.push(t.to_string());
                    }
                }
                i = i + 1 + end + 1;
                continue;
            }
        }
        i += 1;
    }
    // 配置键形态 a.b.c
    let mut j = 0usize;
    let b2 = text.as_bytes();
    while j < b2.len() {
        if b2[j].is_ascii_lowercase() || b2[j] == b'_' {
            let mut k = j;
            let mut dots = 0;
            while k < b2.len() && (b2[k].is_ascii_lowercase() || b2[k].is_ascii_digit() || b2[k] == b'_' || b2[k] == b'.') {
                if b2[k] == b'.' {
                    dots += 1;
                }
                k += 1;
            }
            if dots >= 1 && k - j >= 5 && k - j <= 48 {
                let seg = &text[j..k];
                if !out.iter().any(|x| x == seg) {
                    out.push(seg.to_string());
                }
            }
            j = k;
        } else {
            j += 1;
        }
    }
    out.truncate(12);
    out
}

impl IngestEvent {
    /// 清洗 + 标准化 + 质量校验
    pub fn clean(&self) -> Cleaned {
        match self {
            IngestEvent::ToolResult { tool, task, args, ok, output, scene, source } => {
                let content = clean_text(&format!(
                    "工具 `{tool}` 执行{}。任务：{task}。参数：{}。输出：{}",
                    if *ok { "成功" } else { "失败" },
                    serde_json::to_string(args).unwrap_or_default(),
                    output
                ));
                let findings = sensitive::scan(&content);
                let level = sensitive::level_of(&findings);
                let content = if level != Sensitivity::None { sensitive::redact(&content) } else { content };
                let fp = fingerprint(&content);
                Cleaned {
                    title: format!("工具调用 {tool}"),
                    content,
                    kind: MemoryKind::ToolResult,
                    meta: serde_json::json!({"tool": tool, "task": task, "ok": ok, "args": args}),
                    scene: if scene.is_empty() { "general".into() } else { scene.clone() },
                    source: if source.is_empty() { "agent".into() } else { source.clone() },
                    sensitivity: level,
                    fingerprint: fp,
                }
            }
            IngestEvent::UserBehavior { event, detail, scene, source } => {
                let content = clean_text(&format!("用户行为 {event}：{detail}"));
                let findings = sensitive::scan(&content);
                let level = sensitive::level_of(&findings);
                let content = if level != Sensitivity::None { sensitive::redact(&content) } else { content };
                let fp = fingerprint(&content);
                Cleaned {
                    title: format!("行为 {event}"),
                    content,
                    kind: MemoryKind::UserBehavior,
                    meta: serde_json::json!({"event": event}),
                    scene: if scene.is_empty() { "general".into() } else { scene.clone() },
                    source: if source.is_empty() { "system".into() } else { source.clone() },
                    sensitivity: level,
                    fingerprint: fp,
                }
            }
            IngestEvent::ManualConfig { key, value, scene, source } => {
                let content = clean_text(&format!("手动配置 {key} = {value}"));
                let fp = fingerprint(&content);
                Cleaned {
                    title: format!("配置 {key}"),
                    content,
                    kind: MemoryKind::ManualConfig,
                    meta: serde_json::json!({"key": key, "value": value}),
                    scene: if scene.is_empty() { "general".into() } else { scene.clone() },
                    source: if source.is_empty() { "manual".into() } else { source.clone() },
                    sensitivity: Sensitivity::None,
                    fingerprint: fp,
                }
            }
            IngestEvent::Conversation { role, text, scene, source, meta, window_context } => {
                // 窗口上下文：邻近轮次+会话日期作内容前缀（检索召回增强）
                let text = match &window_context {
                    Some(wc) if !wc.is_empty() => format!("{wc}\n{text}"),
                    _ => text.clone(),
                };
                let content = clean_text(&format!("[{role}] {text}"));
                let findings = sensitive::scan(&content);
                let level = sensitive::level_of(&findings);
                let content = if level != Sensitivity::None { sensitive::redact(&content) } else { content };
                let fp = fingerprint(&content);
                let mut m = serde_json::json!({"role": role});
                if let Some(extra) = meta {
                    if let (Some(dst), Some(src)) = (m.as_object_mut(), extra.as_object()) {
                        for (k, v) in src {
                            dst.insert(k.clone(), v.clone());
                        }
                    }
                }
                Cleaned {
                    title: format!("会话 {role}"),
                    content,
                    kind: MemoryKind::Conversation,
                    meta: m,
                    scene: if scene.is_empty() { "general".into() } else { scene.clone() },
                    source: if source.is_empty() { "agent".into() } else { source.clone() },
                    sensitivity: level,
                    fingerprint: fp,
                }
            }
        }
    }
}

/// 接入层：事件 → 记忆库。返回入库 id 列表。
/// - 敏感 Blocked 拒绝入库（仅审计）；
/// - 重复指纹跳过（短窗口去重）；
/// - 实体/图边自动登记；
/// - ManualConfig 同时写入偏好版本链（source=manual，置信度 1.0）。
pub fn ingest_events(store: &MemoryStore, events: &[IngestEvent]) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    // 去重窗口：最近 200 条指纹
    let recent = store.list(None, None, false, 200)?;
    let mut seen: std::collections::HashSet<String> = recent
        .iter()
        .map(|r| fingerprint(&format!("{}|{}", r.title, r.content)))
        .collect();

    for ev in events {
        let c = ev.clean();
        if !seen.insert(c.fingerprint.clone()) {
            tracing::debug!("重复事件跳过: {}", c.title);
            continue;
        }
        if c.sensitivity == Sensitivity::Blocked {
            store.audit("ingest_blocked", &serde_json::json!({"title": c.title, "fingerprint": c.fingerprint}))?;
            tracing::warn!("阻断级敏感内容拒绝入库: {}", c.title);
            continue;
        }
        let entities = extract_entities(&c.content);
        let mut rec = MemoryRecord::new(Tier::Episodic, c.kind, c.title.clone(), c.content.clone());
        rec.source = c.source.clone();
        rec.scene = c.scene.clone();
        rec.sensitivity = c.sensitivity;
        rec.meta = c.meta.clone();
        rec.entities = entities.clone();

        // 手动配置直接提升为偏好
        if c.kind == MemoryKind::ManualConfig {
            rec.tier = Tier::Preference;
            let key = c.meta.get("key").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let value = c.meta.get("value").cloned().unwrap_or(serde_json::Value::Null);
            if !key.is_empty() {
                store.set_preference(&crate::preference::PreferenceSet {
                    key,
                    value,
                    evidence: vec![serde_json::json!({"fingerprint": c.fingerprint})],
                    source: "manual".into(),
                    confidence: 1.0,
                    scenes: vec![if c.scene.is_empty() { "*".to_string() } else { c.scene.clone() }],
                    force: true,
                })?;
            }
        }

        let id = store.put(rec)?;
        ids.push(id);

        // 实体与图边登记
        let mut ent_ids = Vec::new();
        for name in &entities {
            let eid = store.upsert_entity(name, "thing")?;
            ent_ids.push(eid);
        }
        for w in ent_ids.windows(2) {
            store.upsert_edge(w[0], w[1], "co", Some(id))?;
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashEmbedder;
    use crate::retrieval::SearchParams;

    fn store() -> MemoryStore {
        let dir = tempfile::tempdir().unwrap().into_path();
        MemoryStore::open(&dir.join("m.db"), &dir.join("fts"), Box::new(HashEmbedder::new(64))).unwrap()
    }

    #[test]
    fn test_ingest_tool_result_and_pref() {
        let s = store();
        let ids = ingest_events(
            &s,
            &[
                IngestEvent::ToolResult {
                    tool: "libreoffice".into(),
                    task: "doc_edit".into(),
                    args: serde_json::json!({"file": "a.docx"}),
                    ok: true,
                    output: "导出 pdf 成功".into(),
                    scene: "office".into(),
                    source: String::new(),
                },
                IngestEvent::ManualConfig {
                    key: "output_style.language".into(),
                    value: serde_json::json!("中文"),
                    scene: String::new(),
                    source: String::new(),
                },
            ],
        )
        .unwrap();
        assert_eq!(ids.len(), 2);
        // 配置已入偏好链
        let prefs = s.effective_preferences(None).unwrap();
        assert!(prefs.iter().any(|p| p.key == "output_style.language" && p.value == serde_json::json!("中文")));
        // 工具结果可检索
        let hits = s.search(&SearchParams::new("libreoffice pdf")).unwrap();
        assert!(!hits.is_empty());
    }

    #[test]
    fn test_dedup_and_blocked() {
        let s = store();
        let ev = IngestEvent::Conversation {
            role: "user".into(),
            text: "帮我记住这个 API key：sk-abcdefghijklmnopqrst".into(),
            scene: String::new(),
            source: String::new(),
            meta: None,
            window_context: None,
        };
        let ids = ingest_events(&s, &[ev.clone()]).unwrap();
        assert!(ids.is_empty(), "密钥应被阻断");
        let ev2 = IngestEvent::Conversation {
            role: "user".into(),
            text: "今天聊了《银河麒麟》的开发计划".into(),
            scene: String::new(),
            source: String::new(),
            meta: None,
            window_context: None,
        };
        let ids2 = ingest_events(&s, &[ev2.clone(), ev2]).unwrap();
        assert_eq!(ids2.len(), 1, "重复应去重");
        // 实体抽取
        let rec = s.get(ids2[0]).unwrap().unwrap();
        assert!(rec.entities.contains(&"银河麒麟".to_string()));
    }

    #[test]
    fn test_extract_config_key() {
        let e = extract_entities("配置 deploy.path 指向 `/opt/apps`，端口 8080");
        eprintln!("extract={e:?}");
        assert!(e.contains(&"deploy.path".to_string()), "配置键: {e:?}");
        assert!(e.contains(&"/opt/apps".to_string()), "反引号路径: {e:?}");
    }

    #[test]
    fn test_clean_text() {
        assert_eq!(clean_text("  a\t\tb  \n c  "), "a b\nc");
        let long = "x".repeat(9000);
        assert!(clean_text(&long).chars().count() <= 8000);
    }
}
