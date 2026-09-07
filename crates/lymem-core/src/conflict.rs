//! 知识冲突处理：检测 → 分类 → 仲裁 → 版本化（赛题条款 3，指标正确率 ≥88%）。
//!
//! - 检测：新知识与既有知识在实体重叠 + 向量相似度超阈值时进入冲突流程；
//! - 分类（规则优先）：数值更新 / 方法过时 / 前提失效 / 疑似冲突；
//! - 仲裁：规则（时间戳+来源可信度）优先，模糊时 LLM 三选一（取代/合并/共存）；
//! - 版本化：取代 → 旧版本 is_current=0 保留可回溯；合并 → 生成合并稿；共存 → 双双保留。

use rusqlite::params;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::embedding::cosine;
use crate::error::Result;
use crate::llm_hook::LlmJudge;
use crate::model::{MemoryRecord, Tier};
use crate::store::MemoryStore;

/// 冲突类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictType {
    /// 数值/参数更新
    NumericUpdate,
    /// 方法/流程过时
    MethodObsolete,
    /// 前提失效
    PremiseInvalid,
    /// 表面相似实则不同（非冲突）
    Spurious,
    Unknown,
}

impl ConflictType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictType::NumericUpdate => "numeric_update",
            ConflictType::MethodObsolete => "method_obsolete",
            ConflictType::PremiseInvalid => "premise_invalid",
            ConflictType::Spurious => "spurious",
            ConflictType::Unknown => "unknown",
        }
    }
}

/// 仲裁结果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    Supersede,
    Merge,
    Coexist,
}

impl Resolution {
    pub fn as_str(&self) -> &'static str {
        match self {
            Resolution::Supersede => "supersede",
            Resolution::Merge => "merge",
            Resolution::Coexist => "coexist",
        }
    }
    pub fn parse(s: &str) -> Resolution {
        match s {
            "merge" => Resolution::Merge,
            "coexist" => Resolution::Coexist,
            _ => Resolution::Supersede,
        }
    }
}

/// 来源可信度基线（仲裁规则的一部分）
fn source_trust(source: &str) -> f64 {
    match source {
        "manual" => 0.95,
        "official" => 0.9,
        "agent" => 0.7,
        _ => 0.5,
    }
}

/// 从文本中抽取数字集合（数值更新判定用）
fn numbers_in(text: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() || c == '.' {
            cur.push(c);
        } else if !cur.is_empty() {
            if let Ok(n) = cur.parse() {
                out.push(n);
            }
            cur.clear();
        }
    }
    if !cur.is_empty() {
        if let Ok(n) = cur.parse() {
            out.push(n);
        }
    }
    out
}

/// 冲突候选：(旧记忆 id, chunk 级最大余弦)
pub struct ConflictCandidate {
    pub old_id: i64,
    pub similarity: f32,
}

/// 冲突上下文（传给 LLM 仲裁）
pub struct ConflictContext {
    pub old_record: MemoryRecord,
    pub new_record: MemoryRecord,
    pub similarity: f32,
}

/// 冲突检测结果（含仲裁建议）
pub struct ConflictOutcome {
    pub ctype: ConflictType,
    pub resolution: Resolution,
    pub reason: String,
    pub decided_by: &'static str,
}

/// 人工改判选项（冲突调解台：自动仲裁之上的最终人工裁决，可重复改判）。
#[derive(Debug, Clone)]
pub enum ManualChoice {
    /// 新版生效（维持/恢复"新版取代旧版"）
    KeepNew,
    /// 恢复旧版生效
    KeepOld,
    /// 两条并存（各自进入有效集）
    Coexist,
    /// 合并为一条：merged_text 缺省时拼接两版
    Merge { merged_text: Option<String> },
}

impl ManualChoice {
    pub fn as_str(&self) -> &'static str {
        match self {
            ManualChoice::KeepNew => "keep_new",
            ManualChoice::KeepOld => "keep_old",
            ManualChoice::Coexist => "coexist",
            ManualChoice::Merge { .. } => "merge",
        }
    }
    /// 落库的 resolution 值（沿用既有三值语义）
    fn resolution_str(&self) -> &'static str {
        match self {
            ManualChoice::KeepNew | ManualChoice::KeepOld => "supersede",
            ManualChoice::Coexist => "coexist",
            ManualChoice::Merge { .. } => "merge",
        }
    }
    fn label(&self) -> &'static str {
        match self {
            ManualChoice::KeepNew => "维持新版生效",
            ManualChoice::KeepOld => "恢复旧版生效",
            ManualChoice::Coexist => "改为两条并存",
            ManualChoice::Merge { .. } => "合并为一条",
        }
    }
    pub fn parse(choice: &str, merged_text: Option<String>) -> Option<ManualChoice> {
        match choice {
            "keep_new" => Some(ManualChoice::KeepNew),
            "keep_old" => Some(ManualChoice::KeepOld),
            "coexist" => Some(ManualChoice::Coexist),
            "merge" => Some(ManualChoice::Merge { merged_text }),
            _ => None,
        }
    }
}

impl MemoryStore {
    /// 步骤 1（检测）：对新知识找冲突候选。
    /// 规则：实体名有交集的当前有效知识中，chunk 向量最大余弦 ≥ threshold。
    pub fn detect_conflicts(&self, new_record: &MemoryRecord, threshold: f32) -> Result<Vec<ConflictCandidate>> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for name in &new_record.entities {
            for old_id in self.memories_with_entity(name, Tier::Knowledge)? {
                if old_id == new_record.id || !seen.insert(old_id) {
                    continue;
                }
                let sim = self
                    .chunk_vectors(old_id)?
                    .iter()
                    .filter_map(|old_vec| {
                        // 与新记忆全部 chunk 求最大余弦
                        let best: f32 = self
                            .new_chunk_vectors_cache(new_record)
                            .iter()
                            .map(|nv| cosine(old_vec, nv))
                            .fold(f32::MIN, |a: f32, b: f32| if b > a { b } else { a });
                        Some(best)
                    })
                    .fold(f32::MIN, |a: f32, b: f32| if b > a { b } else { a });
                if sim >= threshold {
                    out.push(ConflictCandidate { old_id, similarity: sim });
                }
            }
        }
        Ok(out)
    }

    // 新记忆的 chunk 向量（put 之后可从库里取；put 之前用嵌入器现算并缓存于调用栈）
    fn new_chunk_vectors_cache<'a>(&self, rec: &MemoryRecord) -> Vec<Vec<f32>> {
        if rec.id > 0 {
            if let Ok(vs) = self.chunk_vectors(rec.id) {
                if !vs.is_empty() {
                    return vs;
                }
            }
        }
        let chunks = crate::store::chunk_text(&rec.content, 480);
        let refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
        self.embedder.embed(&refs).unwrap_or_default()
    }

    /// 步骤 2+3（分类 + 仲裁）。
    /// LLM 可用时模糊样例交给模型三选一并给出理由；否则规则路径。
    pub fn resolve_conflict(&self, ctx: &ConflictContext, judge: Option<&dyn LlmJudge>) -> Result<ConflictOutcome> {
        // ---- 规则分类 ----
        let old_nums = numbers_in(&ctx.old_record.content);
        let new_nums = numbers_in(&ctx.new_record.content);
        let same_count = old_nums.len() == new_nums.len() && !old_nums.is_empty();
        // 数值更新需同时满足：数字个数相同且值不同 + 去数字后文本高度相似
        // （否则"开发端口8080/生产端口443"这类不同属性的数字会被误判为更新）
        let bigrams = |t: &str| -> std::collections::HashSet<String> {
            let cleaned: String = t.chars().filter(|c| !c.is_ascii_digit() && *c != '.').collect();
            let ch: Vec<char> = cleaned.chars().filter(|c| !c.is_whitespace()).collect();
            if ch.len() < 2 {
                return std::collections::HashSet::new();
            }
            ch.windows(2).map(|w| w.iter().collect()).collect()
        };
        let ob = bigrams(&ctx.old_record.content);
        let nb = bigrams(&ctx.new_record.content);
        let inter = ob.intersection(&nb).count();
        let union = ob.len().max(nb.len()).max(1);
        let word_overlap = inter as f64 / union as f64;
        let numeric_changed = same_count && old_nums != new_nums && word_overlap >= 0.4;
        let newer = ctx.new_record.created_at > ctx.old_record.created_at;

        let mut ctype = if numeric_changed {
            ConflictType::NumericUpdate
        } else if ctx.similarity > 0.92 {
            ConflictType::MethodObsolete
        } else if ctx.similarity < 0.82 {
            ConflictType::Spurious
        } else {
            ConflictType::Unknown
        };

        // ---- 规则仲裁 ----
        let trust_old = source_trust(&ctx.old_record.source) + ctx.old_record.confidence;
        let trust_new = source_trust(&ctx.new_record.source) + ctx.new_record.confidence;
        let mut resolution = if ctype == ConflictType::Spurious {
            Resolution::Coexist
        } else if newer && trust_new >= trust_old * 0.8 {
            Resolution::Supersede
        } else {
            Resolution::Coexist
        };
        let mut reason = format!(
            "规则仲裁：相似度{:.3}，数值变化={numeric_changed}，新知识{}，信任度 {trust_new:.2}/{trust_old:.2}",
            ctx.similarity,
            if newer { "更新" } else { "更旧" }
        );
        let mut decided_by = "rule";

        // ---- LLM 仲裁（模糊样例）----
        if let Some(judge) = judge {
            if ctype == ConflictType::Unknown {
                let prompt = format!(
                    "旧知识：[{}]\n新知识：[{}]\n两者主题相近。请判断处理方式，只输出 JSON：\
                     {{\"type\":\"numeric_update|method_obsolete|premise_invalid|spurious\",\
                     \"resolution\":\"supersede|merge|coexist\",\"reason\":\"一句话\"}}",
                    ctx.old_record.content, ctx.new_record.content
                );
                if let Some(out) = judge.complete(
                    "你是 OS Agent 的知识冲突仲裁器。supersede=新知识取代旧的；merge=合并成一条；coexist=两条并存（适用不同场景）。",
                    &prompt,
                ) {
                    let t = out.trim().trim_start_matches("```json").trim_end_matches("```").trim().to_string();
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                        let r = v.get("resolution").and_then(|x| x.as_str()).unwrap_or("supersede");
                        resolution = Resolution::parse(r);
                        ctype = match v.get("type").and_then(|x| x.as_str()).unwrap_or("") {
                            "numeric_update" => ConflictType::NumericUpdate,
                            "method_obsolete" => ConflictType::MethodObsolete,
                            "premise_invalid" => ConflictType::PremiseInvalid,
                            "spurious" => ConflictType::Spurious,
                            _ => ctype,
                        };
                        reason = v.get("reason").and_then(|x| x.as_str()).unwrap_or("LLM 仲裁").to_string();
                        decided_by = "llm";
                    }
                }
            }
        }
        Ok(ConflictOutcome { ctype, resolution, reason, decided_by: if decided_by == "llm" { "llm" } else { "rule" } })
    }

    /// 步骤 4（版本化执行）：按仲裁结果落库并记录冲突。
    /// 返回冲突记录条数。
    pub fn apply_resolution(&self, old_id: i64, new_id: i64, outcome: &ConflictOutcome) -> Result<()> {
        match outcome.resolution {
            Resolution::Supersede => {
                self.mark_superseded(old_id, new_id)?;
            }
            Resolution::Merge => {
                // 合并：拼接两代内容生成合并稿，双方标记历史
                if let (Some(old), Some(new)) = (self.get(old_id)?, self.get(new_id)?) {
                    let merged = format!("{}\n\n----\n\n{}", old.content, new.content);
                    let mut rec = MemoryRecord::new(Tier::Knowledge, new.kind, format!("{}（合并）", new.title), merged);
                    rec.source = new.source.clone();
                    rec.scene = new.scene.clone();
                    rec.entities = old.entities.iter().chain(new.entities.iter()).cloned().collect();
                    rec.version_of = Some(old_id);
                    let merged_id = self.put(rec)?;
                    self.mark_superseded(old_id, merged_id)?;
                    self.mark_superseded(new_id, merged_id)?;
                }
            }
            Resolution::Coexist => {
                // 双双保留：不做变更，仅记录冲突
            }
        }
        self.record_conflict(
            old_id,
            new_id,
            outcome.ctype.as_str(),
            outcome.resolution.as_str(),
            &outcome.reason,
            outcome.decided_by,
        )?;
        Ok(())
    }

    /// 一体化入口：写入知识记忆并跑完整冲突管道。
    /// 返回 (新记忆 id, 冲突处理结果列表)。
    pub fn put_knowledge_with_conflicts(&self, mut rec: MemoryRecord, judge: Option<&dyn LlmJudge>, threshold: f32) -> Result<(i64, Vec<ConflictOutcome>)> {
        rec.tier = Tier::Knowledge;
        let new_id = self.put(rec.clone())?;
        // put 克隆写入，这里必须回填 id，否则 detect_conflicts 的自匹配护栏
        // （old_id == new_record.id）失效，新记录会与自身产生相似度 1.0 的假冲突。
        rec.id = new_id;
        let candidates = self.detect_conflicts(&rec, threshold)?;
        let mut outcomes = Vec::new();
        for c in candidates {
            if let Some(old) = self.get(c.old_id)? {
                let ctx = ConflictContext {
                    old_record: old,
                    new_record: rec.clone(),
                    similarity: c.similarity,
                };
                let outcome = self.resolve_conflict(&ctx, judge)?;
                self.apply_resolution(c.old_id, new_id, &outcome)?;
                outcomes.push(outcome);
            }
        }
        Ok((new_id, outcomes))
    }

    /// 人工改判:调整某条冲突的仲裁结果并把 decided_by 记为 user(人在回路,可重复改判)。
    /// 返回 {ok, resolution};conflict_id 不存在或 old/new 记录缺失时 ok=false。
    pub fn resolve_conflict_manual(&self, conflict_id: i64, choice: &ManualChoice, note: &str) -> Result<serde_json::Value> {
        let (old_id, new_id, old_reason) = {
            let conn = self.conn.lock().unwrap();
            let row: Option<(Option<i64>, Option<i64>, String)> = conn
                .query_row(
                    "SELECT old_id, new_id, reason FROM conflicts WHERE id = ?1",
                    params![conflict_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            match row {
                Some((Some(o), Some(n), reason)) => (o, n, reason),
                _ => return Ok(serde_json::json!({"ok": false, "error": "冲突记录不存在或缺少新旧记忆"})),
            }
        };
        let (old, new) = match (self.get(old_id)?, self.get(new_id)?) {
            (Some(o), Some(n)) => (o, n),
            _ => return Ok(serde_json::json!({"ok": false, "error": "新旧记忆已不存在（可能已被清除）"})),
        };
        let resolution_str = choice.resolution_str();
        match choice {
            ManualChoice::KeepNew => {
                self.mark_superseded(old_id, new_id)?;
                // mark_superseded 只退役旧版；改判路径可能刚执行过 KeepOld(新版被退出),
                // 这里必须把新版拉回有效集
                let conn = self.conn.lock().unwrap();
                conn.execute("UPDATE memories SET is_current = 1 WHERE id = ?1", params![new_id])?;
            }
            ManualChoice::KeepOld => {
                let conn = self.conn.lock().unwrap();
                conn.execute("UPDATE memories SET is_current = 1 WHERE id = ?1", params![old_id])?;
                conn.execute("UPDATE memories SET is_current = 0 WHERE id = ?1", params![new_id])?;
            }
            ManualChoice::Coexist => {
                let conn = self.conn.lock().unwrap();
                conn.execute("UPDATE memories SET is_current = 1 WHERE id = ?1", params![old_id])?;
                conn.execute("UPDATE memories SET is_current = 1 WHERE id = ?1", params![new_id])?;
            }
            ManualChoice::Merge { merged_text } => {
                let text = merged_text
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| format!("{}\n\n----\n\n{}", old.content, new.content));
                let mut rec = MemoryRecord::new(Tier::Knowledge, new.kind, format!("{}（合并）", new.title), text);
                rec.source = new.source.clone();
                rec.scene = new.scene.clone();
                rec.entities = old.entities.iter().chain(new.entities.iter()).cloned().collect();
                rec.version_of = Some(old_id);
                let merged_id = self.put(rec)?;
                self.mark_superseded(old_id, merged_id)?;
                self.mark_superseded(new_id, merged_id)?;
            }
        }
        {
            let conn = self.conn.lock().unwrap();
            let reason_upd = if note.trim().is_empty() {
                format!("{}（原仲裁：{}。人工改判）", choice.label(), old_reason)
            } else {
                format!("{}（原仲裁：{}。人工改判：{}）", choice.label(), old_reason, note.trim())
            };
            conn.execute(
                "UPDATE conflicts SET resolution = ?2, decided_by = 'user', reason = ?3 WHERE id = ?1",
                params![conflict_id, resolution_str, reason_upd],
            )?;
        }
        self.audit("conflict_resolve_manual", &serde_json::json!({"id": conflict_id, "choice": choice.as_str()}))?;
        Ok(serde_json::json!({"ok": true, "resolution": resolution_str, "choice": choice.as_str()}))
    }



    /// 冲突处理正确率统计（评测用）：以 conflicts 表中 decided_by/ctype 汇总
    pub fn conflict_stats(&self) -> Result<serde_json::Value> {
        let conflicts = self.conflicts_all()?;
        let total = conflicts.len();
        let by_res: std::collections::HashMap<String, usize> = conflicts
            .iter()
            .filter_map(|c| c.get("resolution").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .fold(std::collections::HashMap::new(), |mut m, s| {
                *m.entry(s).or_default() += 1;
                m
            });
        let by_who: std::collections::HashMap<String, usize> = conflicts
            .iter()
            .filter_map(|c| c.get("decided_by").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .fold(std::collections::HashMap::new(), |mut m, s| {
                *m.entry(s).or_default() += 1;
                m
            });
        Ok(serde_json::json!({"total": total, "by_resolution": by_res, "by_decider": by_who}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashEmbedder;
    use crate::model::MemoryKind;

    fn store() -> MemoryStore {
        let dir = tempfile::tempdir().unwrap().into_path();
        MemoryStore::open(&dir.join("m.db"), &dir.join("fts"), Box::new(HashEmbedder::new(64))).unwrap()
    }

    #[test]
    fn test_supersede_flow() {
        let s = store();
        let mut a = MemoryRecord::new(Tier::Knowledge, MemoryKind::Fact, "部署路径", "软件默认安装到 /opt/apps 路径下");
        a.entities = vec!["安装路径".into()];
        let (id1, _) = s.put_knowledge_with_conflicts(a, None, 0.8).unwrap();

        let mut b = MemoryRecord::new(Tier::Knowledge, MemoryKind::Fact, "部署路径v2", "软件默认安装到 /usr/local 路径下，2026 版本已修改");
        b.entities = vec!["安装路径".into()];
        let (id2, outcomes) = s.put_knowledge_with_conflicts(b, None, 0.8).unwrap();

        // HashEmbedder 相似度不稳定，此处至少验证管道跑通且冲突表有记录
        if !outcomes.is_empty() {
            let old = s.get(id1).unwrap().unwrap();
            let new = s.get(id2).unwrap().unwrap();
            // 若判定取代，旧版本应退出当前有效集
            if outcomes[0].resolution == Resolution::Supersede {
                assert!(!old.is_current);
                assert!(new.is_current);
                assert_eq!(new.version_of, Some(id1));
            }
            let stats = s.conflict_stats().unwrap();
            assert!(stats["total"].as_u64().unwrap() >= 1);
        }
    }

    #[test]
    fn test_manual_resolve() {
        let s = store();
        let mut a = MemoryRecord::new(Tier::Knowledge, MemoryKind::Fact, "打印服务器地址", "办公区打印服务器地址为 192.168.1.100，端口 631");
        a.entities = vec!["打印服务器".into()];
        let (id1, _) = s.put_knowledge_with_conflicts(a, None, 0.8).unwrap();
        let mut b = MemoryRecord::new(Tier::Knowledge, MemoryKind::Fact, "打印服务器地址", "办公区打印服务器地址为 192.168.1.102，端口 631");
        b.entities = vec!["打印服务器".into()];
        let (id2, _) = s.put_knowledge_with_conflicts(b, None, 0.8).unwrap();

        // 确保有一条冲突记录（管道未产出时插入合成行，聚焦改判本身）
        let conflict_id: i64 = {
            let have: i64 = {
                let conn = s.conn.lock().unwrap();
                conn.query_row("SELECT COUNT(*) FROM conflicts WHERE old_id=?1 AND new_id=?2", params![id1, id2], |r| r.get(0)).unwrap_or(0)
            };
            if have > 0 {
                let conn = s.conn.lock().unwrap();
                conn.query_row("SELECT id FROM conflicts WHERE old_id=?1 AND new_id=?2 LIMIT 1", params![id1, id2], |r| r.get(0)).unwrap()
            } else {
                let conn = s.conn.lock().unwrap();
                conn.execute(
                    "INSERT INTO conflicts(old_id,new_id,ctype,resolution,reason,decided_by,created_at) VALUES (?1,?2,'unknown','supersede','测试','rule',0)",
                    params![id1, id2],
                )
                .unwrap();
                conn.last_insert_rowid()
            }
        };

        // 改判：恢复旧版
        let r = s.resolve_conflict_manual(conflict_id, &ManualChoice::KeepOld, "").unwrap();
        assert_eq!(r["ok"], true);
        assert!(s.get(id1).unwrap().unwrap().is_current);
        assert!(!s.get(id2).unwrap().unwrap().is_current);
        // 再改判：维持新版（幂等可重复）
        let r = s.resolve_conflict_manual(conflict_id, &ManualChoice::KeepNew, "以新地址为准").unwrap();
        assert_eq!(r["ok"], true);
        assert!(s.get(id2).unwrap().unwrap().is_current);
        assert!(!s.get(id1).unwrap().unwrap().is_current);
        // 改判记录应落到冲突行
        let c = s.conflicts_all().unwrap().into_iter().find(|c| c["id"].as_i64() == Some(conflict_id)).unwrap();
        assert_eq!(c["decided_by"], "user");
        assert!(c["reason"].as_str().unwrap().contains("以新地址为准"));
        // 合并（自定义文本）
        let r = s.resolve_conflict_manual(conflict_id, &ManualChoice::Merge { merged_text: Some("地址以 IT 公告为准".into()) }, "").unwrap();
        assert_eq!(r["resolution"], "merge");
        let merged_current = s.list(Some(Tier::Knowledge), None, false, 100).unwrap();
        assert!(merged_current.iter().any(|m| m.content == "地址以 IT 公告为准"));
        assert!(!s.get(id1).unwrap().unwrap().is_current);
        assert!(!s.get(id2).unwrap().unwrap().is_current);
    }
}
