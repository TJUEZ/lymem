//! 存储层：SQLite（元数据/版本链/图边）+ sqlite-vec（向量 ANN）+ tantivy（BM25）。
//!
//! 设计原则：
//! - 单文件数据库 + 全文目录，拷贝即备份，满足端侧轻量化要求；
//! - 向量与 chunk 关联存储，支持片段级检索；
//! - 所有删除可审计（audit_log），遗忘分墓碑/硬清除两档。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::embedding::{vec_to_blob, Embedder};
use crate::error::{CoreError, Result};
use crate::fts::FtsIndex;
use crate::model::{MemoryKind, MemoryRecord, Sensitivity, Tier};

/// 建库 SQL
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS memories(
  id INTEGER PRIMARY KEY,
  tier TEXT NOT NULL,
  kind TEXT NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  content TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT '',
  scene TEXT NOT NULL DEFAULT 'general',
  confidence REAL NOT NULL DEFAULT 0.5,
  sensitivity TEXT NOT NULL DEFAULT 'none',
  meta TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  access_count INTEGER NOT NULL DEFAULT 0,
  last_access_at INTEGER,
  tombstone INTEGER NOT NULL DEFAULT 0,
  tombstoned_at INTEGER,
  version_of INTEGER,
  is_current INTEGER NOT NULL DEFAULT 1,
  entity_keys TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS idx_mem_tier ON memories(tier);
CREATE INDEX IF NOT EXISTS idx_mem_scene ON memories(scene);
CREATE INDEX IF NOT EXISTS idx_mem_cur ON memories(is_current, tombstone);
CREATE INDEX IF NOT EXISTS idx_mem_ver ON memories(version_of);

CREATE TABLE IF NOT EXISTS chunks(
  chunk_id INTEGER PRIMARY KEY,
  memory_id INTEGER NOT NULL,
  chunk_idx INTEGER NOT NULL,
  content TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chunks_mem ON chunks(memory_id);

CREATE TABLE IF NOT EXISTS entities(
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL DEFAULT 'thing'
);
CREATE TABLE IF NOT EXISTS edges(
  id INTEGER PRIMARY KEY,
  src INTEGER NOT NULL,
  dst INTEGER NOT NULL,
  relation TEXT NOT NULL,
  memory_id INTEGER,
  weight REAL NOT NULL DEFAULT 1.0,
  UNIQUE(src, dst, relation, memory_id)
);
CREATE INDEX IF NOT EXISTS idx_edges_src ON edges(src);
CREATE INDEX IF NOT EXISTS idx_edges_dst ON edges(dst);

CREATE TABLE IF NOT EXISTS preferences(
  id INTEGER PRIMARY KEY,
  key TEXT NOT NULL UNIQUE,
  current_version INTEGER NOT NULL DEFAULT 0,
  description TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS pref_versions(
  id INTEGER PRIMARY KEY,
  pref_id INTEGER NOT NULL,
  version INTEGER NOT NULL,
  value TEXT NOT NULL,
  evidence TEXT NOT NULL DEFAULT '[]',
  source TEXT NOT NULL DEFAULT 'rule',
  confidence REAL NOT NULL DEFAULT 0.5,
  scenes TEXT NOT NULL DEFAULT '["*"]',
  valid_from INTEGER NOT NULL,
  valid_to INTEGER,
  superseded_by INTEGER,
  UNIQUE(pref_id, version)
);

CREATE TABLE IF NOT EXISTS conflicts(
  id INTEGER PRIMARY KEY,
  old_id INTEGER,
  new_id INTEGER,
  ctype TEXT NOT NULL,
  resolution TEXT NOT NULL,
  reason TEXT NOT NULL DEFAULT '',
  decided_by TEXT NOT NULL DEFAULT 'rule',
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_log(
  id INTEGER PRIMARY KEY,
  at INTEGER NOT NULL,
  action TEXT NOT NULL,
  detail TEXT NOT NULL DEFAULT '{}'
);
"#;

/// 记忆库：组合 SQLite 连接、全文索引与嵌入器。
/// 连接与索引写入均在 Mutex 内串行（端侧单用户场景足够）。
pub struct MemoryStore {
    pub conn: Mutex<Connection>,
    pub fts: Mutex<FtsIndex>,
    pub embedder: Box<dyn Embedder>,
    pub vec_dim: usize,
    fts_dir: PathBuf,
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<MemoryRecord> {
    let tier_s: String = row.get("tier")?;
    let kind_s: String = row.get("kind")?;
    let sens_s: String = row.get("sensitivity")?;
    let entity_s: String = row.get("entity_keys")?;
    let meta_s: String = row.get("meta")?;
    Ok(MemoryRecord {
        id: row.get("id")?,
        tier: Tier::parse(&tier_s).unwrap_or(Tier::Knowledge),
        kind: MemoryKind::parse(&kind_s).unwrap_or(MemoryKind::Fact),
        title: row.get("title")?,
        content: row.get("content")?,
        source: row.get("source")?,
        scene: row.get("scene")?,
        confidence: row.get("confidence")?,
        sensitivity: Sensitivity::parse(&sens_s),
        meta: serde_json::from_str(&meta_s).unwrap_or(serde_json::json!({})),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        access_count: row.get("access_count")?,
        last_access_at: row.get("last_access_at")?,
        tombstone: row.get::<_, i64>("tombstone")? != 0,
        tombstoned_at: row.get("tombstoned_at")?,
        version_of: row.get("version_of")?,
        is_current: row.get::<_, i64>("is_current")? != 0,
        entities: serde_json::from_str(&entity_s).unwrap_or_default(),
    })
}

const RECORD_COLS: &str = "id, tier, kind, title, content, source, scene, confidence, sensitivity, meta, created_at, updated_at, access_count, last_access_at, tombstone, tombstoned_at, version_of, is_current, entity_keys";

/// 文本分块：按段落聚合，单块不超过 max_chars（默认 480，适配嵌入模型窗口）
pub fn chunk_text(content: &str, max_chars: usize) -> Vec<String> {
    let paras: Vec<&str> = content.split("\n\n").map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for p in paras {
        if p.chars().count() > max_chars {
            // 超长段落硬切
            if !cur.is_empty() {
                chunks.push(std::mem::take(&mut cur));
            }
            let chars: Vec<char> = p.chars().collect();
            for seg in chars.chunks(max_chars) {
                chunks.push(seg.iter().collect());
            }
            continue;
        }
        let add = if cur.is_empty() { p.len() } else { cur.len() + 2 + p.len() };
        if add > max_chars {
            chunks.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(p);
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    if chunks.is_empty() {
        chunks.push(content.trim().to_string());
    }
    chunks
}

impl MemoryStore {
    /// 打开/创建记忆库。db_path 为 SQLite 文件，fts_dir 为全文索引目录。
    pub fn open(db_path: &Path, fts_dir: &Path, embedder: Box<dyn Embedder>) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // sqlite-vec 扩展必须在打开连接前注册
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;

        let vec_dim = embedder.dim();
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(chunk_id INTEGER PRIMARY KEY, embedding float[{vec_dim}]);"
        ))?;

        let fts = FtsIndex::open(fts_dir)?;
        Ok(MemoryStore {
            conn: Mutex::new(conn),
            fts: Mutex::new(fts),
            embedder,
            vec_dim,
            fts_dir: fts_dir.to_path_buf(),
        })
    }

    pub fn fts_dir(&self) -> &Path {
        &self.fts_dir
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    /// 写入一条记忆：分块 → 嵌入 → 落库（SQLite + 向量 + 全文）。
    /// 返回分配的记忆 id。blocked 级敏感内容拒绝入库（仅审计）。
    pub fn put(&self, mut rec: MemoryRecord) -> Result<i64> {
        if rec.content.trim().is_empty() {
            return Err(CoreError::InvalidInput("content 为空".into()));
        }
        if rec.sensitivity == Sensitivity::Blocked {
            self.audit("ingest_blocked", &serde_json::json!({"title": rec.title, "kind": rec.kind.as_str()}))?;
            return Err(CoreError::InvalidInput("内容包含阻断级敏感信息，已拒绝入库".into()));
        }
        let chunks = chunk_text(&rec.content, 480);
        let chunk_refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
        let vectors = self.embedder.embed(&chunk_refs)?;

        let conn = self.conn.lock().unwrap();
        let now = Self::now();
        rec.created_at = now;
        rec.updated_at = now;
        conn.execute(
            "INSERT INTO memories(tier, kind, title, content, source, scene, confidence, sensitivity, meta, created_at, updated_at, entity_keys)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11)",
            params![
                rec.tier.as_str(),
                rec.kind.as_str(),
                rec.title,
                rec.content,
                rec.source,
                rec.scene,
                rec.confidence,
                rec.sensitivity.as_str(),
                rec.meta.to_string(),
                now,
                serde_json::to_string(&rec.entities).unwrap_or_else(|_| "[]".into()),
            ],
        )?;
        let mem_id = conn.last_insert_rowid();

        let mut fts = self.fts.lock().unwrap();
        for (idx, (chunk, vec)) in chunks.into_iter().zip(vectors.into_iter()).enumerate() {
            conn.execute(
                "INSERT INTO chunks(memory_id, chunk_idx, content) VALUES (?1, ?2, ?3)",
                params![mem_id, idx as i64, chunk],
            )?;
            let chunk_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO vec_chunks(chunk_id, embedding) VALUES (?1, ?2)",
                params![chunk_id, vec_to_blob(&vec)],
            )?;
            let _ = fts.add_chunk(mem_id, chunk_id, rec.tier.as_str(), &rec.scene, &chunk);
        }
        drop(fts);
        drop(conn);
        self.commit()?;
        self.audit("ingest", &serde_json::json!({"id": mem_id, "tier": rec.tier.as_str(), "kind": rec.kind.as_str()}))?;
        Ok(mem_id)
    }

    /// 提交全文索引与数据库事务性（先 commit fts，再由调用方决定）
    pub fn commit(&self) -> Result<()> {
        let mut fts = self.fts.lock().unwrap();
        fts.commit()?;
        Ok(())
    }

    pub fn get(&self, id: i64) -> Result<Option<MemoryRecord>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("SELECT {RECORD_COLS} FROM memories WHERE id = ?1");
        Ok(conn.query_row(&sql, params![id], row_to_record).optional()?)
    }

    /// 通用列表查询（默认排除墓碑与历史版本）
    pub fn list(&self, tier: Option<Tier>, scene: Option<&str>, include_history: bool, limit: usize) -> Result<Vec<MemoryRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = format!("SELECT {RECORD_COLS} FROM memories WHERE 1=1");
        if let Some(t) = tier {
            sql.push_str(&format!(" AND tier = '{}'", t.as_str()));
        }
        if let Some(s) = scene {
            sql.push_str(&format!(" AND scene = '{}'", s.replace('\'', "''")));
        }
        if !include_history {
            sql.push_str(" AND tombstone = 0 AND is_current = 1");
        }
        sql.push_str(&format!(" ORDER BY id DESC LIMIT {}", limit as i64));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_record)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 向量通道：KNN 检索，返回 (chunk_id, distance, memory_id)
    pub fn knn(&self, query_vec: &[f32], k: usize) -> Result<Vec<(i64, f64, i64)>> {
        if query_vec.len() != self.vec_dim {
            return Err(CoreError::Embed(format!("向量维度不匹配: {} != {}", query_vec.len(), self.vec_dim)));
        }
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT v.chunk_id, v.distance, c.memory_id, m.tombstone, m.is_current
             FROM (SELECT chunk_id, distance FROM vec_chunks WHERE embedding MATCH ?1 AND k = ?2) v
             JOIN chunks c ON c.chunk_id = v.chunk_id
             JOIN memories m ON m.id = c.memory_id
             WHERE m.tombstone = 0 AND m.is_current = 1
             ORDER BY v.distance ASC",
        )?;
        let blob = vec_to_blob(query_vec);
        let rows = stmt.query_map(params![blob, k as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?, row.get::<_, i64>(2)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            match r {
                Ok(v) => out.push(v),
                Err(e) => tracing::warn!("knn 行解析失败: {e}"),
            }
        }
        Ok(out)
    }

    /// 取某记忆全部 chunk 的向量（冲突检测用：chunk 级最大余弦）
    pub fn chunk_vectors(&self, memory_id: i64) -> Result<Vec<Vec<f32>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT v.embedding FROM vec_chunks v JOIN chunks c ON c.chunk_id = v.chunk_id WHERE c.memory_id = ?1",
        )?;
        let rows = stmt.query_map(params![memory_id], |row| {
            let blob: Vec<u8> = row.get(0)?;
            Ok(crate::embedding::blob_to_vec(&blob))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 读取一个 chunk 的内容（检索片段用）
    pub fn chunk_content(&self, chunk_id: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT content FROM chunks WHERE chunk_id = ?1", params![chunk_id], |r| r.get(0)).optional()?)
    }

    /// 更新访问计数（检索命中后调用）
    pub fn touch(&self, ids: &[i64]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Self::now();
        for id in ids {
            let _ = conn.execute(
                "UPDATE memories SET access_count = access_count + 1, last_access_at = ?2 WHERE id = ?1",
                params![id, now],
            );
        }
        Ok(())
    }

    /// 墓碑软删（可审计、可恢复）
    pub fn tombstone(&self, ids: &[i64]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = Self::now();
        let n = conn.execute(
            "UPDATE memories SET tombstone = 1, tombstoned_at = ?1 WHERE id IN (SELECT json_each.value FROM json_each(?2)) AND tombstone = 0",
            params![now, serde_json::to_string(ids).unwrap()],
        )?;
        drop(conn);
        self.audit("forget_tombstone", &serde_json::json!({"ids": ids}))?;
        Ok(n)
    }

    /// 恢复墓碑
    pub fn restore(&self, ids: &[i64]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE memories SET tombstone = 0, tombstoned_at = NULL WHERE id IN (SELECT json_each.value FROM json_each(?1))",
            params![serde_json::to_string(ids).unwrap()],
        )?;
        drop(conn);
        self.audit("forget_restore", &serde_json::json!({"ids": ids}))?;
        Ok(n)
    }

    /// 硬清除：从所有存储（关系/向量/全文/图边）物理删除
    pub fn purge(&self, ids: &[i64]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let ids_json = serde_json::to_string(ids).unwrap();
        let chunk_ids: Vec<i64> = {
            let mut stmt = conn.prepare(
                "SELECT chunk_id FROM chunks WHERE memory_id IN (SELECT json_each.value FROM json_each(?1))",
            )?;
            let rows = stmt.query_map(params![ids_json], |r| r.get::<_, i64>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        let n = conn.execute(
            "DELETE FROM memories WHERE id IN (SELECT json_each.value FROM json_each(?1))",
            params![ids_json],
        )?;
        conn.execute(
            "DELETE FROM chunks WHERE memory_id IN (SELECT json_each.value FROM json_each(?1))",
            params![ids_json],
        )?;
        for cid in &chunk_ids {
            conn.execute("DELETE FROM vec_chunks WHERE chunk_id = ?1", params![cid])?;
        }
        conn.execute(
            "DELETE FROM edges WHERE memory_id IN (SELECT json_each.value FROM json_each(?1))",
            params![ids_json],
        )?;
        drop(conn);
        let mut fts = self.fts.lock().unwrap();
        for id in ids {
            let _ = fts.delete_memory(*id);
        }
        drop(fts);
        self.commit()?;
        self.audit("forget_purge", &serde_json::json!({"ids": ids}))?;
        Ok(n)
    }

    /// 审计日志
    pub fn audit(&self, action: &str, detail: &serde_json::Value) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO audit_log(at, action, detail) VALUES (?1, ?2, ?3)",
            params![Self::now(), action, detail.to_string()],
        )?;
        Ok(())
    }

    pub fn audit_recent(&self, limit: usize) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT at, action, detail FROM audit_log ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let at: i64 = row.get(0)?;
            let action: String = row.get(1)?;
            let detail: String = row.get(2)?;
            Ok(serde_json::json!({"at": at, "action": action, "detail": serde_json::from_str::<serde_json::Value>(&detail).unwrap_or_default()}))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 记录冲突
    pub fn record_conflict(&self, old_id: i64, new_id: i64, ctype: &str, resolution: &str, reason: &str, decided_by: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO conflicts(old_id, new_id, ctype, resolution, reason, decided_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![old_id, new_id, ctype, resolution, reason, decided_by, Self::now()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn conflicts_all(&self) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, old_id, new_id, ctype, resolution, reason, decided_by, created_at FROM conflicts ORDER BY id DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, i64>(0)?, "old_id": row.get::<_, i64>(1)?, "new_id": row.get::<_, i64>(2)?,
                "type": row.get::<_, String>(3)?, "resolution": row.get::<_, String>(4)?,
                "reason": row.get::<_, String>(5)?, "decided_by": row.get::<_, String>(6)?, "at": row.get::<_, i64>(7)?,
            }))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 标记旧版本被取代（保留历史，检索默认排除）
    pub fn mark_superseded(&self, old_id: i64, new_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE memories SET is_current = 0 WHERE id = ?1", params![old_id])?;
        conn.execute("UPDATE memories SET version_of = ?2 WHERE id = ?1", params![new_id, old_id])?;
        Ok(())
    }

    // ---------- 实体图 ----------

    /// 登记实体（存在则返回既有 id）
    pub fn upsert_entity(&self, name: &str, kind: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO entities(name, kind) VALUES (?1, ?2) ON CONFLICT(name) DO NOTHING", params![name, kind])?;
        let id: i64 = conn.query_row("SELECT id FROM entities WHERE name = ?1", params![name], |r| r.get(0))?;
        Ok(id)
    }

    /// 登记边（权重累加）
    pub fn upsert_edge(&self, src: i64, dst: i64, relation: &str, memory_id: Option<i64>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO edges(src, dst, relation, memory_id, weight) VALUES (?1, ?2, ?3, ?4, 1.0)
             ON CONFLICT(src, dst, relation, memory_id) DO UPDATE SET weight = weight + 1.0",
            params![src, dst, relation, memory_id],
        )?;
        Ok(())
    }

    /// 名称模糊匹配实体（图扩展入口）
    pub fn find_entities_like(&self, pattern: &str, limit: usize) -> Result<Vec<(i64, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name FROM entities WHERE name LIKE ?1 LIMIT ?2")?;
        let rows = stmt.query_map(params![format!("%{}%", pattern), limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// 图扩展：从实体出发，一/二跳邻居实体关联的记忆 id 及跳数
    pub fn graph_expand(&self, entity_ids: &[i64], max_hop: usize) -> Result<HashMap<i64, usize>> {
        let conn = self.conn.lock().unwrap();
        let mut result: HashMap<i64, usize> = HashMap::new();
        let mut frontier: Vec<i64> = entity_ids.to_vec();
        let mut visited: Vec<i64> = Vec::new();
        for hop in 1..=max_hop {
            if frontier.is_empty() {
                break;
            }
            let mut next: Vec<i64> = Vec::new();
            let list = frontier.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
            // 邻居实体
            let mut stmt = conn.prepare(&format!(
                "SELECT DISTINCT CASE WHEN src IN ({list}) THEN dst ELSE src END AS other FROM edges WHERE src IN ({list}) OR dst IN ({list})"
            ))?;
            let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
            for other in rows.filter_map(|r| r.ok()) {
                if !visited.contains(&other) && !frontier.contains(&other) {
                    next.push(other);
                }
            }
            // 本跳实体关联的记忆
            let mut stmt2 = conn.prepare(&format!(
                "SELECT DISTINCT memory_id FROM edges WHERE (src IN ({list}) OR dst IN ({list})) AND memory_id IS NOT NULL"
            ))?;
            let rows2 = stmt2.query_map([], |r| r.get::<_, i64>(0))?;
            for mid in rows2.filter_map(|r| r.ok()) {
                result.entry(mid).and_modify(|h| *h = (*h).min(hop)).or_insert(hop);
            }
            // 记忆自身声明的实体（entity_keys 精确匹配）
            let mut stmt3 = conn.prepare(&format!(
                "SELECT DISTINCT id FROM memories WHERE entity_keys LIKE ?1 AND tombstone = 0 AND is_current = 1 LIMIT 200"
            ))?;
            for eid in &frontier {
                let name: String = conn.query_row("SELECT name FROM entities WHERE id = ?1", params![eid], |r| r.get(0))?;
                let rows3 = stmt3.query_map(params![format!("%\"{}\"%", name.replace('%', "").replace('_', ""))], |r| r.get::<_, i64>(0))?;
                for mid in rows3.filter_map(|r| r.ok()) {
                    result.entry(mid).and_modify(|h| *h = (*h).min(hop)).or_insert(hop);
                }
            }
            visited.extend(frontier.iter().copied());
            frontier = next;
        }
        Ok(result)
    }

    /// 按实体名数组直接查关联记忆（供冲突检测）
    pub fn memories_with_entity(&self, name: &str, tier: Tier) -> Result<Vec<i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id FROM memories WHERE entity_keys LIKE ?1 AND tier = ?2 AND tombstone = 0 AND is_current = 1",
        )?;
        let rows = stmt.query_map(params![format!("%\"{}\"%", name), tier.as_str()], |r| r.get::<_, i64>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashEmbedder;

    fn store() -> (MemoryStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let s = MemoryStore::open(&dir.path().join("m.db"), &dir.path().join("fts"), Box::new(HashEmbedder::new(64))).unwrap();
        (s, dir)
    }

    #[test]
    fn test_put_get_knn() {
        let (s, _d) = store();
        let mut r = MemoryRecord::new(Tier::Knowledge, MemoryKind::Fact, "麒麟", "银河麒麟是国产操作系统，采用 Rust 开发记忆模块");
        r.entities = vec!["银河麒麟".into()];
        let id = s.put(r).unwrap();
        assert!(id > 0);
        let got = s.get(id).unwrap().unwrap();
        assert_eq!(got.title, "麒麟");
        let hits = s.knn(&s.embedder.embed_one("银河麒麟").unwrap(), 5).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].2, id);
    }

    #[test]
    fn test_blocked_sensitivity() {
        let (s, _d) = store();
        let mut r = MemoryRecord::new(Tier::Knowledge, MemoryKind::Fact, "密钥", "api key 是 sk-abcdefghijklmnopqrst 请保存");
        r.sensitivity = Sensitivity::Blocked;
        assert!(s.put(r).is_err());
    }

    #[test]
    fn test_tombstone_purge() {
        let (s, _d) = store();
        let id = s.put(MemoryRecord::new(Tier::Episodic, MemoryKind::Conversation, "t", "普通内容一段")).unwrap();
        assert_eq!(s.tombstone(&[id]).unwrap(), 1);
        assert!(s.get(id).unwrap().unwrap().tombstone);
        assert!(s.knn(&s.embedder.embed_one("普通内容").unwrap(), 5).unwrap().is_empty());
        assert_eq!(s.purge(&[id]).unwrap(), 1);
        assert!(s.get(id).unwrap().is_none());
    }

    #[test]
    fn test_chunk_text() {
        let chunks = chunk_text(&format!("第一段{}\n\n第二段", "长".repeat(600)), 480);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            assert!(c.chars().count() <= 480);
        }
    }

    #[test]
    fn test_graph() {
        let (s, _d) = store();
        let a = s.upsert_entity("银河麒麟", "os").unwrap();
        let b = s.upsert_entity("Rust", "lang").unwrap();
        s.upsert_edge(a, b, "developed_with", Some(1)).unwrap();
        let hits = s.graph_expand(&[a], 2).unwrap();
        assert!(hits.contains_key(&1));
    }
}
