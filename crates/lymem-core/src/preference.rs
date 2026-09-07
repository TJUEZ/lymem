//! 偏好记忆：双通道提取 + 版本时间线 + 跨场景适配（赛题条款 2）。
//!
//! - 快通道（规则，纯 Rust）：从工具调用结果/行为事件统计工具选择、输出格式偏好；
//! - 慢通道（LLM，可选）：从文本证据提取结构化偏好增量；
//! - 版本化：每次变更追加 pref_versions 节点，支持时间点回溯与场景级生效。

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::llm_hook::LlmJudge;
use crate::model::MemoryKind;
use crate::store::MemoryStore;

/// 一个偏好键的当前视图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreferenceView {
    pub key: String,
    pub version: i64,
    /// 当前生效值（JSON）
    pub value: serde_json::Value,
    /// 证据（来源记忆 id / 描述）
    pub evidence: Vec<serde_json::Value>,
    /// 来源：rule / llm / manual / behavior
    pub source: String,
    pub confidence: f64,
    /// 生效场景列表（"*" 为全局）
    pub scenes: Vec<String>,
    pub valid_from: i64,
    pub valid_to: Option<i64>,
}

/// 偏好写入请求
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PreferenceSet {
    pub key: String,
    pub value: serde_json::Value,
    #[serde(default)]
    pub evidence: Vec<serde_json::Value>,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_conf")]
    pub confidence: f64,
    #[serde(default = "default_scenes")]
    pub scenes: Vec<String>,
    /// 是否覆盖更高置信度的既有版本（默认：置信度不低于当前版本才覆盖）
    #[serde(default)]
    pub force: bool,
}

fn default_source() -> String {
    "manual".into()
}
fn default_conf() -> f64 {
    0.8
}
fn default_scenes() -> Vec<String> {
    vec!["*".into()]
}

impl MemoryStore {
    /// 写入新的偏好版本（追加版本链）
    pub fn set_preference(&self, p: &PreferenceSet) -> Result<PreferenceView> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO preferences(key, description) VALUES (?1, '') ON CONFLICT(key) DO NOTHING",
            params![p.key],
        )?;
        let pref_id: i64 = conn.query_row("SELECT id FROM preferences WHERE key = ?1", params![p.key], |r| r.get(0))?;
        let cur_ver: i64 =
            conn.query_row("SELECT current_version FROM preferences WHERE id = ?1", params![pref_id], |r| r.get(0))?;
        let now = chrono::Utc::now().timestamp();
        let new_ver = cur_ver + 1;
        // 关闭旧版本
        conn.execute(
            "UPDATE pref_versions SET valid_to = ?3, superseded_by = ?4 WHERE pref_id = ?1 AND version = ?2",
            params![pref_id, cur_ver, now, new_ver],
        )?;
        conn.execute(
            "INSERT INTO pref_versions(pref_id, version, value, evidence, source, confidence, scenes, valid_from)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                pref_id,
                new_ver,
                p.value.to_string(),
                serde_json::to_string(&p.evidence).unwrap_or_else(|_| "[]".into()),
                p.source,
                p.confidence,
                serde_json::to_string(&p.scenes).unwrap_or_else(|_| "[\"*\"]".into()),
                now
            ],
        )?;
        conn.execute("UPDATE preferences SET current_version = ?2 WHERE id = ?1", params![pref_id, new_ver])?;
        drop(conn);
        self.audit("pref_set", &serde_json::json!({"key": p.key, "version": new_ver, "source": p.source}))?;
        Ok(PreferenceView {
            key: p.key.clone(),
            version: new_ver,
            value: p.value.clone(),
            evidence: p.evidence.clone(),
            source: p.source.clone(),
            confidence: p.confidence,
            scenes: p.scenes.clone(),
            valid_from: now,
            valid_to: None,
        })
    }

    fn row_to_view(key: &str, row: &rusqlite::Row) -> rusqlite::Result<PreferenceView> {
        Ok(PreferenceView {
            key: key.to_string(),
            version: row.get("version")?,
            value: serde_json::from_str(&row.get::<_, String>("value")?).unwrap_or(serde_json::Value::Null),
            evidence: serde_json::from_str(&row.get::<_, String>("evidence")?).unwrap_or_default(),
            source: row.get("source")?,
            confidence: row.get("confidence")?,
            scenes: serde_json::from_str(&row.get::<_, String>("scenes")?).unwrap_or_else(|_| vec!["*".into()]),
            valid_from: row.get("valid_from")?,
            valid_to: row.get("valid_to")?,
        })
    }

    /// 当前生效的所有偏好
    pub fn effective_preferences(&self, scene: Option<&str>) -> Result<Vec<PreferenceView>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT p.key, v.version, v.value, v.evidence, v.source, v.confidence, v.scenes, v.valid_from, v.valid_to
             FROM preferences p JOIN pref_versions v ON v.pref_id = p.id AND v.version = p.current_version",
        )?;
        let rows = stmt.query_map([], |row| {
            let key: String = row.get(0)?;
            Self::row_to_view(&key, row)
        })?;
        Ok(rows
            .filter_map(|r| r.ok())
            .filter(|v| scene.is_none_or(|s| v.scenes.iter().any(|sc| sc == "*" || sc == s)))
            .collect())
    }

    /// 某键的完整版本历史（回溯）
    pub fn preference_history(&self, key: &str) -> Result<Vec<PreferenceView>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT p.key, v.version, v.value, v.evidence, v.source, v.confidence, v.scenes, v.valid_from, v.valid_to
             FROM preferences p JOIN pref_versions v ON v.pref_id = p.id
             WHERE p.key = ?1 ORDER BY v.version ASC",
        )?;
        let rows = stmt.query_map(params![key], |row| {
            let k: String = row.get(0)?;
            Self::row_to_view(&k, row)
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 时间点回溯：返回某时刻生效的版本
    pub fn preference_at(&self, key: &str, at: i64) -> Result<Option<PreferenceView>> {
        Ok(self
            .preference_history(key)?
            .into_iter()
            .find(|v| v.valid_from <= at && v.valid_to.is_none_or(|t| at <= t)))
    }

    /// 全部键在某时刻的生效偏好（时间机器：拖动时点看偏好全貌）。
    /// 同键多个版本落在时点窗口内时取最新版本。
    pub fn effective_preferences_at(&self, at: i64) -> Result<Vec<PreferenceView>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT p.key, v.version, v.value, v.evidence, v.source, v.confidence, v.scenes, v.valid_from, v.valid_to
             FROM pref_versions v JOIN preferences p ON p.id = v.pref_id
             WHERE v.valid_from <= ?1 AND (v.valid_to IS NULL OR v.valid_to > ?1)
               AND v.version = (SELECT MAX(v2.version) FROM pref_versions v2
                                WHERE v2.pref_id = v.pref_id AND v2.valid_from <= ?1
                                  AND (v2.valid_to IS NULL OR v2.valid_to > ?1))
             ORDER BY p.key ASC",
        )?;
        let rows = stmt.query_map(params![at], |row| {
            let k: String = row.get(0)?;
            Self::row_to_view(&k, row)
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 全部键的全部版本（时间机器滑块域与版本浏览）。
    pub fn preferences_history_all(&self) -> Result<Vec<PreferenceView>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT p.key, v.version, v.value, v.evidence, v.source, v.confidence, v.scenes, v.valid_from, v.valid_to
             FROM pref_versions v JOIN preferences p ON p.id = v.pref_id
             ORDER BY p.key ASC, v.version ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let k: String = row.get(0)?;
            Self::row_to_view(&k, row)
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 偏好规则快通道：统计最近的工具调用记录，产出工具选择偏好。
    /// 阈值：同一任务类目下某工具占比 ≥ share 且调用次数 ≥ min_count 时更新偏好。
    pub fn preference_rules_from_tools(&self, lookback: usize, min_count: usize, share: f64) -> Result<Vec<PreferenceView>> {
        let tools = self.recent_tool_results(lookback)?;
        // 按任务类目（meta.task）聚合工具使用
        let mut by_task: std::collections::HashMap<String, std::collections::HashMap<String, (usize, Vec<i64>)>> =
            std::collections::HashMap::new();
        for (id, task, tool) in tools {
            let m = by_task.entry(task).or_default();
            let e = m.entry(tool).or_insert((0, vec![]));
            e.0 += 1;
            e.1.push(id);
        }
        let mut updates = Vec::new();
        for (task, tools_map) in by_task {
            let total: usize = tools_map.values().map(|(c, _)| c).sum();
            let mut ranked: Vec<_> = tools_map.into_iter().collect();
            ranked.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
            if let Some((tool, (count, ids))) = ranked.into_iter().next() {
                if total >= min_count && count as f64 / total as f64 >= share {
                    let key = format!("tool_choice.{task}");
                    // 仅当与当前版本不同才更新
                    let cur = self.effective_preferences(None)?.into_iter().find(|v| v.key == key);
                    let same = cur.is_some_and(|v| v.value == serde_json::json!({"tool": tool}));
                    if !same {
                        let ev: Vec<serde_json::Value> = ids.iter().map(|i| serde_json::json!({"memory_id": i})).collect();
                        updates.push(self.set_preference(&PreferenceSet {
                            key,
                            value: serde_json::json!({"tool": tool}),
                            evidence: ev,
                            source: "rule".into(),
                            confidence: (count as f64 / total as f64).min(0.99),
                            scenes: vec!["*".into()],
                            force: false,
                        })?);
                    }
                }
            }
        }
        Ok(updates)
    }

    /// 慢通道：LLM 从一段会话/工具结果文本中提取偏好增量。
    /// 无 LLM 或解析失败时返回空（规则通道兜底）。
    pub fn preference_llm_extract(&self, judge: &dyn LlmJudge, text: &str, scene: &str) -> Result<Vec<PreferenceSet>> {
        let Some(out) = judge.complete(
            "你是 OS Agent 的偏好提取器。从给定文本中提取用户偏好增量，\
             以 JSON 数组输出，每项形如 {\"key\":\"类别.名称\",\"value\":...,\"evidence\":\"原文摘录\",\
             \"confidence\":0~1,\"scenes\":[\"场景\"]}。类别限定：tool_choice/output_style/security/app/general。\
             没有可靠偏好时输出 []。只输出 JSON。",
            text,
        ) else {
            return Ok(Vec::new());
        };
        let trimmed = out.trim().trim_start_matches("```json").trim_end_matches("```").trim();
        let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => return Ok(Vec::new()),
        };
        let arr = match parsed.as_array() {
            Some(a) => a.clone(),
            None => return Ok(Vec::new()),
        };
        let mut sets = Vec::new();
        for item in arr {
            let key = item.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if key.is_empty() {
                continue;
            }
            let value = item.get("value").cloned().unwrap_or(serde_json::Value::Null);
            if value.is_null() {
                continue;
            }
            let evidence = item
                .get("evidence")
                .map(|e| vec![serde_json::json!({"text": e, "scene": scene})])
                .unwrap_or_default();
            let scenes = item
                .get("scenes")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                .unwrap_or_else(|| vec![scene.to_string()]);
            sets.push(PreferenceSet {
                key,
                value,
                evidence,
                source: "llm".into(),
                confidence: item.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.6).clamp(0.0, 1.0),
                scenes,
                force: false,
            });
        }
        Ok(sets)
    }

    /// 近期工具调用记录：(memory_id, task 类目, tool 名)
    pub fn recent_tool_results(&self, limit: usize) -> Result<Vec<(i64, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, meta FROM memories WHERE kind = ?1 AND tombstone = 0 ORDER BY id DESC LIMIT ?2")?;
        let rows = stmt.query_map(params![MemoryKind::ToolResult.as_str(), limit as i64], |row| {
            let id: i64 = row.get(0)?;
            let meta: String = row.get(1)?;
            Ok((id, meta))
        })?;
        Ok(rows
            .filter_map(|r| r.ok())
            .filter_map(|(id, meta)| {
                let v: serde_json::Value = serde_json::from_str(&meta).ok()?;
                let task = v.get("task").and_then(|x| x.as_str()).unwrap_or("general").to_string();
                let tool = v.get("tool").and_then(|x| x.as_str())?.to_string();
                Some((id, task, tool))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashEmbedder;
    use crate::model::{MemoryRecord, Tier};

    fn store() -> MemoryStore {
        let dir = tempfile::tempdir().unwrap().into_path();
        MemoryStore::open(&dir.join("m.db"), &dir.join("fts"), Box::new(HashEmbedder::new(64))).unwrap()
    }

    #[test]
    fn test_version_chain_and_history() {
        let s = store();
        s.set_preference(&PreferenceSet {
            key: "output_style.language".into(),
            value: serde_json::json!("中文"),
            source: "manual".into(),
            confidence: 1.0,
            ..Default::default()
        })
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let v2 = s.set_preference(&PreferenceSet {
            key: "output_style.language".into(),
            value: serde_json::json!("English"),
            source: "behavior".into(),
            confidence: 0.9,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(v2.version, 2);
        let hist = s.preference_history("output_style.language").unwrap();
        assert_eq!(hist.len(), 2);
        assert!(hist[0].valid_to.is_some());
        assert!(hist[1].valid_to.is_none());
        let cur = s.effective_preferences(None).unwrap();
        assert_eq!(cur[0].value, serde_json::json!("English"));
        // 时间点回溯
        let old = s.preference_at("output_style.language", hist[0].valid_from).unwrap().unwrap();
        assert_eq!(old.value, serde_json::json!("中文"));
    }

    #[test]
    fn test_rule_channel() {
        let s = store();
        for i in 0..5 {
            let mut r = MemoryRecord::new(Tier::Episodic, MemoryKind::ToolResult, "工具调用", format!("第{i}次编辑文档"));
            r.meta = serde_json::json!({"task": "doc_edit", "tool": "libreoffice", "ok": true});
            s.put(r).unwrap();
        }
        let mut r = MemoryRecord::new(Tier::Episodic, MemoryKind::ToolResult, "工具调用", "一次用别的");
        r.meta = serde_json::json!({"task": "doc_edit", "tool": "wps", "ok": true});
        s.put(r).unwrap();
        let updates = s.preference_rules_from_tools(100, 3, 0.6).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].key, "tool_choice.doc_edit");
        assert_eq!(updates[0].value, serde_json::json!({"tool": "libreoffice"}));
    }
}
