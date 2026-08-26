//! 遗忘模块：自然语言指令 → 结构化范围 → 预览 → 墓碑/硬清除（赛题条款 5）。
//!
//! 指令解析优先 LLM（范围更准），无 LLM 时用关键词规则兜底；
//! 执行分两档：墓碑软删（可审计可恢复）与硬清除（物理删除）。

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::llm_hook::LlmJudge;
use crate::model::MemoryRecord;
use crate::store::MemoryStore;

/// 遗忘范围：主体/主题/场景/时间窗四维
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ForgetScope {
    /// 主题关键词（匹配标题/内容/实体）
    pub topics: Vec<String>,
    /// 场景限定
    pub scenes: Vec<String>,
    /// 层级限定
    pub tiers: Vec<String>,
    /// 时间窗（unix 秒）
    pub after: Option<i64>,
    pub before: Option<i64>,
}

impl ForgetScope {
    pub fn is_empty(&self) -> bool {
        self.topics.is_empty() && self.scenes.is_empty() && self.tiers.is_empty() && self.after.is_none() && self.before.is_none()
    }
}

/// 遗忘执行模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForgetMode {
    /// 墓碑软删
    Tombstone,
    /// 硬清除
    Purge,
}

/// 命中的可遗忘目标
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgetTarget {
    pub id: i64,
    pub title: String,
    pub tier: String,
    pub scene: String,
    pub created_at: i64,
    pub matched_by: String,
}

impl MemoryStore {
    /// 解析自然语言遗忘指令为结构化范围。
    /// LLM 路径：让模型输出四维 JSON；规则路径：抽取"关于X"与常见场景词。
    pub fn parse_forget_scope(&self, instruction: &str, judge: Option<&dyn LlmJudge>) -> Result<ForgetScope> {
        if let Some(j) = judge {
            if let Some(out) = j.complete(
                "你是 OS Agent 的遗忘指令解析器。把用户的遗忘请求解析为 JSON：\
                 {\"topics\":[关键词],\"scenes\":[场景],\"tiers\":[\"episodic|knowledge|preference\"],\
                 \"after\":unix秒或null,\"before\":unix秒或null}。\
                 scenes 常用值：coding/office/system/general。不确定的字段留空/留 null。只输出 JSON。",
                instruction,
            ) {
                let t = out.trim().trim_start_matches("```json").trim_end_matches("```").trim();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
                    let scope = ForgetScope {
                        topics: v.get("topics").and_then(|x| x.as_array()).map(|a| {
                            a.iter().filter_map(|s| s.as_str().map(String::from)).collect()
                        }).unwrap_or_default(),
                        scenes: v.get("scenes").and_then(|x| x.as_array()).map(|a| {
                            a.iter().filter_map(|s| s.as_str().map(String::from)).collect()
                        }).unwrap_or_default(),
                        tiers: v.get("tiers").and_then(|x| x.as_array()).map(|a| {
                            a.iter().filter_map(|s| s.as_str().map(String::from)).collect()
                        }).unwrap_or_default(),
                        after: v.get("after").and_then(|x| x.as_i64()),
                        before: v.get("before").and_then(|x| x.as_i64()),
                    };
                    return Ok(scope);
                }
            }
        }
        Ok(rule_parse_forget(instruction))
    }

    /// 预览：返回范围内将被删除的记忆（不执行）
    pub fn forget_preview(&self, scope: &ForgetScope, limit: usize) -> Result<Vec<ForgetTarget>> {
        let mut out = Vec::new();
        let records = self.list(None, None, false, 2000)?;
        for rec in records {
            if !scope_filter(&rec, scope) {
                continue;
            }
            let mut matched_by = String::new();
            if !scope.topics.is_empty() {
                matched_by = "topic".into();
            } else if !scope.scenes.is_empty() {
                matched_by = "scene".into();
            } else if scope.after.is_some() || scope.before.is_some() {
                matched_by = "time".into();
            }
            out.push(ForgetTarget {
                id: rec.id,
                title: rec.title,
                tier: rec.tier.as_str().to_string(),
                scene: rec.scene.clone(),
                created_at: rec.created_at,
                matched_by,
            });
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// 执行遗忘。返回影响条数。
    pub fn forget_execute(&self, scope: &ForgetScope, mode: ForgetMode) -> Result<usize> {
        let targets = self.forget_preview(scope, 100000)?;
        let ids: Vec<i64> = targets.iter().map(|t| t.id).collect();
        if ids.is_empty() {
            return Ok(0);
        }
        match mode {
            ForgetMode::Tombstone => self.tombstone(&ids),
            ForgetMode::Purge => self.purge(&ids),
        }
    }
}

fn scope_filter(rec: &MemoryRecord, scope: &ForgetScope) -> bool {
    if !scope.tiers.is_empty() && !scope.tiers.iter().any(|t| t == rec.tier.as_str()) {
        return false;
    }
    if !scope.scenes.is_empty() && !scope.scenes.iter().any(|s| s == &rec.scene) {
        return false;
    }
    if let Some(a) = scope.after {
        if rec.created_at < a {
            return false;
        }
    }
    if let Some(b) = scope.before {
        if rec.created_at > b {
            return false;
        }
    }
    if !scope.topics.is_empty() {
        let hay = format!("{} {} {}", rec.title, rec.content, rec.entities.join(" "));
        return scope.topics.iter().any(|t| hay.contains(t.as_str()));
    }
    true
}

/// 规则兜底解析："忘掉/删除/清除 ... 关于X的(一切|东西)"、场景词、层级词
fn rule_parse_forget(instruction: &str) -> ForgetScope {
    let mut scope = ForgetScope::default();
    // 场景词
    for (word, scene) in [("办公", "office"), ("编码", "coding"), ("开发", "coding"), ("系统", "system"), ("设置", "system")] {
        if instruction.contains(word) {
            scope.scenes.push(scene.into());
        }
    }
    // 层级词
    if instruction.contains("会话") || instruction.contains("聊天") {
        scope.tiers.push("episodic".into());
    }
    if instruction.contains("知识") {
        scope.tiers.push("knowledge".into());
    }
    if instruction.contains("偏好") || instruction.contains("习惯") {
        scope.tiers.push("preference".into());
    }
    // "关于X" 主题抽取
    if let Some(pos) = instruction.find("关于") {
        let rest = &instruction[pos + "关于".len()..];
        for stop in ["的一切", "的东西", "的内容", "相关", "的"] {
            if let Some(end) = rest.find(stop) {
                let topic = rest[..end].trim();
                if !topic.is_empty() && topic.chars().count() <= 40 {
                    scope.topics.push(topic.to_string());
                }
                break;
            }
        }
        // 没匹配到停止词：整句余量作为主题
        if scope.topics.is_empty() {
            let topic = rest.trim().trim_end_matches("。.!！?？");
            if !topic.is_empty() && topic.chars().count() <= 40 {
                scope.topics.push(topic.to_string());
            }
        }
    } else {
        // 引号内的内容作为主题
        let re = crate::sensitive::scan_free_quoted(instruction);
        scope.topics.extend(re);
    }
    scope
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashEmbedder;
    use crate::model::MemoryKind;
    use crate::model::Tier;

    fn store() -> MemoryStore {
        let dir = tempfile::tempdir().unwrap().into_path();
        MemoryStore::open(&dir.join("m.db"), &dir.join("fts"), Box::new(HashEmbedder::new(64))).unwrap()
    }

    #[test]
    fn test_rule_parse() {
        let s = rule_parse_forget("忘掉关于项目部署路径的一切");
        assert_eq!(s.topics, vec!["项目部署路径".to_string()]);
        let s2 = rule_parse_forget("删除办公场景的会话记录");
        assert!(s2.scenes.contains(&"office".to_string()));
        assert!(s2.tiers.contains(&"episodic".to_string()));
    }

    #[test]
    fn test_forget_flow() {
        let s = store();
        let mut a = MemoryRecord::new(Tier::Knowledge, MemoryKind::Fact, "部署", "项目部署路径是 /opt/apps 目录");
        a.entities = vec!["项目部署路径".into()];
        a.scene = "coding".into();
        let id = s.put(a).unwrap();
        let b = MemoryRecord::new(Tier::Episodic, MemoryKind::Conversation, "闲聊", "今天天气不错");
        s.put(b).unwrap();

        let scope = ForgetScope {
            topics: vec!["项目部署路径".into()],
            ..Default::default()
        };
        let targets = s.forget_preview(&scope, 100).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, id);

        let n = s.forget_execute(&scope, ForgetMode::Tombstone).unwrap();
        assert_eq!(n, 1);
        assert!(s.get(id).unwrap().unwrap().tombstone);
        // 恢复
        s.restore(&[id]).unwrap();
        assert!(!s.get(id).unwrap().unwrap().tombstone);
        // 硬清除
        let n2 = s.forget_execute(&scope, ForgetMode::Purge).unwrap();
        assert_eq!(n2, 1);
        assert!(s.get(id).unwrap().is_none());
    }
}
