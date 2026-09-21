//! 记忆中枢（Memory Hub）：终端多 agent 记忆源发现、汇聚导入、记忆调度分发、
//! 全局总索引与总管巡检。
//!
//! - 源发现：扫描本机常见 agent 记忆载体（zcode/claude-code/codex/gemini/opencode/dsh
//!   及通用 AGENTS.md/CLAUDE.md），零配置发现，也可经 LYMEM_HUB_EXTRA 注册自定义源；
//! - 导入：解析为离散记忆条目（指纹去重）→ 清洗/敏感分级/实体抽取 → 入库，
//!   来源标记 agent，映射写入 hub_imports 支持增量重导；
//! - 分发：把 lymem 记忆/偏好/实体摘要以受管标记块写回任意 agent 的记忆/规则文件
//!   （只动标记块内内容，不碰用户手写内容），全程审计；
//! - 总索引：借鉴 Semantica 离线索引图规则的全规则实现 —— 实体抽取、共现建边（权重
//!   累计）、标签传播社区检测、跨源实体链接统计，端侧离线可跑；
//! - 矛盾扫描：按实体聚合跨源记忆，数值/时序矛盾成对检测，LLM 可选复核；
//! - 巡检：重复指纹分组、失活记忆、墓碑积压、来源体量统计，输出可执行维护计划。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::ingest::{clean_text, extract_entities, fingerprint};
use crate::model::{MemoryKind, MemoryRecord, Sensitivity, Tier};
use crate::store::MemoryStore;

// ---------------- 源发现 ----------------

/// 源载体类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    /// 记忆型 markdown（zcode/claude memory：frontmatter 条目或索引行）
    MemoryMd,
    /// 规则/指令型 markdown（CLAUDE.md / AGENTS.md / GEMINI.md，按小节切条）
    RulesMd,
    /// Codex CLI 记忆 SQLite（stage1_outputs 表）
    CodexSqlite,
    /// Agent 会话历史 JSONL（每行一个带 text/session_id/ts 的对象）
    SessionJsonl,
    /// JSON 存储（如 dsh workspace.json，按键切条）
    JsonStore,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::MemoryMd => "memory_md",
            SourceKind::RulesMd => "rules_md",
            SourceKind::CodexSqlite => "codex_sqlite",
            SourceKind::SessionJsonl => "session_jsonl",
            SourceKind::JsonStore => "json",
        }
    }
}

/// 发现的一个 agent 记忆源
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSource {
    pub id: String,
    /// 所属 agent（zcode / codex / claude-code / gemini / opencode / dsh / generic）
    pub agent: String,
    pub kind: SourceKind,
    pub path: String,
    pub exists: bool,
    pub writable: bool,
    pub items: usize,
    pub bytes: u64,
    pub mtime: Option<i64>,
    /// 解析/读取异常说明（None 为正常）
    pub note: Option<String>,
}

fn home() -> PathBuf {
    std::env::var("HOME").unwrap_or_else(|_| ".".into()).into()
}

fn probe(kind: SourceKind, agent: &str, path: &Path, writable: bool, id_hint: &str) -> AgentSource {
    let exists = path.exists();
    let (items, bytes, mtime, note) = if !exists {
        (0, 0, None, None)
    } else {
        match count_items(kind, path) {
            Ok(n) => {
                let meta = std::fs::metadata(path).ok();
                let mt = meta.as_ref().and_then(|m| m.modified().ok()).map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
                });
                let bytes = if path.is_dir() {
                    dir_size(path)
                } else {
                    meta.map(|m| m.len()).unwrap_or(0)
                };
                (n, bytes, mt, None)
            }
            Err(e) => (0, 0, None, Some(e)),
        }
    };
    AgentSource {
        id: format!("{agent}/{id_hint}"),
        agent: agent.to_string(),
        kind,
        path: path.to_string_lossy().to_string(),
        exists,
        writable,
        items,
        bytes,
        mtime,
        note,
    }
}

fn dir_size(p: &Path) -> u64 {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            if let Ok(md) = e.metadata() {
                if md.is_file() {
                    n += md.len();
                } else if md.is_dir() {
                    n += dir_size(&e.path());
                }
            }
        }
    }
    n
}

/// 自定义源注册：LYMEM_HUB_EXTRA = JSON 数组 [{"agent":"x","kind":"rules_md","path":"..."}]
fn extra_sources() -> Vec<AgentSource> {
    let raw = std::env::var("LYMEM_HUB_EXTRA").unwrap_or_default();
    if raw.trim().is_empty() {
        return Vec::new();
    }
    #[derive(Deserialize)]
    struct Extra {
        agent: String,
        kind: String,
        path: String,
    }
    let ok = |k: &str| match k {
        "memory_md" => Some(SourceKind::MemoryMd),
        "rules_md" => Some(SourceKind::RulesMd),
        "codex_sqlite" => Some(SourceKind::CodexSqlite),
        "session_jsonl" => Some(SourceKind::SessionJsonl),
        "json" => Some(SourceKind::JsonStore),
        _ => None,
    };
    serde_json::from_str::<Vec<Extra>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            let kind = ok(&e.kind)?;
            let id_hint = Path::new(&e.path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "src".into());
            Some(probe(kind, &e.agent, Path::new(&e.path), true, &id_hint))
        })
        .collect()
}

/// 扫描本机终端 agent 的记忆载体（存在的与可创建的分发目标都会列出）
pub fn discover_sources() -> Vec<AgentSource> {
    let h = home();
    let mut out = Vec::new();

    // zcode 记忆目录（每项目一个 memory/ 目录）
    let zc = h.join(".zcode/cli/memories/projects");
    if let Ok(rd) = std::fs::read_dir(&zc) {
        for e in rd.flatten() {
            let memdir = e.path().join("memory");
            if memdir.is_dir() {
                let proj = e.file_name().to_string_lossy().to_string();
                out.push(probe(SourceKind::MemoryMd, "zcode", &memdir, false, &format!("mem-{proj}")));
            }
        }
    }

    // claude-code 兼容布局
    out.push(probe(SourceKind::RulesMd, "claude-code", &h.join(".claude/CLAUDE.md"), true, "CLAUDE.md"));
    out.push(probe(SourceKind::MemoryMd, "claude-code", &h.join(".claude/memory"), false, "memory"));

    // Codex 可能同时产出 SQLite 中间记忆和 ~/.codex/memories 汇总文件；
    // 两者都读取，避免后台汇总尚未完成时漏掉已经可用的结果。
    out.push(probe(SourceKind::RulesMd, "codex", &h.join(".codex/AGENTS.md"), true, "AGENTS.md"));
    out.push(probe(SourceKind::MemoryMd, "codex", &h.join(".codex/memories"), false, "memories"));
    // Codex 的后台摘要库可能尚未产出 stage1_outputs；会话历史作为可选兜底源，
    // 仍由 lymem 的敏感扫描、去重和审计管线处理，不直接视为长期知识。
    out.push(probe(SourceKind::SessionJsonl, "codex", &h.join(".codex/history.jsonl"), false, "history.jsonl"));
    if let Ok(rd) = std::fs::read_dir(h.join(".codex")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("memories") && name.ends_with(".sqlite") {
                out.push(probe(SourceKind::CodexSqlite, "codex", &e.path(), false, &name));
            }
        }
    }

    // gemini / opencode / dsh
    out.push(probe(SourceKind::RulesMd, "gemini", &h.join(".gemini/GEMINI.md"), true, "GEMINI.md"));
    out.push(probe(SourceKind::RulesMd, "opencode", &h.join(".config/opencode/AGENTS.md"), true, "AGENTS.md"));
    out.push(probe(SourceKind::JsonStore, "dsh", &h.join(".dsh/storages/workspace.json"), false, "workspace.json"));

    // 通用规则文件
    out.push(probe(SourceKind::RulesMd, "generic", &h.join("AGENTS.md"), true, "AGENTS.md"));
    out.push(probe(SourceKind::RulesMd, "generic", &h.join("CLAUDE.md"), true, "CLAUDE.md"));

    // 服务从项目目录启动时，把项目级 Agent 规则也纳入发现。项目文件仅导入，
    // 分发仍写用户级受管文件，避免无意修改仓库中的团队规则。
    if let Ok(cwd) = std::env::current_dir() {
        let agents = cwd.join("AGENTS.md");
        if agents.is_file() {
            out.push(probe(SourceKind::RulesMd, "codex", &agents, false, "project-AGENTS.md"));
        }
        let claude = cwd.join("CLAUDE.md");
        if claude.is_file() {
            out.push(probe(SourceKind::RulesMd, "claude-code", &claude, false, "project-CLAUDE.md"));
        }
    }

    // lymem 自身共享记忆文件（纯分发目标）
    out.push(probe(SourceKind::RulesMd, "lymem", &data_dir().join("dispatch/lymem-share.md"), true, "lymem-share.md"));

    out.extend(extra_sources());
    out
}

/// 分发目标：存在的可写 markdown 源 + 虚拟目标（按需创建）
pub fn dispatch_targets() -> Vec<AgentSource> {
    let mut out: Vec<AgentSource> = discover_sources()
        .into_iter()
        .filter(|s| s.writable && s.kind == SourceKind::RulesMd)
        .collect();
    let h = home();
    // codex 记忆 SQLite 不可直写，其分发落 AGENTS.md
    let codex_db = h.join(".codex");
    if codex_db.is_dir() && !out.iter().any(|s| s.id == "codex/AGENTS.md") {
        out.push(probe(SourceKind::RulesMd, "codex", &h.join(".codex/AGENTS.md"), true, "AGENTS.md"));
    }
    // dsh 无规则文件：建 lymem-memory.md
    if h.join(".dsh").is_dir() && !out.iter().any(|s| s.id == "dsh/lymem-memory.md") {
        out.push(probe(SourceKind::RulesMd, "dsh", &h.join(".dsh/lymem-memory.md"), true, "lymem-memory.md"));
    }
    out
}

fn data_dir() -> PathBuf {
    std::env::var("LYMEM_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share/lymem"))
}

// ---------------- 条目解析 ----------------

/// 源中的一个离散记忆条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceItem {
    pub title: String,
    pub content: String,
}

fn count_items(kind: SourceKind, path: &Path) -> std::result::Result<usize, String> {
    Ok(parse_source(kind, path)?.len())
}

/// 解析一个源为离散条目列表
pub fn parse_source(kind: SourceKind, path: &Path) -> std::result::Result<Vec<SourceItem>, String> {
    match kind {
        SourceKind::MemoryMd => parse_memory_dir(path),
        SourceKind::RulesMd => parse_rules_md(path),
        SourceKind::CodexSqlite => parse_codex_sqlite(path),
        SourceKind::SessionJsonl => parse_session_jsonl(path),
        SourceKind::JsonStore => parse_json_store(path),
    }
}

/// 记忆目录：每个 *.md 一条（frontmatter name 做标题）；MEMORY.md 索引逐行一条
fn parse_memory_dir(dir: &Path) -> std::result::Result<Vec<SourceItem>, String> {
    let mut out = Vec::new();
    let rd = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "md").unwrap_or(false)).collect();
    files.sort();
    for f in files {
        let body = std::fs::read_to_string(&f).map_err(|e| e.to_string())?;
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        if name == "MEMORY.md" {
            for line in body.lines() {
                let t = line.trim();
                if t.starts_with("- [") {
                    let title = t.trim_start_matches("- [").split("](").next().unwrap_or(t).to_string();
                    out.push(SourceItem { title, content: t.to_string() });
                }
            }
            continue;
        }
        let (front_name, body) = split_frontmatter(&body);
        let title = front_name.unwrap_or_else(|| {
            body.lines().find(|l| l.trim().starts_with('#')).map(|l| l.trim_start_matches('#').trim().to_string()).unwrap_or(name)
        });
        let content = body.trim().to_string();
        if !content.is_empty() {
            out.push(SourceItem { title, content });
        }
    }
    Ok(out)
}

fn split_frontmatter(body: &str) -> (Option<String>, &str) {
    if let Some(rest) = body.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let fm = &rest[..end];
            let name = fm.lines().find_map(|l| l.strip_prefix("name:")).map(|s| s.trim().to_string());
            return (name, &rest[end + 4..]);
        }
    }
    (None, body)
}

/// 规则 markdown：按 `## ` 小节切；无小节则按顶层 bullet 聚簇
fn parse_rules_md(path: &Path) -> std::result::Result<Vec<SourceItem>, String> {
    let body = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    Ok(split_rules(&body))
}

/// 供发现阶段与导入阶段共用
pub fn split_rules(body: &str) -> Vec<SourceItem> {
    let mut out = Vec::new();
    for sec in body.split("\n## ") {
        let sec = sec.trim();
        if sec.is_empty() {
            continue;
        }
        let (title, content) = if sec.starts_with("# ") || (sec.contains('\n') && !sec.starts_with("- ")) {
            let mut lines = sec.lines();
            let first = lines.next().unwrap_or("").trim_start_matches('#').trim().to_string();
            (first, sec.to_string())
        } else {
            (String::new(), sec.to_string())
        };
        if content.chars().count() < 4 {
            continue;
        }
        let title = if title.is_empty() {
            content.lines().next().unwrap_or("").trim_start_matches("- ").chars().take(40).collect()
        } else {
            title.chars().take(60).collect()
        };
        out.push(SourceItem { title, content });
    }
    out
}

/// Codex CLI 记忆库：stage1_outputs 的摘要（缺则原文截断）
fn parse_codex_sqlite(path: &Path) -> std::result::Result<Vec<SourceItem>, String> {
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT rollout_summary, raw_memory FROM stage1_outputs ORDER BY generated_at")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows.flatten() {
        let (summary, raw) = row;
        let text = if summary.trim().is_empty() { raw } else { summary };
        let text = clean_text(&text);
        if text.is_empty() {
            continue;
        }
        let title = text.lines().next().unwrap_or("").chars().take(48).collect();
        out.push(SourceItem { title, content: text });
    }
    Ok(out)
}

/// 会话历史只提取用户可读文本，不导入内部工具参数或运行时元数据。
fn parse_session_jsonl(path: &Path) -> std::result::Result<Vec<SourceItem>, String> {
    let body = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (line_no, line) in body.lines().enumerate() {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Some(text) = value.get("text").and_then(|v| v.as_str()) else { continue };
        let text = clean_text(text);
        if text.chars().count() < 2 {
            continue;
        }
        let session = value.get("session_id").and_then(|v| v.as_str()).unwrap_or("unknown");
        let title_text: String = text.lines().next().unwrap_or("").chars().take(42).collect();
        out.push(SourceItem {
            title: format!("会话 {} · {}", &session[..session.len().min(8)], title_text),
            content: text,
        });
        // 防止异常超大历史文件一次扫描占用过多内存；较新内容由后续增量扫描补入。
        if line_no >= 4_999 {
            break;
        }
    }
    Ok(out)
}

/// JSON 存储：顶层键值各一条
fn parse_json_store(path: &Path) -> std::result::Result<Vec<SourceItem>, String> {
    let body = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            out.push(SourceItem {
                title: k.clone(),
                content: format!("{k}: {}", serde_json::to_string_pretty(val).unwrap_or_default()),
            });
        }
    }
    Ok(out)
}

// ---------------- 导入 ----------------

/// 导入报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    pub source: String,
    /// 源内条目总数
    pub items: usize,
    pub imported: usize,
    /// 命中 hub_imports 已导入记录而跳过
    pub skipped_known: usize,
    /// 内容指纹与库内既有记忆重复而跳过
    pub skipped_dup: usize,
    /// 敏感阻断条数
    pub blocked: usize,
    pub ids: Vec<i64>,
}

fn tier_kind_of(kind: SourceKind) -> (Tier, MemoryKind) {
    match kind {
        SourceKind::MemoryMd => (Tier::Knowledge, MemoryKind::Fact),
        SourceKind::RulesMd => (Tier::Knowledge, MemoryKind::Workflow),
        SourceKind::CodexSqlite => (Tier::Knowledge, MemoryKind::Case),
        SourceKind::SessionJsonl => (Tier::Episodic, MemoryKind::Conversation),
        SourceKind::JsonStore => (Tier::Knowledge, MemoryKind::ManualConfig),
    }
}

/// 把一个源的全部条目导入记忆库（增量：已导入条目按 (source, hash) 跳过）
pub fn import_source(store: &MemoryStore, src: &AgentSource) -> Result<ImportReport> {
    let items = parse_source(src.kind, Path::new(&src.path)).map_err(CoreError::InvalidInput)?;
    let mut rep = ImportReport { source: src.id.clone(), items: items.len(), imported: 0, skipped_known: 0, skipped_dup: 0, blocked: 0, ids: Vec::new() };
    let (tier, kind) = tier_kind_of(src.kind);
    // 第一层判断“该来源的条目是否导过”；第二层比较规范化后的标题与正文，
    // 防止多个 Agent 复制了同一份规则后重复占用记忆库。
    let known: std::collections::HashSet<String> = {
        let conn = store.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT item_hash FROM hub_imports WHERE source = ?1")?;
        let rows = stmt.query_map(params![src.id], |r| r.get::<_, String>(0))?;
        let s: std::collections::HashSet<String> = rows.filter_map(|r| r.ok()).collect();
        drop(stmt);
        drop(conn);
        s
    };
    let mut existing: std::collections::HashSet<String> = store
        .list(None, None, false, 10_000)?
        .into_iter()
        .map(|r| fingerprint(&format!("{}|{}", r.title, r.content)))
        .collect();
    for item in &items {
        let hash = fingerprint(&format!("{}|{}", src.id, item.content));
        if known.contains(&hash) {
            rep.skipped_known += 1;
            continue;
        }
        let content = clean_text(&item.content);
        if content.is_empty() {
            continue;
        }
        let content_hash = fingerprint(&format!("{}|{}", clean_text(&item.title), content));
        if existing.contains(&content_hash) {
            rep.skipped_dup += 1;
            continue;
        }
        let findings = crate::sensitive::scan(&content);
        let level = crate::sensitive::level_of(&findings);
        if level == Sensitivity::Blocked {
            rep.blocked += 1;
            store.audit("hub_import_blocked", &serde_json::json!({"source": src.id, "title": item.title}))?;
            continue;
        }
        let content = if level != Sensitivity::None { crate::sensitive::redact(&content) } else { content };
        let mut rec = MemoryRecord::new(tier, kind, item.title.clone(), content);
        rec.source = src.agent.clone();
        rec.scene = "hub".into();
        rec.confidence = 0.8;
        rec.sensitivity = level;
        rec.entities = extract_entities(&format!("{} {}", rec.title, rec.content));
        rec.meta = serde_json::json!({"hub": {"source": src.id, "path": src.path, "hash": hash}});
        let id = store.put(rec)?;
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO hub_imports(source, item_hash, memory_id, at) VALUES (?1, ?2, ?3, ?4)",
                params![src.id, hash, id, now_ts()],
            )?;
        }
        rep.ids.push(id);
        rep.imported += 1;
        existing.insert(content_hash);
    }
    store.audit("hub_import", &serde_json::json!({"source": src.id, "imported": rep.imported, "known": rep.skipped_known}))?;
    Ok(rep)
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

// ---------------- 分发（调度） ----------------

/// 分发模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispatchMode {
    /// 受管块整体替换（幂等同步）
    Replace,
    /// 追加一个新受管块
    Append,
    /// 移除全部受管块
    Remove,
}

impl DispatchMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            DispatchMode::Replace => "replace",
            DispatchMode::Append => "append",
            DispatchMode::Remove => "remove",
        }
    }
}

pub const MARK_BEGIN: &str = "<!-- lymem:begin";
pub const MARK_END: &str = "<!-- lymem:end -->";

/// 分发报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchReport {
    pub target: String,
    pub path: String,
    pub mode: String,
    pub items: usize,
    pub bytes_written: u64,
    /// 写入目标文件是否为新建
    pub created: bool,
}

/// 生成受管标记块文本
pub fn managed_block(title: &str, lines: &[String]) -> String {
    let mut s = String::new();
    s.push_str(&format!("{MARK_BEGIN} {} -->\n", chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")));
    s.push_str(&format!("### {title}\n"));
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    s.push_str(MARK_END);
    s
}

/// 把受管块写入目标文件。只增删 lymem 受管块，不触碰文件其余内容。
pub fn write_dispatch(target_path: &Path, block: Option<&str>, mode: DispatchMode) -> Result<DispatchReport> {
    let existed = target_path.exists();
    if !existed {
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::InvalidInput(e.to_string()))?;
        }
        std::fs::write(target_path, "").map_err(|e| CoreError::InvalidInput(e.to_string()))?;
    }
    let body = std::fs::read_to_string(target_path).map_err(|e| CoreError::InvalidInput(e.to_string()))?;
    let stripped = strip_managed_blocks(&body);
    let new_body = match mode {
        DispatchMode::Remove => stripped,
        DispatchMode::Append => {
            let mut s = stripped.trim_end().to_string();
            if !s.is_empty() {
                s.push_str("\n\n");
            }
            s.push_str(block.ok_or_else(|| CoreError::InvalidInput("append 需要块内容".into()))?);
            s.push('\n');
            s
        }
        DispatchMode::Replace => {
            let mut s = stripped.trim_end().to_string();
            if !s.is_empty() {
                s.push_str("\n\n");
            }
            s.push_str(block.ok_or_else(|| CoreError::InvalidInput("replace 需要块内容".into()))?);
            s.push('\n');
            s
        }
    };
    std::fs::write(target_path, &new_body).map_err(|e| CoreError::InvalidInput(e.to_string()))?;
    Ok(DispatchReport {
        target: String::new(),
        path: target_path.to_string_lossy().to_string(),
        mode: mode.as_str().to_string(),
        items: 0,
        bytes_written: new_body.len() as u64,
        created: !existed,
    })
}

/// 去除文中全部 lymem 受管块
fn strip_managed_blocks(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(pos) = rest.find(MARK_BEGIN) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        rest = match after.find(MARK_END) {
            Some(e) => &after[e + MARK_END.len()..],
            None => "", // 未闭合块：整段丢弃
        };
    }
    out.push_str(rest);
    out
}

// ---------------- 全局总索引（Semantica 式离线规则图） ----------------

/// 实体统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityStat {
    pub name: String,
    pub degree: usize,
    pub memory_count: usize,
    /// 提及该实体的来源 agent
    pub sources: Vec<String>,
}

/// 社区（标签传播所得实体簇）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Community {
    pub label: String,
    pub size: usize,
    pub top: Vec<String>,
    pub sources: Vec<String>,
}

/// 总索引报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexReport {
    pub built_at: i64,
    pub duration_ms: u64,
    pub memories_indexed: usize,
    pub entities: usize,
    pub edges: usize,
    pub communities: Vec<Community>,
    pub top_entities: Vec<EntityStat>,
    /// 被 ≥2 个来源 agent 提及的实体（跨源枢纽，也是矛盾高发位）
    pub cross_source: Vec<EntityStat>,
}

/// 记录的实体名集合：声明的实体 + 规则抽取 + 非通用标题（build_index 与矛盾扫描共用）
fn entity_names_of(rec: &MemoryRecord) -> Vec<String> {
    let mut names = rec.entities.clone();
    names.extend(extract_entities(&format!("{} {}", rec.title, &rec.content.chars().take(1500).collect::<String>())));
    // 通用标题（会话 user / 工具调用 X / 行为 X / 配置 X）不作为实体
    let generic_title = ["会话", "工具调用", "行为", "配置"].iter().any(|p| rec.title.starts_with(p) && rec.title.contains(' '));
    if !rec.title.is_empty() && !generic_title && rec.title.chars().count() <= 24 {
        names.push(rec.title.clone());
    }
    names.retain(|n| {
        let c = n.chars().count();
        c >= 2 && c <= 48
    });
    names.sort();
    names.dedup();
    names.truncate(12);
    names
}

/// 全库重建总索引：实体抽取 → 共现建边（权重累计）→ 标签传播社区 → 报告存 graph_meta。
/// 纯规则离线，无 LLM；重复构建幂等（边按 (src,dst,relation,memory_id) 累计权重）。
pub fn build_index(store: &MemoryStore, max_memories: usize) -> Result<IndexReport> {
    let t0 = std::time::Instant::now();
    let records = store.list(None, None, false, max_memories)?;
    let mut ent_mem: HashMap<String, Vec<i64>> = HashMap::new();
    let mut ent_src: HashMap<String, Vec<String>> = HashMap::new();

    for rec in &records {
        let names = entity_names_of(rec);
        let mut ids = Vec::new();
        for n in &names {
            let eid = store.upsert_entity(n, guess_kind(n))?;
            ids.push(eid);
            ent_mem.entry(n.clone()).or_default().push(rec.id);
            // 跨源统计按 agent 归一（locomo/conv-26 → locomo）
            let agent = rec.source.split('/').next().unwrap_or(&rec.source);
            let srcs = ent_src.entry(n.clone()).or_default();
            if !srcs.iter().any(|s| s == agent) {
                srcs.push(agent.to_string());
            }
        }
        for w in ids.windows(2) {
            store.upsert_edge(w[0], w[1], "co", Some(rec.id))?;
        }
    }

    // 邻接表（co 边，无向权重和）
    let (entity_count, edge_count, adj) = {
        let conn = store.conn.lock().unwrap();
        let entities: usize = conn.query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))?;
        let edges: usize = conn.query_row("SELECT COUNT(*) FROM edges WHERE relation = 'co'", [], |r| r.get(0))?;
        let mut stmt = conn.prepare("SELECT src, dst, weight FROM edges WHERE relation = 'co'")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?)))?;
        let mut adj: HashMap<i64, HashMap<i64, f64>> = HashMap::new();
        for row in rows.flatten() {
            let (a, b, w) = row;
            *adj.entry(a).or_default().entry(b).or_insert(0.0) += w;
            *adj.entry(b).or_default().entry(a).or_insert(0.0) += w;
        }
        (entities, edges, adj)
    };

    // 实体 id → 名称 / 度
    let (id_name, degree) = {
        let conn = store.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name FROM entities")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut m = HashMap::new();
        for row in rows.flatten() {
            let d = adj.get(&row.0).map(|n| n.values().sum::<f64>()).unwrap_or(0.0);
            m.insert(row.0, (row.1, d));
        }
        let deg: HashMap<i64, f64> = m.iter().map(|(k, (_, d))| (*k, *d)).collect();
        (m, deg)
    };

    // 标签传播社区检测（确定性：固定遍历序，平票取小标签）
    let mut labels: HashMap<i64, i64> = id_name.keys().map(|&k| (k, k)).collect();
    let mut ids_sorted: Vec<i64> = id_name.keys().copied().collect();
    ids_sorted.sort();
    for _ in 0..12 {
        let mut changed = false;
        for &id in &ids_sorted {
            let mut votes: HashMap<i64, f64> = HashMap::new();
            if let Some(nb) = adj.get(&id) {
                for (&other, &w) in nb {
                    *votes.entry(labels[&other]).or_insert(0.0) += w;
                }
            }
            let best = votes
                .into_iter()
                .min_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)))
                .map(|(l, _)| l);
            if let Some(best) = best {
                if best != labels[&id] {
                    labels.insert(id, best);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut comm_members: HashMap<i64, Vec<i64>> = HashMap::new();
    for (&id, &l) in &labels {
        comm_members.entry(l).or_default().push(id);
    }
    let mut communities: Vec<Community> = comm_members
        .into_values()
        .filter(|m| m.len() >= 3)
        .map(|members| {
            let mut ms = members.clone();
            ms.sort_by_key(|&id| std::cmp::Reverse(degree.get(&id).copied().unwrap_or(0.0) as i64));
            let top: Vec<String> = ms.iter().take(6).filter_map(|id| id_name.get(id).map(|(n, _)| n.clone())).collect();
            let mut srcs: Vec<String> = Vec::new();
            for n in &top {
                if let Some(ss) = ent_src.get(n) {
                    for s in ss {
                        if !srcs.contains(s) {
                            srcs.push(s.clone());
                        }
                    }
                }
            }
            Community { label: top.first().cloned().unwrap_or_default(), size: members.len(), top, sources: srcs }
        })
        .collect();
    communities.sort_by(|a, b| b.size.cmp(&a.size));
    communities.truncate(12);

    // 实体统计（name → id 反查表避免 O(n²)）
    let name_id: HashMap<&str, i64> = id_name.iter().map(|(id, (n, _))| (n.as_str(), *id)).collect();
    let mut stats: Vec<EntityStat> = ent_mem
        .into_iter()
        .map(|(name, mems)| {
            let d = name_id
                .get(name.as_str())
                .and_then(|id| degree.get(id).copied())
                .unwrap_or(0.0);
            EntityStat {
                degree: d as usize,
                memory_count: mems.len(),
                sources: ent_src.get(&name).cloned().unwrap_or_default(),
                name,
            }
        })
        .collect();
    stats.sort_by(|a, b| b.memory_count.cmp(&a.memory_count).then(b.degree.cmp(&a.degree)));
    let top_entities: Vec<EntityStat> = stats.iter().take(24).cloned().collect();
    let cross_source: Vec<EntityStat> = stats
        .iter()
        .filter(|s| s.sources.len() >= 2)
        .take(24)
        .cloned()
        .collect();

    let rep = IndexReport {
        built_at: now_ts(),
        duration_ms: t0.elapsed().as_millis() as u64,
        memories_indexed: records.len(),
        entities: entity_count,
        edges: edge_count,
        communities,
        top_entities,
        cross_source,
    };
    store.graph_meta_set("index_report", &serde_json::to_string(&rep).unwrap_or_default())?;
    store.graph_meta_set("index_built_at", &rep.built_at.to_string())?;
    store.audit("hub_index_build", &serde_json::json!({"entities": rep.entities, "edges": rep.edges, "communities": rep.communities.len()}))?;
    Ok(rep)
}

fn guess_kind(name: &str) -> &'static str {
    if name.contains('/') || name.contains("\\\\") {
        "path"
    } else if name.contains('.') && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_') {
        "config"
    } else {
        "thing"
    }
}

// ---------------- 跨源矛盾扫描 ----------------

/// 一对矛盾候选
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contradiction {
    pub entity: String,
    pub a_id: i64,
    pub a_source: String,
    pub a_text: String,
    pub b_id: i64,
    pub b_source: String,
    pub b_text: String,
    /// numeric / lexical / llm
    pub kind: String,
    pub reason: String,
    pub severity: f64,
}

/// 跨源矛盾扫描：按实体聚合 ≥2 条记忆的组，成对检测数值矛盾与高相似低重合矛盾。
/// LLM 可选复核（judge 提供时对前 N 对做语义判定）。
pub fn scan_contradictions(store: &MemoryStore, judge: Option<&dyn crate::llm_hook::LlmJudge>, max_pairs: usize) -> Result<Vec<Contradiction>> {
    let records = store.list(None, None, false, 3000)?;
    let mut by_entity: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, r) in records.iter().enumerate() {
        for e in entity_names_of(r) {
            by_entity.entry(e).or_default().push(i);
        }
    }
    let mut out = Vec::new();
    for (ent, idxs) in &by_entity {
        if idxs.len() < 2 || out.len() >= max_pairs * 4 {
            continue;
        }
        for pi in 0..idxs.len() {
            for qi in (pi + 1)..idxs.len() {
                if out.len() >= max_pairs * 4 {
                    break;
                }
                let (a, b) = (&records[idxs[pi]], &records[idxs[qi]]);
                if a.id == b.id || (a.source == b.source && a.content == b.content) {
                    continue;
                }
                let na = numbers_in(&a.content);
                let nb = numbers_in(&b.content);
                let ov = lexical_overlap(&a.content, &b.content);
                // 数值矛盾：同主题（重叠≥0.25）且数字集合不同且各自非空
                if !na.is_empty() && !nb.is_empty() && ov >= 0.2 && na != nb {
                    let diff = na.iter().any(|x| !nb.contains(x)) || nb.iter().any(|x| !na.contains(x));
                    if diff {
                        let cross = a.source != b.source;
                        out.push(Contradiction {
                            entity: ent.clone(),
                            a_id: a.id,
                            a_source: a.source.clone(),
                            a_text: a.content.chars().take(160).collect(),
                            b_id: b.id,
                            b_source: b.source.clone(),
                            b_text: b.content.chars().take(160).collect(),
                            kind: "numeric".into(),
                            reason: format!("同主题数字集合不一致：{:?} vs {:?}{}", short_nums(&na), short_nums(&nb), if cross { "（跨源）" } else { "" }),
                            severity: (ov * if cross { 1.0 } else { 0.7 }).min(1.0),
                        });
                        continue;
                    }
                }
                // 高相似但断言不同：相似度极高而 token 重合低 → 疑似互斥陈述
                if ov >= 0.10 && ov < 0.2 && a.source != b.source {
                    out.push(Contradiction {
                        entity: ent.clone(),
                        a_id: a.id,
                        a_source: a.source.clone(),
                        a_text: a.content.chars().take(160).collect(),
                        b_id: b.id,
                        b_source: b.source.clone(),
                        b_text: b.content.chars().take(160).collect(),
                        kind: "lexical".into(),
                        reason: "跨源同实体相近陈述，表述分歧行待核".into(),
                        severity: ov * 0.6,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| b.severity.partial_cmp(&a.severity).unwrap_or(std::cmp::Ordering::Equal));
    // 同一对记录经多个共享实体会重复出现：按记录对去重，保留最高严重度
    let mut seen_pairs: std::collections::HashSet<(i64, i64)> = std::collections::HashSet::new();
    out.retain(|c| {
        let key = (c.a_id.min(c.b_id), c.a_id.max(c.b_id));
        seen_pairs.insert(key)
    });
    out.truncate(max_pairs);
    // LLM 复核：只对数值候选 top 半数做确认（确定性规则已足够初筛）
    if let Some(j) = judge {
        for c in out.iter_mut().take(10) {
            if c.kind != "numeric" {
                continue;
            }
            if let Some(ans) = j.complete(
                "判断两条记忆是否构成事实矛盾（同一事实的不同取值/互斥陈述）。只输出 JSON：{\"contradicts\": bool}",
                &format!("记忆A（来源{}）：{}\n记忆B（来源{}）：{}", c.a_source, c.a_text, c.b_source, c.b_text),
            ) {
                let t = ans.trim().trim_start_matches("```json").trim_end_matches("```").trim();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
                    if v.get("contradicts").and_then(|x| x.as_bool()) == Some(false) {
                        c.kind = "dismissed".into();
                        c.severity *= 0.2;
                    } else {
                        c.kind = "llm".into();
                        c.severity = (c.severity * 1.3).min(1.0);
                    }
                }
            }
        }
        out.sort_by(|a, b| b.severity.partial_cmp(&a.severity).unwrap_or(std::cmp::Ordering::Equal));
    }
    Ok(out)
}

fn short_nums(v: &[f64]) -> Vec<String> {
    v.iter().take(4).map(|x| if x.fract() == 0.0 { format!("{}", *x as i64) } else { format!("{x}") }).collect()
}

fn numbers_in(text: &str) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
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
    out.retain(|n| *n >= 10.0 || n.fract() != 0.0); // 过滤一位数噪声
    out.truncate(8);
    out
}

fn lexical_overlap(a: &str, b: &str) -> f64 {
    let toks = |s: &str| -> std::collections::HashSet<String> {
        let mut set = std::collections::HashSet::new();
        for w in s.split_whitespace() {
            if w.chars().count() >= 2 {
                set.insert(w.to_lowercase());
            }
        }
        // CJK bigram
        let chars: Vec<char> = s.chars().collect();
        for w in chars.windows(2) {
            if w[0] >= '\u{4e00}' && w[0] <= '\u{9fff}' && w[1] >= '\u{4e00}' && w[1] <= '\u{9fff}' {
                set.insert(w.iter().collect());
            }
        }
        set
    };
    let (ta, tb) = (toks(a), toks(b));
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let inter = ta.intersection(&tb).count();
    inter as f64 / ta.union(&tb).count() as f64
}

// ---------------- 总管巡检（维护计划） ----------------

/// 维护计划
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintReport {
    /// 重复指纹组（组内 >1 条，保留最新，其余可遗忘）
    pub dup_groups: Vec<DupGroup>,
    /// 失活记忆（低访问 + 低置信 + 超期未用）
    pub stale_ids: Vec<i64>,
    /// 墓碑积压（软删未清理）
    pub tombstone_backlog: usize,
    pub source_stats: Vec<SourceStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DupGroup {
    pub fingerprint: String,
    pub ids: Vec<i64>,
    pub keep_id: i64,
    pub sample: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceStat {
    pub source: String,
    pub count: usize,
}

/// 全库巡检：重复 / 失活 / 墓碑积压 / 来源体量
pub fn maintenance_scan(store: &MemoryStore) -> Result<MaintReport> {
    let records = store.list(None, None, false, 5000)?;
    let mut by_fp: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, r) in records.iter().enumerate() {
        by_fp.entry(fingerprint(&format!("{}|{}", r.title, r.content))).or_default().push(i);
    }
    let dup_groups: Vec<DupGroup> = by_fp
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(fp, v)| {
            let mut ids: Vec<i64> = v.iter().map(|&i| records[i].id).collect();
            ids.sort_by_key(|&id| {
                let r = records.iter().find(|r| r.id == id).unwrap();
                std::cmp::Reverse(r.updated_at)
            });
            DupGroup {
                sample: records[v[0]].content.chars().take(60).collect(),
                fingerprint: fp,
                keep_id: ids[0],
                ids,
            }
        })
        .take(30)
        .collect();

    let now = now_ts();
    let stale_ids: Vec<i64> = records
        .iter()
        .filter(|r| {
            r.tier == Tier::Episodic
                && r.access_count <= 1
                && r.confidence < 0.7
                && now - r.updated_at > 30 * 86400
        })
        .map(|r| r.id)
        .take(200)
        .collect();

    let (tombstone_backlog, mut source_stats) = {
        let conn = store.conn.lock().unwrap();
        let tb: usize = conn.query_row("SELECT COUNT(*) FROM memories WHERE tombstone = 1", [], |r| r.get(0))?;
        let mut stmt = conn.prepare("SELECT COALESCE(NULLIF(source,''),'(未知)'), COUNT(*) FROM memories WHERE tombstone = 0 AND is_current = 1 GROUP BY 1 ORDER BY 2 DESC LIMIT 20")?;
        let rows = stmt.query_map([], |r| Ok(SourceStat { source: r.get(0)?, count: r.get(1)? }))?;
        (tb, rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
    };
    source_stats.sort_by(|a, b| b.count.cmp(&a.count));
    Ok(MaintReport { dup_groups, stale_ids, tombstone_backlog, source_stats })
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
    fn test_split_rules() {
        let items = split_rules("# 工作流\n\n- 偏好使用 libreoffice\n\n## 部署\n\n部署脚本在 /opt/apps");
        assert!(items.len() >= 2);
    }

    #[test]
    fn test_parse_session_jsonl_skips_invalid_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        std::fs::write(
            &path,
            "{\"session_id\":\"abcdefgh-1\",\"ts\":1,\"text\":\"以后报告使用中文\"}\nnot-json\n{\"text\":\"部署端口是 8801\"}\n",
        )
        .unwrap();
        let items = parse_session_jsonl(&path).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items[0].title.contains("abcdefgh"));
        assert_eq!(items[1].content, "部署端口是 8801");
    }

    #[test]
    fn test_managed_block_roundtrip() {
        let block = managed_block("lymem 记忆分发", &["- **[偏好]** 语言=中文".into(), "- **[知识]** 部署在 /opt/apps".into()]);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("AGENTS.md");
        std::fs::write(&p, "用户手写内容\n").unwrap();
        write_dispatch(&p, Some(&block), DispatchMode::Replace).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.starts_with("用户手写内容"));
        assert!(body.contains(MARK_BEGIN));
        // 再次 replace 不叠加
        write_dispatch(&p, Some(&block), DispatchMode::Replace).unwrap();
        let body2 = std::fs::read_to_string(&p).unwrap();
        assert_eq!(body2.matches(MARK_BEGIN).count(), 1);
        // remove 后仅剩手写内容
        write_dispatch(&p, None, DispatchMode::Remove).unwrap();
        let body3 = std::fs::read_to_string(&p).unwrap();
        assert!(!body3.contains("lymem"));
        assert!(body3.contains("用户手写内容"));
    }

    #[test]
    fn test_import_and_index_and_conflict() {
        let s = store();
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("AGENTS.md");
        std::fs::write(&src_path, "## 工作偏好\n\n- 使用 libreoffice 导出 pdf\n\n## 部署\n\n- 部署脚本在 `/opt/apps`，端口 8080\n").unwrap();
        let src = probe(SourceKind::RulesMd, "testagent", &src_path, true, "AGENTS.md");
        assert_eq!(src.items, 2);
        let rep = import_source(&s, &src).unwrap();
        assert_eq!(rep.imported, 2);
        // 重导全跳过（增量幂等）
        let rep2 = import_source(&s, &src).unwrap();
        assert_eq!(rep2.imported, 0);
        assert_eq!(rep2.skipped_known, 2);

        // 相同规则来自另一个 Agent 时按库内内容去重，但仍与来源内增量去重分开计数。
        let copied = probe(SourceKind::RulesMd, "copied-agent", &src_path, true, "AGENTS.md");
        let copied_rep = import_source(&s, &copied).unwrap();
        assert_eq!(copied_rep.imported, 0);
        assert_eq!(copied_rep.skipped_dup, 2);

        // 另一 agent 写入矛盾的端口
        let src2_path = dir.path().join("B.md");
        std::fs::write(&src2_path, "## 部署\n\n- 部署脚本在 `/opt/apps`，端口 9090\n").unwrap();
        let src2 = probe(SourceKind::RulesMd, "other-agent", &src2_path, true, "B.md");
        import_source(&s, &src2).unwrap();

        let idx = build_index(&s, 1000).unwrap();
        assert!(idx.entities >= 3, "entities={} top={:?}", idx.entities, idx.top_entities.iter().map(|e|(&e.name,e.memory_count)).collect::<Vec<_>>());
        assert!(idx.memories_indexed >= 3);

        let cons = scan_contradictions(&s, None, 20).unwrap();
        assert!(cons.iter().any(|c| c.kind == "numeric" && c.a_source != c.b_source), "应有跨源数值矛盾：{cons:?}");

        let maint = maintenance_scan(&s).unwrap();
        assert!(!maint.source_stats.is_empty());
    }

    #[test]
    fn test_dispatch_targets_shape() {
        let ts = dispatch_targets();
        // 至少包含 lymem 自身共享文件这一虚拟目标
        assert!(ts.iter().any(|t| t.agent == "lymem"));
    }
}
