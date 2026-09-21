//! 混合检索：向量 ANN + BM25 全文 + 实体图扩展，RRF 融合后按偏好重排。
//! 对应赛题条款 (3) 的"关联检索优化"与指标"知识检索召回率/响应时间"。

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::model::{MemoryRecord, ScoredMemory, Tier};
use crate::store::MemoryStore;

/// 检索参数
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchParams {
    pub query: String,
    pub top_k: usize,
    /// 限定层级（默认知识+情景）
    pub tiers: Option<Vec<Tier>>,
    /// 限定场景
    pub scenes: Option<Vec<String>>,
    /// 时间窗（unix 秒）
    pub after: Option<i64>,
    pub before: Option<i64>,
    /// 通道开关（消融实验用）
    pub use_vec: bool,
    pub use_bm25: bool,
    pub use_graph: bool,
    /// 偏好重排开关（消融实验用）
    pub preference_rerank: bool,
    /// RRF 平滑常数 k
    pub rrf_k: f64,
    /// 每通道取多少候选（top_k 的倍数）
    pub candidate_multiplier: usize,
}

impl Default for SearchParams {
    fn default() -> Self {
        SearchParams {
            query: String::new(),
            top_k: 8,
            tiers: None,
            scenes: None,
            after: None,
            before: None,
            use_vec: true,
            use_bm25: true,
            use_graph: true,
            preference_rerank: true,
            rrf_k: 60.0,
            candidate_multiplier: 3,
        }
    }
}

impl SearchParams {
    pub fn new(query: impl Into<String>) -> Self {
        SearchParams {
            query: query.into(),
            ..Default::default()
        }
    }
}

fn tier_ok(rec_tier: &str, tiers: &Option<Vec<Tier>>) -> bool {
    match tiers {
        None => true,
        Some(list) => list.iter().any(|t| t.as_str() == rec_tier),
    }
}

/// Return per-query channel multipliers without changing the default behavior.
/// The classifier is deliberately small and deterministic so it can be audited
/// and evaluated with the LoCoMo question type breakdown.
fn adaptive_channel_weights(query: &str) -> (&'static str, f64, f64, f64) {
    let q = query.to_lowercase();
    let temporal = [
        "when", "date", "year", "before", "after", "recent", "last", "时间", "日期", "哪年", "什么时候",
    ]
    .iter()
    .any(|term| q.contains(term));
    let relational = [
        "who", "whose", "between", "relationship", "relation", "谁", "哪个人", "关系", "之间",
    ]
    .iter()
    .any(|term| q.contains(term));
    let semantic = [
        "why", "how", "similar", "related", "explain", "为什么", "如何", "类似", "相关", "解释",
    ]
    .iter()
    .any(|term| q.contains(term));
    let exactish = q.split_whitespace().count() <= 4
        && (q.chars().any(|c| c.is_ascii_digit())
            || q.contains('@')
            || q.contains('.')
            || q.contains('_')
            || q.contains('-'));

    if temporal {
        ("temporal", 0.85, 1.30, 0.75)
    } else if relational {
        ("relational", 0.90, 1.00, 1.25)
    } else if semantic {
        ("semantic", 1.25, 0.90, 0.90)
    } else if exactish {
        ("exactish", 0.75, 1.35, 0.85)
    } else {
        ("default", 1.0, 1.0, 1.0)
    }
}

/// 轻量词面证据：中文查询按二字片段计分，避免低质量/降级向量把
/// 完全不含查询词的长文本排到明确命中的记忆之前。
fn lexical_evidence(query: &str, title: &str, content: &str) -> f64 {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() { return 0.0; }
    let hay = format!("{} {}", title, content).to_lowercase();
    let mut hits = 0usize;
    let mut total = 0usize;
    for w in q.windows(2) {
        total += 1;
        if hay.contains(&w.iter().collect::<String>().to_lowercase()) { hits += 1; }
    }
    if total == 0 {
        return if hay.contains(&query.to_lowercase()) { 1.0 } else { 0.0 };
    }
    let phrase = query.split_whitespace().collect::<String>().to_lowercase();
    let phrase_bonus = if !phrase.is_empty() && hay.contains(&phrase) { 0.35 } else { 0.0 };
    (hits as f64 / total as f64 + phrase_bonus).min(1.0)
}

impl MemoryStore {
    /// 三路混合检索主入口。
    /// 流程：嵌入查询 → 向量 KNN / BM25 / 图扩展 → 过滤 → RRF 融合 → 偏好重排 → 触摸计数。
    pub fn search(&self, p: &SearchParams) -> Result<Vec<ScoredMemory>> {
        let t0 = std::time::Instant::now();
        let k = p.top_k.max(1);
        let cand = k * p.candidate_multiplier.max(1);

        // 查询侧繁→简归一化：与入库侧（store.put）保持同一规范形，
        // 否则简体查询对繁体语料的 BM25/向量双通道全部失配。
        let query = crate::t2s::to_simplified(&p.query);

        // 通道 1：向量（场景过滤时超采样，避免候选被其它场景占满）
        let mut vec_rank: Vec<(i64, f64, i64)> = Vec::new(); // (memory_id, distance, chunk_id)
        if p.use_vec {
            let qv = self.embedder.embed_one(&query)?;
            let knn_k = if p.scenes.is_some() { cand.max(256) } else { cand };
            vec_rank = self.knn(&qv, knn_k)?;
        }

        // 通道 2：BM25（对查询做同样的 CJK bigram 处理由分析器完成）
        let mut bm25_rank: Vec<(i64, i64, f32, String)> = Vec::new();
        if p.use_bm25 {
            let scene = p.scenes.as_ref().and_then(|v| {
                v.iter().find(|s| *s != "*").map(|s| s.as_str())
            });
            let fts = self.fts.lock().unwrap();
            if let Ok(hits) = fts.search(&query, None, scene, cand) {
                bm25_rank = hits;
            }
        }

        // 通道 3：图扩展（查询词模糊匹配实体 → 邻域记忆）
        let mut graph_rank: Vec<(i64, usize)> = Vec::new();
        if p.use_graph {
            // 取查询中的关键词（简单策略：整体 + 空格分词）
            let terms: Vec<String> = query.split_whitespace().map(|s| s.to_string()).chain(std::iter::once(query.clone())).collect();
            let mut ent_ids = Vec::new();
            for t in &terms {
                if t.chars().count() < 2 {
                    continue;
                }
                for (eid, _name) in self.find_entities_like(t, 10)? {
                    if !ent_ids.contains(&eid) {
                        ent_ids.push(eid);
                    }
                }
            }
            if !ent_ids.is_empty() {
                let expanded = self.graph_expand(&ent_ids, 2)?;
                graph_rank = expanded.into_iter().take(cand).collect();
                graph_rank.sort_by_key(|(_, hop)| *hop);
            }
        }

        // 收集全部候选 id（HashSet 去重，保持首现顺序）
        let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let mut all_ids: Vec<i64> = Vec::new();
        for (mid, _, _) in &vec_rank {
            if seen.insert(*mid) {
                all_ids.push(*mid);
            }
        }
        for (mid, _, _, _) in &bm25_rank {
            if seen.insert(*mid) {
                all_ids.push(*mid);
            }
        }
        for (mid, _) in &graph_rank {
            if seen.insert(*mid) {
                all_ids.push(*mid);
            }
        }

        // 批量拉取记录并应用过滤器（替代逐候选 get 的 N 次锁+全行读）
        let mut by_id: std::collections::HashMap<i64, MemoryRecord> = std::collections::HashMap::new();
        for rec in self.get_many(&all_ids)? {
            if !rec.tombstone && rec.is_current
                && tier_ok(rec.tier.as_str(), &p.tiers)
                && (p.scenes.is_none() || p.scenes.as_ref().unwrap().iter().any(|s| *s == rec.scene || s == "*"))
                && (p.after.is_none() || rec.created_at >= p.after.unwrap())
                && (p.before.is_none() || rec.created_at <= p.before.unwrap())
            {
                by_id.insert(rec.id, rec);
            }
        }

        // 每通道取各候选的最佳名次（原逻辑为逐候选线性扫描，O(n²)）
        let mut vec_best: std::collections::HashMap<i64, (usize, i64)> = std::collections::HashMap::new();
        for (rank, (mid, _, chunk)) in vec_rank.iter().enumerate() {
            vec_best.entry(*mid).or_insert((rank + 1, *chunk));
        }
        let mut bm25_best: std::collections::HashMap<i64, (usize, &String)> = std::collections::HashMap::new();
        for (rank, (mid, _, _, snip)) in bm25_rank.iter().enumerate() {
            bm25_best.entry(*mid).or_insert((rank + 1, snip));
        }
        let mut graph_best: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
        for (rank, (mid, _hop)) in graph_rank.iter().enumerate() {
            graph_best.entry(*mid).or_insert(rank + 1);
        }
        // 向量命中片段批量预取（替代逐命中查库）
        let chunk_ids: Vec<i64> = vec_best.values().map(|(_, c)| *c).collect();
        let chunk_snips = self.chunk_contents(&chunk_ids)?;

        // RRF 权重：读一次环境（原为每候选 3 次 env::var）
        let env_w = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0);
        let (mut w_vec, mut w_bm25, mut w_graph) =
            (env_w("LYMEM_W_VEC"), env_w("LYMEM_W_BM25"), env_w("LYMEM_W_GRAPH"));
        if std::env::var("LYMEM_ADAPTIVE_RETRIEVAL").as_deref() == Ok("1") {
            let (profile, vec_mul, bm25_mul, graph_mul) = adaptive_channel_weights(&query);
            w_vec *= vec_mul;
            w_bm25 *= bm25_mul;
            w_graph *= graph_mul;
            tracing::debug!(profile, w_vec, w_bm25, w_graph, "自适应检索权重");
        }
        let first_seen: std::collections::HashMap<i64, usize> =
            all_ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();

        // RRF 融合
        let mut fused: Vec<(i64, f64, Option<usize>, Option<usize>, Option<usize>, String)> = Vec::new();
        for rec in by_id.values() {
            let id = rec.id;
            let (mut score, mut rv, mut rb, mut rg) = (0.0, None, None, None);
            let mut snippet = String::new();
            if let Some((rank, chunk)) = vec_best.get(&id) {
                rv = Some(*rank);
                score += w_vec / (p.rrf_k + *rank as f64);
                snippet = chunk_snips.get(chunk).cloned().unwrap_or_default();
            }
            if let Some((rank, snip)) = bm25_best.get(&id) {
                rb = Some(*rank);
                score += w_bm25 / (p.rrf_k + *rank as f64);
                if snippet.is_empty() {
                    snippet = snip.to_string();
                }
            }
            if let Some(rank) = graph_best.get(&id) {
                rg = Some(*rank);
                score += w_graph / (p.rrf_k + *rank as f64);
            }
            let lexical = lexical_evidence(&query, &rec.title, &rec.content);
            // 词面证据是硬信号：命中查询词的记忆获得明显提升；完全没有
            // 词面证据且只靠向量命中的长文本轻微降权。
            score += lexical * 0.055;
            if lexical == 0.0 && rb.is_none() && rg.is_none() {
                score *= 0.55;
            }
            if snippet.is_empty() {
                snippet = rec.content.chars().take(120).collect();
            }
            fused.push((id, score, rv, rb, rg, snippet));
        }
        fused.sort_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| first_seen.get(&a.0).cmp(&first_seen.get(&b.0)))
        });

        // 偏好重排：命中偏好作用域的记忆获得提升
        let prefs = if p.preference_rerank { self.effective_preferences(None)? } else { Vec::new() };
        let mut out: Vec<ScoredMemory> = Vec::new();
        for (id, rrf, rv, rb, rg, snippet) in fused.into_iter().take(k) {
            let Some(rec) = by_id.get(&id) else { continue };
            let rec = rec.clone();
            let mut final_score = rrf;
            for pref in &prefs {
                // 偏好键形如 tool_choice.xxx / output_style.xxx；作用场景匹配或通配时轻微提升
                if pref.scenes.iter().any(|s| s == "*" || s == &rec.scene) {
                    final_score *= 1.0 + 0.05 * pref.confidence.clamp(0.0, 1.0);
                }
            }
            out.push(ScoredMemory {
                record: rec,
                rrf_score: rrf,
                final_score,
                rank_vec: rv,
                rank_bm25: rb,
                rank_graph: rg,
                snippet,
            });
        }
        self.touch_many(&out.iter().map(|s| s.record.id).collect::<Vec<_>>())?;
        tracing::debug!("检索完成 {}ms, 候选{}条", t0.elapsed().as_millis(), all_ids.len());
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashEmbedder;
    use crate::model::MemoryKind;

    #[test]
    fn adaptive_weights_follow_query_intent() {
        assert_eq!(adaptive_channel_weights("when did Alice move") .0, "temporal");
        assert_eq!(adaptive_channel_weights("who is related to Alice").0, "relational");
        assert_eq!(adaptive_channel_weights("why did this happen").0, "semantic");
        assert_eq!(adaptive_channel_weights("alice@example.com").0, "exactish");
        assert_eq!(adaptive_channel_weights("tell me about Alice").0, "default");
    }

    #[test]
    fn test_hybrid_search() {
        let dir = tempfile::tempdir().unwrap().into_path();
        let s = MemoryStore::open(&dir.join("m.db"), &dir.join("fts"), Box::new(HashEmbedder::new(64))).unwrap();
        let mut a = MemoryRecord::new(Tier::Knowledge, MemoryKind::Fact, "麒麟AI", "银河麒麟操作系统内置 AI 运行时，提供文本嵌入服务");
        a.entities = vec!["银河麒麟".into()];
        s.put(a).unwrap();
        let mut b = MemoryRecord::new(Tier::Knowledge, MemoryKind::Case, "案例", "用户在办公场景偏好使用 WPS 输出 pdf 文档");
        b.entities = vec!["WPS".into()];
        s.put(b).unwrap();

        let hits = s.search(&SearchParams::new("银河麒麟 嵌入")).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].record.title, "麒麟AI");

        // 场景过滤
        let mut p = SearchParams::new("文档");
        p.scenes = Some(vec!["office".into()]);
        let hits2 = s.search(&p).unwrap();
        assert!(hits2.iter().all(|h| h.record.scene == "office"));

        // 通道开关：关向量后 BM25 仍可召回
        let mut p2 = SearchParams::new("银河麒麟");
        p2.use_vec = false;
        let hits3 = s.search(&p2).unwrap();
        assert!(!hits3.is_empty());
    }
}
