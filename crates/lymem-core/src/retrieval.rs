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

impl MemoryStore {
    /// 三路混合检索主入口。
    /// 流程：嵌入查询 → 向量 KNN / BM25 / 图扩展 → 过滤 → RRF 融合 → 偏好重排 → 触摸计数。
    pub fn search(&self, p: &SearchParams) -> Result<Vec<ScoredMemory>> {
        let t0 = std::time::Instant::now();
        let k = p.top_k.max(1);
        let cand = k * p.candidate_multiplier.max(1);

        // 通道 1：向量（场景过滤时超采样，避免候选被其它场景占满）
        let mut vec_rank: Vec<(i64, f64, i64)> = Vec::new(); // (memory_id, distance, chunk_id)
        if p.use_vec {
            let qv = self.embedder.embed_one(&p.query)?;
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
            if let Ok(hits) = fts.search(&p.query, None, scene, cand) {
                bm25_rank = hits;
            }
        }

        // 通道 3：图扩展（查询词模糊匹配实体 → 邻域记忆）
        let mut graph_rank: Vec<(i64, usize)> = Vec::new();
        if p.use_graph {
            // 取查询中的关键词（简单策略：整体 + 空格分词）
            let terms: Vec<String> = p.query.split_whitespace().map(|s| s.to_string()).chain(std::iter::once(p.query.clone())).collect();
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

        // 收集全部候选 id（去重）
        let mut all_ids: Vec<i64> = Vec::new();
        for (mid, _, _) in &vec_rank {
            if !all_ids.contains(mid) {
                all_ids.push(*mid);
            }
        }
        for (mid, _, _, _) in &bm25_rank {
            if !all_ids.contains(mid) {
                all_ids.push(*mid);
            }
        }
        for (mid, _) in &graph_rank {
            if !all_ids.contains(mid) {
                all_ids.push(*mid);
            }
        }

        // 拉取记录并应用过滤器
        let mut records: Vec<MemoryRecord> = Vec::new();
        for id in &all_ids {
            if let Some(rec) = self.get(*id)? {
                if !rec.tombstone && rec.is_current
                    && tier_ok(rec.tier.as_str(), &p.tiers)
                    && (p.scenes.is_none() || p.scenes.as_ref().unwrap().iter().any(|s| *s == rec.scene || s == "*"))
                    && (p.after.is_none() || rec.created_at >= p.after.unwrap())
                    && (p.before.is_none() || rec.created_at <= p.before.unwrap())
                {
                    records.push(rec);
                }
            }
        }

        // RRF 融合
        let mut fused: Vec<(i64, f64, Option<usize>, Option<usize>, Option<usize>, String)> = Vec::new();
        for rec in &records {
            let id = rec.id;
            let (mut score, mut rv, mut rb, mut rg) = (0.0, None, None, None);
            let mut snippet = String::new();
            let w_vec = std::env::var("LYMEM_W_VEC").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
            for (rank, (mid, dist, chunk)) in vec_rank.iter().enumerate() {
                if *mid == id {
                    rv = Some(rank + 1);
                    score += w_vec / (p.rrf_k + rank as f64 + 1.0);
                    // 余弦相似度展示用
                    snippet = self.chunk_content(*chunk).unwrap_or_default().unwrap_or_default();
                    let _ = dist;
                    break;
                }
            }
            let w_bm25 = std::env::var("LYMEM_W_BM25").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
            for (rank, (mid, _, _, snip)) in bm25_rank.iter().enumerate() {
                if *mid == id {
                    rb = Some(rank + 1);
                    score += w_bm25 / (p.rrf_k + rank as f64 + 1.0);
                    if snippet.is_empty() {
                        snippet = snip.clone();
                    }
                    break;
                }
            }
            let w_graph = std::env::var("LYMEM_W_GRAPH").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
            for (rank, (mid, _hop)) in graph_rank.iter().enumerate() {
                if *mid == id {
                    rg = Some(rank + 1);
                    score += w_graph / (p.rrf_k + rank as f64 + 1.0);
                    break;
                }
            }
            if snippet.is_empty() {
                snippet = rec.content.chars().take(120).collect();
            }
            fused.push((id, score, rv, rb, rg, snippet));
        }
        fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // 偏好重排：命中偏好作用域的记忆获得提升
        let prefs = if p.preference_rerank { self.effective_preferences(None)? } else { Vec::new() };
        let mut out: Vec<ScoredMemory> = Vec::new();
        for (id, rrf, rv, rb, rg, snippet) in fused.into_iter().take(k) {
            let rec = records.iter().find(|r| r.id == id).cloned().unwrap();
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
        self.touch(&out.iter().map(|s| s.record.id).collect::<Vec<_>>())?;
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
