//! lymem CLI：记忆库的命令行入口。

use std::io::Read;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use lymem_core::forget::{ForgetMode, ForgetScope};
use lymem_core::llm_hook::LlmJudge;
use lymem_core::preference::PreferenceSet;
use lymem_core::retrieval::SearchParams;
use lymem_core::MemoryStore;

#[derive(Parser)]
#[command(name = "lymem", version, about = "麟忆尽智 lymem —— OS Agent 记忆管理命令行")]
struct Cli {
    /// 数据目录（默认 ~/.local/share/lymem）
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// 嵌入后端：auto（麒麟优先）/ hash（离线测试）
    #[arg(long, global = true, default_value = "auto")]
    embedder: String,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 系统状态
    Status,
    /// 批量接入事件（JSON 数组，文件或 stdin）
    Ingest {
        /// JSON 文件路径；缺省读 stdin
        file: Option<String>,
    },
    /// 混合检索
    Search {
        query: String,
        #[arg(long, default_value = "8")]
        top_k: usize,
        /// 消融：关闭向量通道
        #[arg(long)]
        no_vec: bool,
        /// 消融：关闭 BM25 通道
        #[arg(long)]
        no_bm25: bool,
        /// 消融：关闭图扩展通道
        #[arg(long)]
        no_graph: bool,
        /// 消融：关闭偏好重排
        #[arg(long)]
        no_pref: bool,
        /// 输出 JSON（便于脚本处理）
        #[arg(long)]
        json: bool,
    },
    /// 偏好管理
    Pref {
        #[command(subcommand)]
        cmd: PrefCmd,
    },
    /// 冲突记录
    Conflicts,
    /// 自然语言遗忘
    Forget {
        /// 遗忘指令
        instruction: String,
        /// --preview 仅预览；--exec 软删；--purge 硬清除
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        exec: bool,
        #[arg(long)]
        purge: bool,
    },
    /// 蒸馏：情景 → 知识
    Consolidate,
    /// 启动 HTTP 服务
    Serve {
        #[arg(long, default_value = "8801")]
        port: u16,
    },
}

#[derive(Subcommand)]
enum PrefCmd {
    /// 列出当前偏好
    List {
        #[arg(long)]
        scene: Option<String>,
    },
    /// 查看某键版本历史
    History { key: String },
    /// 手动设置偏好（值为 JSON）
    Set {
        key: String,
        value: String,
        #[arg(long, default_value = "*")]
        scenes: String,
    },
    /// 触发规则快通道（从近期工具调用提取偏好）
    MineRules,
}

fn open_store(cli: &Cli) -> Result<MemoryStore, Box<dyn std::error::Error>> {
    // 与 lymem-server 对齐：--data-dir > LYMEM_DATA_DIR > ~/.local/share/lymem
    let dir = cli.data_dir.clone().or_else(|| std::env::var("LYMEM_DATA_DIR").ok().map(PathBuf::from)).unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".local/share/lymem")
    });
    let embedder: Box<dyn lymem_core::embedding::Embedder> = match cli.embedder.as_str() {
        "hash" => Box::new(lymem_core::embedding::HashEmbedder::new(64)),
        _ => Box::new(lymem_kylin::KylinEmbedder::connect_auto()?),
    };
    Ok(MemoryStore::open(&dir.join("lymem.db"), &dir.join("fts"), embedder)?)
}

fn judge() -> Box<dyn LlmJudge> {
    lymem_llm::judge_from_env()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    let cli = Cli::parse();

    if matches!(cli.cmd, Cmd::Serve { .. }) {
        // serve 走独立二进制，避免 CLI 依赖 axum
        eprintln!("请使用 lymem-server 启动服务：LYMEM_PORT=8801 lymem-server");
        std::process::exit(2);
    }

    let store = open_store(&cli)?;
    let judge = judge();

    match &cli.cmd {
        Cmd::Status => {
            let counts: std::collections::BTreeMap<String, i64> = {
                let conn = store.conn.lock().unwrap();
                let mut stmt =
                    conn.prepare("SELECT tier, COUNT(*) FROM memories WHERE tombstone = 0 AND is_current = 1 GROUP BY tier")?;
                let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
                rows.filter_map(|r| r.ok()).collect()
            };
            let prefs = store.effective_preferences(None)?;
            println!("嵌入后端: {} (dim={})", embedder_name(&store), store.vec_dim);
            println!("LLM: {}", llm_name(judge.as_ref()));
            println!("记忆条目: {counts:?}");
            println!("当前偏好: {} 项", prefs.len());
        }
        Cmd::Ingest { file } => {
            let text = match file {
                Some(f) => std::fs::read_to_string(f)?,
                None => {
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf
                }
            };
            let events: Vec<lymem_core::ingest::IngestEvent> = serde_json::from_str(&text)?;
            let ids = lymem_core::ingest::ingest_events(&store, &events)?;
            println!("{}", serde_json::json!({"ok": true, "ingested": ids.len(), "ids": ids}));
        }
        Cmd::Search { query, top_k, no_vec, no_bm25, no_graph, no_pref, json } => {
            let mut p = SearchParams::new(query.clone());
            p.top_k = *top_k;
            p.use_vec = !no_vec;
            p.use_bm25 = !no_bm25;
            p.use_graph = !no_graph;
            p.preference_rerank = !no_pref;
            let t0 = std::time::Instant::now();
            let hits = store.search(&p)?;
            let ms = t0.elapsed().as_millis();
            if *json {
                println!("{}", serde_json::json!({"took_ms": ms, "hits": hits}));
            } else {
                println!("检索耗时 {ms}ms，命中 {} 条：", hits.len());
                for h in &hits {
                    println!(
                        "  #{} [{:.4}] {}（vec={:?} bm25={:?} graph={:?}）",
                        h.record.id, h.final_score, h.record.title, h.rank_vec, h.rank_bm25, h.rank_graph
                    );
                    println!("      {}", h.snippet.replace('\n', " ").chars().take(100).collect::<String>());
                }
            }
        }
        Cmd::Pref { cmd } => match cmd {
            PrefCmd::List { scene } => {
                let prefs = store.effective_preferences(scene.as_deref())?;
                println!("{}", serde_json::to_string_pretty(&prefs)?);
            }
            PrefCmd::History { key } => {
                let hist = store.preference_history(key)?;
                println!("{}", serde_json::to_string_pretty(&hist)?);
            }
            PrefCmd::Set { key, value, scenes } => {
                let v: serde_json::Value = serde_json::from_str(value)
                    .map_err(|e| format!("value 必须为合法 JSON: {e}"))?;
                let view = store.set_preference(&PreferenceSet {
                    key: key.clone(),
                    value: v,
                    evidence: vec![serde_json::json!({"via": "cli"})],
                    source: "manual".into(),
                    confidence: 1.0,
                    scenes: scenes.split(',').map(|s| s.trim().to_string()).collect(),
                    force: true,
                })?;
                println!("{}", serde_json::to_string_pretty(&view)?);
            }
            PrefCmd::MineRules => {
                let updates = store.preference_rules_from_tools(200, 3, 0.6)?;
                println!("规则通道更新 {} 项偏好", updates.len());
                for u in updates {
                    println!("  {} = {} (v{}, conf={:.2})", u.key, serde_json::to_string(&u.value).unwrap(), u.version, u.confidence);
                }
            }
        },
        Cmd::Conflicts => {
            let stats = store.conflict_stats()?;
            println!("{}", serde_json::to_string_pretty(&stats)?);
            let all = store.conflicts_all()?;
            for c in all.iter().take(50) {
                println!("  {} -> {} [{}] {} ({}) {}", c["old_id"], c["new_id"], c["type"], c["resolution"], c["decided_by"], c["reason"]);
            }
        }
        Cmd::Forget { instruction, preview, exec, purge } => {
            let scope: ForgetScope = store.parse_forget_scope(instruction, Some(judge.as_ref()))?;
            println!("解析范围: {}", serde_json::to_string(&scope)?);
            if *preview {
                let targets = store.forget_preview(&scope, 50)?;
                println!("命中 {} 条：", targets.len());
                for t in targets {
                    println!("  #{} [{}] {} ({})", t.id, t.tier, t.title, t.scene);
                }
            }
            if *exec || *purge {
                let mode = if *purge { ForgetMode::Purge } else { ForgetMode::Tombstone };
                let n = store.forget_execute(&scope, mode)?;
                println!("已执行 {:?}，影响 {n} 条", mode);
            }
            if !*preview && !*exec && !*purge {
                println!("（加 --preview 预览 / --exec 软删 / --purge 硬清除）");
            }
        }
        Cmd::Consolidate => {
            let created = store.consolidate_episodic(Some(judge.as_ref()), 3, 4)?;
            println!("蒸馏生成 {} 条知识", created.len());
            for id in created {
                if let Some(r) = store.get(id)? {
                    println!("  #{} {}", id, r.title);
                }
            }
        }
        Cmd::Serve { .. } => unreachable!(),
    }
    Ok(())
}

fn embedder_name(store: &MemoryStore) -> String {
    // HashEmbedder dim=64；麒麟 768。够用的粗略区分展示
    if store.vec_dim == 64 { "hash".into() } else { format!("kylin(dim={})", store.vec_dim) }
}

fn llm_name(j: &dyn LlmJudge) -> String {
    if j.complete("回复 ok", "ping").is_some() { "已连接".into() } else { "未配置（规则兜底）".into() }
}
