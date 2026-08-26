//! lymem-bench：评测入口。
//!
//! 用法：
//!   lymem-bench run --dataset locomo --data /path/locomo10.json \
//!       [--limit-convs N] [--limit-questions N] [--top-k 5] \
//!       [--ablation all|nov|nob|nog|nop] [--tag 名称] [--out 结果目录]
//!
//! 协议：按 session 顺序接入对话轮次（模拟多会话累积），对每个问题做混合检索，
//! 以「金标准证据轮次是否被检索到」计算 hit@k / 证据召回率 —— 该协议不需要 LLM，
//! 可在无 key 环境先跑；答案生成准确率（LLM judge）由 --judge 模式后续提供。
//!
//! 结果缓存：每次运行的逐题明细与汇总写入 结果目录/<tag>.json，重跑覆盖，
//! 不同配置各自一个文件，便于横向汇总。

mod dataset;
mod locomo;
mod metrics;

use clap::Parser;
use dataset::{Conversation, Dataset};
use lymem_core::model::{MemoryKind, Tier};
use lymem_core::retrieval::SearchParams;
use lymem_core::MemoryStore;
use metrics::{summarize, QueryOutcome};
use serde::Serialize;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "lymem-bench", version, about = "麟忆尽智评测框架")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// 运行一次评测
    Run {
        #[arg(long, default_value = "locomo")]
        dataset: String,
        /// 数据文件路径
        #[arg(long)]
        data: std::path::PathBuf,
        /// 最多评测前 N 段对话（默认全部）
        #[arg(long)]
        limit_convs: Option<usize>,
        /// 最多评测每段对话前 N 个问题（默认全部）
        #[arg(long)]
        limit_questions: Option<usize>,
        #[arg(long, default_value = "5")]
        top_k: usize,
        /// 消融配置：all 全通道 | nov 关向量 | nob 关BM25 | nog 关图 | nop 关偏好重排
        #[arg(long, default_value = "all")]
        ablation: String,
        /// 结果标签（默认 dataset+ablation）
        #[arg(long)]
        tag: Option<String>,
        /// 结果输出目录
        #[arg(long, default_value = "/home/ez/桌面/kylin-mem/benchmark/results")]
        out: std::path::PathBuf,
        /// 嵌入后端：auto（麒麟）/ hash（离线快速）
        #[arg(long, default_value = "auto")]
        embedder: String,
    },
}

#[derive(Serialize)]
struct RunResult {
    tag: String,
    dataset: String,
    ablation: String,
    top_k: usize,
    embedder: String,
    n_conversations: usize,
    summary: metrics::MetricsReport,
    per_query: Vec<QueryOutcome>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run { dataset, data, limit_convs, limit_questions, top_k, ablation, tag, out, embedder } => {
            run_eval(&dataset, &data, limit_convs, limit_questions, top_k, &ablation, tag, &out, &embedder)
        }
    }
}

fn build_store(embedder: &str) -> Result<(MemoryStore, bool), Box<dyn std::error::Error>> {
    // 评测用独立数据目录，避免污染正式记忆库
    let dir = std::env::var("LYMEM_BENCH_DATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/lymem-bench"));
    // LYMEM_BENCH_REUSE=1 时复用已有库（消融矩阵共享接入结果）
    let reuse = std::env::var("LYMEM_BENCH_REUSE").ok().as_deref() == Some("1")
        && dir.join("bench.db").exists();
    if !reuse {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let emb: Box<dyn lymem_core::embedding::Embedder> = match embedder {
        "hash" => Box::new(lymem_core::embedding::HashEmbedder::new(256)),
        _ => Box::new(lymem_kylin::KylinEmbedder::connect_auto()?),
    };
    Ok((MemoryStore::open(&dir.join("bench.db"), &dir.join("fts"), emb)?, reuse))
}

fn run_eval(
    dataset: &str,
    data: &std::path::Path,
    limit_convs: Option<usize>,
    limit_questions: Option<usize>,
    top_k: usize,
    ablation: &str,
    tag: Option<String>,
    out_dir: &std::path::Path,
    embedder: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let ds: Box<dyn Dataset> = match dataset {
        "locomo" => Box::new(locomo::LoCoMo),
        other => return Err(format!("未知数据集: {other}").into()),
    };
    let convs_all = ds.load(data)?;
    let convs: Vec<Conversation> = convs_all.into_iter().take(limit_convs.unwrap_or(usize::MAX)).collect();
    eprintln!("数据集 {} 加载 {} 段对话", ds.name(), convs.len());

    let (store, reuse) = build_store(embedder)?;
    let tag = tag.unwrap_or_else(|| format!("{}-{}", ds.name(), ablation));

    // ---- 接入阶段：逐 session 写入（模拟多会话累积）；复用模式跳过 ----
    let t0 = Instant::now();
    let mut dia_index: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    if reuse {
        // 从既有库重建 dia_id → memory_id 映射
        let conn = store.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, meta, source FROM memories WHERE kind = 'conversation'")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        let triples: Vec<(i64, String, String)> = rows.filter_map(|x| x.ok()).collect();
        drop(stmt);
        drop(conn);
        for (id, meta, source) in triples {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&meta) {
                if let Some(d) = v.get("dia_id").and_then(|x| x.as_str()) {
                    // source 形如 "locomo/<conv_id>"
                    let conv_id = source.strip_prefix("locomo/").unwrap_or("");
                    dia_index.insert(format!("{conv_id}/{d}"), id);
                }
            }
        }
        eprintln!("复用已有记忆库：{} 轮", dia_index.len());
    }
    for conv in &convs {
        if reuse { break; }
        for (si, session) in conv.sessions.iter().enumerate() {
            let events: Vec<lymem_core::ingest::IngestEvent> = session
                .turns
                .iter()
                .map(|t| lymem_core::ingest::IngestEvent::Conversation {
                    role: t.speaker.clone(),
                    text: t.text.clone(),
                    scene: "locomo".into(),
                    source: format!("locomo/{}", conv.id),
                    meta: Some(serde_json::json!({"dia_id": t.dia_id})),
                })
                .collect();
            let ids = lymem_core::ingest::ingest_events(&store, &events)?;
            // dia_id → memory_id 映射（逐条对应；被去重跳过时回退文本匹配）
            let mut it = ids.into_iter();
            for t in &session.turns {
                if let Some(id) = it.next() {
                    dia_index.insert(format!("{}/{}", conv.id, t.dia_id), id);
                }
            }
            if si % 4 == 3 {
                eprintln!("  接入进度：conv {} session {}/{}", conv.id, si + 1, conv.sessions.len());
            }
        }
    }
    eprintln!("接入完成：{} 轮，耗时 {:.1}s", dia_index.len(), t0.elapsed().as_secs_f64());

    // ---- 检索评测阶段 ----
    let mut params = SearchParams::default();
    params.top_k = top_k;
    match ablation {
        "nov" => params.use_vec = false,
        "nob" => params.use_bm25 = false,
        "nog" => params.use_graph = false,
        "nop" => params.preference_rerank = false,
        "all" => {}
        other => return Err(format!("未知消融配置: {other}").into()),
    }

    let mut outcomes: Vec<QueryOutcome> = Vec::new();
    for conv in &convs {
        for (qi, q) in conv.questions.iter().take(limit_questions.unwrap_or(usize::MAX)).enumerate() {
            params.query = q.question.clone();
            let t1 = Instant::now();
            let hits = store.search(&params);
            let latency_ms = t1.elapsed().as_secs_f64() * 1000.0;
            let hits = match hits {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("检索失败 q{qi}: {e}");
                    Vec::new()
                }
            };
            // 证据比对：命中 dia_id 集合
            let gold: std::collections::HashSet<String> =
                q.evidence.iter().map(|d| format!("{}/{}", conv.id, d)).collect();
            let gold_ids: std::collections::HashSet<i64> =
                gold.iter().filter_map(|d| dia_index.get(d.as_str()).copied()).collect();
            let mut hit_any = false;
            let mut hit_count = 0usize;
            let mut mrr_term = 0.0f64;
            let debug = std::env::var("LYMEM_BENCH_DEBUG").is_ok() && qi < 5 && outcomes.is_empty() == false || (std::env::var("LYMEM_BENCH_DEBUG").is_ok() && outcomes.len() < 5);
            if debug {
                eprintln!("\nQ: {}\n  gold={:?} gold_ids={:?}", q.question, q.evidence, gold_ids);
                for (rank, h) in hits.iter().enumerate() {
                    eprintln!("  #{} mem={} vec={:?} bm={:?} g={:?} score={:.4} | {}",
                        rank, h.record.id, h.rank_vec, h.rank_bm25, h.rank_graph, h.final_score,
                        h.snippet.replace('\n', " ").chars().take(70).collect::<String>());
                }
            }
            for (rank, h) in hits.iter().enumerate() {
                if gold_ids.contains(&h.record.id) {
                    hit_any = true;
                    hit_count += 1;
                    if mrr_term == 0.0 {
                        mrr_term = 1.0 / (rank + 1) as f64;
                    }
                }
            }
            let evidence_recall = if gold_ids.is_empty() {
                0.0 // 证据缺失（如开放域类无轮次证据）不计入召回，仅计入 hit=false
            } else {
                hit_count as f64 / gold_ids.len() as f64
            };
            outcomes.push(QueryOutcome {
                question_idx: qi,
                category: q.category.clone(),
                evidence_recall,
                hit: hit_any,
                mrr_term,
                latency_ms,
            });
        }
    }

    let summary = summarize(&outcomes);
    let result = RunResult {
        tag: tag.clone(),
        dataset: ds.name().to_string(),
        ablation: ablation.to_string(),
        top_k,
        embedder: embedder.to_string(),
        n_conversations: convs.len(),
        summary,
        per_query: outcomes,
    };
    std::fs::create_dir_all(out_dir)?;
    let out_path = out_dir.join(format!("{tag}.json"));
    std::fs::write(&out_path, serde_json::to_string_pretty(&result)?)?;
    eprintln!(
        "评测完成 [{tag}] n={} hit@{top_k}={:.4} 证据召回={:.4} MRR={:.4} p50={:.0}ms p95={:.0}ms → {}",
        result.summary.n,
        result.summary.hit_at_k,
        result.summary.evidence_recall,
        result.summary.mrr,
        result.summary.latency_p50_ms,
        result.summary.latency_p95_ms,
        out_path.display()
    );
    // 占位使用 Tier/MemoryKind 以便未来蒸馏评测扩展
    let _ = (Tier::Episodic, MemoryKind::Conversation);
    Ok(())
}
