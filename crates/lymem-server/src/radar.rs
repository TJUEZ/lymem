//! 文件雷达：授权目录内的文件内容索引，让"按内容找文件"成为可能。
//!
//! 隐私模型（差异化答案，回应无差别屏幕记录的争议）：
//! - 默认不索引任何目录，逐目录 opt-in 授权，随时删除并彻底清除；
//! - 敏感内容沿用系统双档：阻断级（密钥/密码/身份证/银行卡）整文件拒绝入库，
//!   一般敏感（手机号/邮箱/IP）脱敏后入库；
//! - 增量扫描：mtime+size 未变的文件零成本跳过；删除/变更后旧索引退役。
//!
//! 抽取：文本族直读（UTF-8）；PDF 走 pdftotext（存在时）；其余扩展名明示不支持。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use lymem_core::model::{MemoryKind, MemoryRecord, Sensitivity, Tier};
use lymem_core::sensitive;
use lymem_core::MemoryStore;
use rusqlite::params;
use rusqlite::OptionalExtension;
use serde_json::json;

static SCANNING: AtomicBool = AtomicBool::new(false);

const MAX_FILES: usize = 2000;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_CONTENT_CHARS: usize = 60_000;
const MAX_DEPTH: usize = 8;

const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "rst", "csv", "tsv", "json", "yaml", "yml", "log", "html", "htm",
    "xml", "ini", "conf", "toml", "rs", "py", "js", "ts", "jsx", "tsx", "java", "c", "h", "cpp",
    "hpp", "go", "sh", "bash", "sql", "css",
];
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", "build", "__pycache__", ".venv", "venv", ".cache"];

fn dirs_path() -> PathBuf {
    let base = std::env::var("LYMEM_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".local/share/lymem")
        });
    base.join("radar.json")
}

pub fn load_dirs() -> Vec<String> {
    fs::read_to_string(dirs_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

pub fn save_dirs(dirs: &[String]) -> std::io::Result<()> {
    if let Some(parent) = dirs_path().parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(dirs_path(), serde_json::to_string_pretty(dirs).unwrap_or_else(|_| "[]".into()))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn is_indexable(name: &str) -> bool {
    let ext = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    TEXT_EXTS.contains(&ext.as_str()) || ext == "pdf"
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= MAX_FILES {
            return;
        }
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(&p, out, depth + 1);
        } else if is_indexable(&name) && fs::metadata(&p).map(|m| m.len() <= MAX_FILE_BYTES).unwrap_or(false) {
            out.push(p);
        }
    }
}

fn extract(path: &Path, ext: &str) -> Result<String, String> {
    if ext == "pdf" {
        let out = std::process::Command::new("pdftotext")
            .arg("-q")
            .arg(path)
            .arg("-")
            .output()
            .map_err(|_| "pdftotext 不可用（未安装 poppler-utils）".to_string())?;
        if !out.status.success() {
            return Err("PDF 文本抽取失败（可能是扫描件/加密件）".into());
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        fs::read_to_string(path).map_err(|_| "非 UTF-8 文本".to_string())
    }
}

fn record_status(store: &MemoryStore, path: &str, dir: &str, mtime: i64, size: i64, memory_id: i64, status: &str, reason: &str) {
    let conn = store.conn.lock().unwrap();
    let _ = conn.execute(
        "INSERT INTO radar_files(path, dir, mtime, size, memory_id, status, reason, at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(path) DO UPDATE SET dir=?2, mtime=?3, size=?4, memory_id=?5, status=?6, reason=?7, at=?8",
        params![path, dir, mtime, size, memory_id, status, reason, now_secs()],
    );
}

/// 退役某文件此前的索引记忆（内容变更/被拦截后旧块不得残留于检索）
fn retire(store: &MemoryStore, path: &str) {
    let conn = store.conn.lock().unwrap();
    let mem: Option<i64> = conn
        .query_row("SELECT memory_id FROM radar_files WHERE path = ?1", params![path], |r| r.get(0))
        .optional()
        .ok()
        .flatten();
    if let Some(id) = mem.filter(|v| *v > 0) {
        let _ = conn.execute(
            "UPDATE memories SET tombstone = 1, tombstoned_at = ?2, is_current = 0 WHERE id = ?1",
            params![id, now_secs()],
        );
    }
}

pub fn scan(store: &MemoryStore) -> serde_json::Value {
    if SCANNING.swap(true, Ordering::SeqCst) {
        return json!({"ok": false, "error": "已有扫描在进行中，请稍候"});
    }
    let t0 = Instant::now();
    let mut v = scan_inner(store);
    SCANNING.store(false, Ordering::SeqCst);
    v["elapsed_ms"] = json!(t0.elapsed().as_millis() as u64);
    v
}

fn scan_inner(store: &MemoryStore) -> serde_json::Value {
    let dirs = load_dirs();
    if dirs.is_empty() {
        return json!({"ok": false, "error": "尚未授权任何目录，请先添加"});
    }
    let mut files = Vec::new();
    for d in &dirs {
        walk(Path::new(d), &mut files, 0);
    }
    let (mut indexed, mut updated, mut skipped, mut blocked, mut unsupported, mut errors) = (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    for path in &files {
        let path_str = path.to_string_lossy().to_string();
        let dir = dirs.iter().find(|d| path_str.starts_with(d.as_str())).cloned().unwrap_or_default();
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let size = meta.len() as i64;

        // 增量：mtime+size 未变零成本跳过（含已拦截/不支持文件，状态未变无需重评）
        let prior: Option<(i64, i64)> = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT mtime, size FROM radar_files WHERE path = ?1 AND status IN ('indexed','blocked','unsupported','empty')",
                params![path_str],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten()
        };
        if let Some((om, os)) = prior {
            if om == mtime && os == size {
                skipped += 1;
                continue;
            }
        }
        let existed = prior.is_some();

        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let content = match extract(path, &ext) {
            Ok(c) => c,
            Err(reason) => {
                retire(store, &path_str);
                record_status(store, &path_str, &dir, mtime, size, 0, "unsupported", &reason);
                unsupported += 1;
                continue;
            }
        };
        let trimmed: String = content.chars().take(MAX_CONTENT_CHARS).collect();
        if trimmed.trim().is_empty() {
            retire(store, &path_str);
            record_status(store, &path_str, &dir, mtime, size, 0, "empty", "无可抽取文本");
            skipped += 1;
            continue;
        }

        // 敏感门（沿用系统双档）：阻断级整文件拒绝；一般敏感脱敏后入库
        let findings = sensitive::scan(&trimmed);
        let content = match sensitive::level_of(&findings) {
            Sensitivity::Blocked => {
                let kinds: std::collections::BTreeSet<String> = findings.iter().map(|f| f.kind.clone()).collect();
                retire(store, &path_str);
                record_status(store, &path_str, &dir, mtime, size, 0, "blocked", &format!("命中阻断级敏感信息:{}", kinds.into_iter().collect::<Vec<_>>().join("/")));
                blocked += 1;
                continue;
            }
            Sensitivity::Sensitive => sensitive::redact(&trimmed),
            Sensitivity::None => trimmed,
        };

        // 变更文件：旧索引先退役
        retire(store, &path_str);
        let rel = path
            .strip_prefix(&dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path_str.clone());
        let mut rec = MemoryRecord::new(Tier::Knowledge, MemoryKind::Fact, rel, content);
        rec.scene = "file".into();
        rec.source = "radar".into();
        rec.meta = json!({"path": path_str, "dir": dir, "mtime": mtime, "size": size});
        match store.put(rec) {
            Ok(id) => {
                record_status(store, &path_str, &dir, mtime, size, id, "indexed", "");
                if existed {
                    updated += 1;
                } else {
                    indexed += 1;
                }
            }
            Err(e) => {
                record_status(store, &path_str, &dir, mtime, size, 0, "error", &e.to_string());
                errors += 1;
            }
        }
    }
    json!({
        "ok": true, "files": files.len(), "indexed": indexed, "updated": updated,
        "skipped": skipped, "blocked": blocked, "unsupported": unsupported, "errors": errors,
    })
}

/// 删除授权目录并彻底清除该目录的索引数据（可清除是隐私承诺的一半）
pub fn purge_dir(store: &MemoryStore, dir: &str) -> usize {
    let n = {
        let conn = store.conn.lock().unwrap();
        let ids: Vec<i64> = match conn.prepare("SELECT memory_id FROM radar_files WHERE dir = ?1 AND memory_id > 0") {
            Ok(mut stmt) => stmt
                .query_map(params![dir], |r| r.get(0))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        for id in ids {
            let _ = conn.execute(
                "UPDATE memories SET tombstone = 1, tombstoned_at = ?2, is_current = 0 WHERE id = ?1",
                params![id, now_secs()],
            );
        }
        conn.execute("DELETE FROM radar_files WHERE dir = ?1", params![dir]).unwrap_or(0)
    };
    let _ = store.audit("radar_purge_dir", &json!({"dir": dir, "removed": n}));
    n
}

/// 状态总览（雷达页）
pub fn status(store: &MemoryStore) -> serde_json::Value {
    let conn = store.conn.lock().unwrap();
    let mut by = std::collections::BTreeMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT status, COUNT(*) FROM radar_files GROUP BY status") {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
            for row in rows.filter_map(|r| r.ok()) {
                by.insert(row.0, row.1);
            }
        }
    }
    json!({
        "scanning": SCANNING.load(Ordering::SeqCst),
        "dirs": load_dirs(),
        "by_status": by,
        "total": by.values().sum::<i64>(),
    })
}
