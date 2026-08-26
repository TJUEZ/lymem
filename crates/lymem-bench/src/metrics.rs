//! 检索评测指标：hit@k / 证据召回率 / MRR / 延迟分布。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct QueryOutcome {
    pub question_idx: usize,
    pub category: String,
    /// 命中的证据比例 |检索∩金标| / |金标|
    pub evidence_recall: f64,
    /// 是否至少命中一条证据
    pub hit: bool,
    /// 金标证据首次出现的排名倒数（未命中为 0）
    pub mrr_term: f64,
    pub latency_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricsReport {
    pub n: usize,
    pub hit_at_k: f64,
    pub evidence_recall: f64,
    pub mrr: f64,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub by_category: Vec<(String, CategoryStats)>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CategoryStats {
    pub n: usize,
    pub hit_at_k: f64,
    pub evidence_recall: f64,
}

pub fn summarize(outcomes: &[QueryOutcome]) -> MetricsReport {
    let n = outcomes.len();
    let mut lat: Vec<f64> = outcomes.iter().map(|o| o.latency_ms).collect();
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pick = |q: f64| -> f64 {
        if lat.is_empty() {
            0.0
        } else {
            let i = (((lat.len() as f64 - 1.0) * q).round()) as usize;
            lat[i.min(lat.len() - 1)]
        }
    };
    let mut by_cat: std::collections::BTreeMap<String, CategoryStats> = Default::default();
    for o in outcomes {
        let e = by_cat.entry(o.category.clone()).or_default();
        e.n += 1;
        e.hit_at_k += if o.hit { 1.0 } else { 0.0 };
        e.evidence_recall += o.evidence_recall;
    }
    let by_category = by_cat
        .into_iter()
        .map(|(k, mut v)| {
            if v.n > 0 {
                v.hit_at_k /= v.n as f64;
                v.evidence_recall /= v.n as f64;
            }
            (k, v)
        })
        .collect();
    MetricsReport {
        n,
        hit_at_k: outcomes.iter().map(|o| if o.hit { 1.0 } else { 0.0 }).sum::<f64>() / n.max(1) as f64,
        evidence_recall: outcomes.iter().map(|o| o.evidence_recall).sum::<f64>() / n.max(1) as f64,
        mrr: outcomes.iter().map(|o| o.mrr_term).sum::<f64>() / n.max(1) as f64,
        latency_p50_ms: pick(0.5),
        latency_p95_ms: pick(0.95),
        by_category,
    }
}
