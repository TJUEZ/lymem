//! 记忆晋升管道（赛题条款 6：短/中期记忆数据流转）。
//!
//! 工作记忆（内存态） --flush--> 情景记忆（中期，SQLite）
//! 情景记忆 --consolidate--> 知识记忆（长期，蒸馏+冲突管道）
//!
//! 蒸馏策略：LLM 可用时做摘要式蒸馏；否则抽取式（按访问度/置信度选取代表内容）。

use crate::error::Result;
use crate::llm_hook::LlmJudge;
use crate::model::{MemoryKind, MemoryRecord, Tier};
use crate::store::MemoryStore;

/// 工作记忆：会话内的短期缓冲（内存态，进程重启即失效，由调用方决定 flush 时机）
pub struct WorkingMemory {
    pub items: Vec<MemoryRecord>,
    capacity: usize,
    pub scene: String,
    pub source: String,
}

impl WorkingMemory {
    pub fn new(scene: impl Into<String>, capacity: usize) -> Self {
        WorkingMemory {
            items: Vec::new(),
            capacity,
            scene: scene.into(),
            source: "agent".into(),
        }
    }

    pub fn push(&mut self, kind: MemoryKind, title: impl Into<String>, content: impl Into<String>) {
        let mut rec = MemoryRecord::new(Tier::Working, kind, title, content);
        rec.scene = self.scene.clone();
        rec.source = self.source.clone();
        self.items.push(rec);
        // 环形容量：超限挤掉最旧
        if self.items.len() > self.capacity {
            self.items.remove(0);
        }
    }

    /// 生成注入给 Agent 的上下文（最近 N 条 + 工作摘要行）
    pub fn render_context(&self, last_n: usize) -> String {
        let recent: Vec<String> = self.items.iter().rev().take(last_n).map(|r| r.content.clone()).collect();
        format!("[工作记忆 x{}]\n{}", self.items.len(), recent.join("\n"))
    }

    /// 清空并返回全部条目（供 flush）
    pub fn drain(&mut self) -> Vec<MemoryRecord> {
        std::mem::take(&mut self.items)
    }
}

impl MemoryStore {
    /// 短期 → 中期：把工作记忆条目落入情景层。
    /// 返回入库 id 列表。
    pub fn flush_working(&self, working: &mut WorkingMemory) -> Result<Vec<i64>> {
        let items = working.drain();
        let mut ids = Vec::with_capacity(items.len());
        for mut rec in items {
            rec.tier = Tier::Episodic;
            ids.push(self.put(rec)?);
        }
        if !ids.is_empty() {
            self.audit("promote_working_to_episodic", &serde_json::json!({"count": ids.len()}))?;
        }
        Ok(ids)
    }

    /// 中期 → 长期：情景记忆蒸馏为知识记忆。
    /// 触发条件：同主题（首个实体）条目数 ≥ min_group 或累计访问 ≥ min_access。
    /// LLM 可用时摘要蒸馏；否则抽取式（取访问度最高的代表条目拼接）。
    /// 返回新知识的 id 列表。
    pub fn consolidate_episodic(&self, judge: Option<&dyn LlmJudge>, min_group: usize, min_access: i64) -> Result<Vec<i64>> {
        let episodic = self.list(Some(Tier::Episodic), None, false, 5000)?;
        // 按主实体聚合
        let mut groups: std::collections::HashMap<String, Vec<&MemoryRecord>> = std::collections::HashMap::new();
        for rec in &episodic {
            let key = rec.entities.first().cloned().unwrap_or_else(|| "__none__".into());
            groups.entry(key).or_default().push(rec);
        }
        let mut created = Vec::new();
        for (topic, members) in groups {
            let access_sum: i64 = members.iter().map(|m| m.access_count).sum();
            if members.len() < min_group && access_sum < min_access {
                continue;
            }
            // 已蒸馏标记检查（meta.distilled）
            if members.iter().all(|m| m.meta.get("distilled").is_some_and(|v| v.as_bool() == Some(true))) {
                continue;
            }
            let mut ranked = members.clone();
            ranked.sort_by(|a, b| (b.access_count, b.confidence as i64).cmp(&(a.access_count, a.confidence as i64)));
            let scene = ranked[0].scene.clone();
            let source = ranked[0].source.clone();

            // 蒸馏内容
            let joined = ranked.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
            let content = if let Some(j) = judge {
                j.complete(
                    "你是 OS Agent 的知识蒸馏器。把多条相关情景记忆压缩成一条简洁的知识条目（不超过 300 字），\
                     保留可复用的事实、流程与结论，去除临时细节。直接输出条目正文。",
                    &joined,
                )
                .map(|s| crate::ingest::clean_text(&s))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| extractive(&ranked))
            } else {
                extractive(&ranked)
            };

            let mut rec = MemoryRecord::new(
                Tier::Knowledge,
                ranked[0].kind,
                format!("知识：{topic}"),
                content,
            );
            rec.scene = scene;
            rec.source = source;
            rec.entities = vec![topic.clone()];
            rec.confidence = ranked
                .iter()
                .map(|m| m.confidence)
                .fold(0.0_f64, |a: f64, b: f64| if b > a { b } else { a })
                .min(0.95);
            let (id, _) = self.put_knowledge_with_conflicts(rec, judge, 0.8)?;
            created.push(id);

            // 标记已蒸馏
            let conn = self.conn.lock().unwrap();
            for m in &ranked {
                let mut meta = m.meta.clone();
                meta["distilled"] = serde_json::json!(true);
                let _ = conn.execute(
                    "UPDATE memories SET meta = ?2 WHERE id = ?1",
                    rusqlite::params![m.id, meta.to_string()],
                );
            }
        }
        if !created.is_empty() {
            self.audit("promote_episodic_to_knowledge", &serde_json::json!({"created": created}))?;
        }
        Ok(created)
    }
}

/// 抽取式蒸馏：每条取首句，最多 5 条
fn extractive(ranked: &[&MemoryRecord]) -> String {
    let mut parts = Vec::new();
    for m in ranked.iter().take(5) {
        let first_sentence = m
            .content
            .split(|c| c == '。' || c == '\n')
            .find(|s| !s.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
        if !first_sentence.is_empty() {
            parts.push(first_sentence);
        }
    }
    parts.join("。")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashEmbedder;

    fn store() -> MemoryStore {
        let dir = tempfile::tempdir().unwrap().into_path();
        MemoryStore::open(&dir.join("m.db"), &dir.join("fts"), Box::new(HashEmbedder::new(64))).unwrap()
    }

    #[test]
    fn test_working_flush_and_consolidate() {
        let s = store();
        let mut wm = WorkingMemory::new("coding", 100);
        wm.push(MemoryKind::Conversation, "r1", "关于`部署脚本`的讨论：用 deploy.sh 一键部署");
        wm.push(MemoryKind::ToolResult, "r2", "工具 `bash` 执行 deploy.sh 成功");
        assert!(wm.render_context(2).contains("[工作记忆 x2]"));

        let ids = s.flush_working(&mut wm).unwrap();
        assert_eq!(ids.len(), 2);
        assert!(wm.items.is_empty());

        // 实体需要登记后 consolidate 才能分组：模拟实体
        let eid = s.upsert_entity("部署脚本", "thing").unwrap();
        s.upsert_edge(eid, eid, "self", Some(ids[0])).unwrap();
        // 给记忆打实体（flush 后补）
        let conn = s.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE memories SET entity_keys = '[\"部署脚本\"]' WHERE id IN (?1, ?2)",
            rusqlite::params![ids[0], ids[1]],
        );
        drop(conn);

        let created = s.consolidate_episodic(None, 2, 0).unwrap();
        assert_eq!(created.len(), 1);
        let k = s.get(created[0]).unwrap().unwrap();
        assert_eq!(k.tier, Tier::Knowledge);
        assert!(k.title.contains("部署脚本"));
        // 幂等：再次蒸馏不再新建
        let created2 = s.consolidate_episodic(None, 2, 0).unwrap();
        assert!(created2.is_empty());
    }
}
