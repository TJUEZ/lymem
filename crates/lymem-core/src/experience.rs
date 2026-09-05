//! 经验复用层：从轨迹（情景层）编译可复用经验卡，带采纳反馈门控。
//!
//! 借鉴 WikiSkill（arXiv 2608.27454）的三层分离思想——原始执行经验 / 持久知识页 / 可执行技能：
//! - 原始轨迹 = 情景层（本模块只读，不改写入路径）；
//! - 持久经验页 = **经验卡**：Knowledge 记忆 + `meta.experience` 结构化字段，
//!   入库走 [`MemoryStore::put_knowledge_with_conflicts`] 冲突管道，经验随新旧碰撞自动演化；
//! - 技能演化 = 门控状态机：`draft →（复用≥2 且无失败）→ verified`，任何失败反馈回退 draft，
//!   永不物理删除（对齐"被拒提议也留知识"）。
//!
//! 两类经验卡：
//! - **做法**（practice，kind=workflow）：成功轨迹提炼的可复用流程；
//! - **避坑**（pitfall，kind=case）：失败轨迹提炼的规避清单。
//!
//! 无 LLM 时走规则兜底（统计式提炼），保证端侧离线可用。

use crate::error::Result;
use crate::llm_hook::LlmJudge;
use crate::model::{MemoryKind, MemoryRecord, Tier};
use crate::store::MemoryStore;

use serde::{Deserialize, Serialize};

/// meta.experience 的 schema 版本
pub const EXPERIENCE_VERSION: u8 = 1;
/// verified 门槛：无失败复用次数
pub const VERIFY_WINS: u32 = 2;
/// 源轨迹防重编标记（meta 键）
pub const SRC_MARK: &str = "experience_src";

/// 经验卡结构化字段（存于 memories.meta.experience）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ExperienceMeta {
    pub v: u8,
    /// practice = 做法；pitfall = 避坑
    #[serde(rename = "type")]
    pub exp_type: String,
    /// 归属任务（分组键的一部分）
    pub task: String,
    /// 适用条件：何时该想起这条经验
    pub trigger: String,
    #[serde(default)]
    pub steps: Vec<String>,
    pub outcome: Outcome,
    /// 证据链：来源轨迹记忆 id
    pub source_ids: Vec<i64>,
    /// draft | verified
    pub status: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Outcome {
    #[serde(default)]
    pub wins: u32,
    #[serde(default)]
    pub fails: u32,
}

impl Outcome {
    pub fn success_rate(&self) -> Option<f64> {
        let n = self.wins + self.fails;
        (n > 0).then_some(self.wins as f64 / n as f64)
    }
}

/// 经验卡对外视图（API/界面消费）
#[derive(Serialize, Clone, Debug)]
pub struct ExperienceCard {
    pub id: i64,
    pub title: String,
    pub scene: String,
    pub content: String,
    pub exp_type: String,
    pub task: String,
    pub trigger: String,
    pub steps: Vec<String>,
    pub wins: u32,
    pub fails: u32,
    pub success_rate: Option<f64>,
    pub status: String,
    pub source_ids: Vec<i64>,
    pub created_at: i64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CompileOpts {
    /// 单卡至少要几条同侧轨迹支撑
    #[serde(default = "default_min_support")]
    pub min_support: usize,
    /// 交给 LLM 的单条轨迹正文截断长度
    #[serde(default = "default_clip")]
    pub clip: usize,
}

fn default_min_support() -> usize {
    1
}
fn default_clip() -> usize {
    300
}

impl Default for CompileOpts {
    fn default() -> Self {
        Self { min_support: default_min_support(), clip: default_clip() }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct CompileReport {
    pub ok: bool,
    pub compiled: usize,
    pub skipped_existing: usize,
    pub practice: usize,
    pub pitfall: usize,
    pub details: Vec<String>,
}

/// 一组同任务轨迹（按 scene+task 聚合后的两侧）
struct TrajGroup {
    scene: String,
    task: String,
    wins: Vec<MemoryRecord>,
    fails: Vec<MemoryRecord>,
}

impl MemoryStore {
    /// 从情景层轨迹编译经验卡。幂等：同 (scene, task, type) 已有卡则跳过；
    /// 已编译过的源轨迹打 `meta.experience_src` 标记，不再重复消费。
    pub fn compile_experiences(&self, judge: Option<&dyn LlmJudge>, opts: &CompileOpts) -> Result<CompileReport> {
        let episodic = self.list(Some(Tier::Episodic), None, false, 5000)?;
        let mut groups: Vec<TrajGroup> = Vec::new();
        for rec in episodic {
            if is_marked(&rec.meta, SRC_MARK) {
                continue;
            }
            let (tool, task, ok) = traj_fields(&rec.meta);
            let task = match task {
                Some(t) if !t.is_empty() => t,
                _ => continue, // v1 只从带任务标签的工具轨迹提炼
            };
            let g = match groups.iter_mut().find(|g| g.scene == rec.scene && g.task == task) {
                Some(g) => g,
                None => {
                    groups.push(TrajGroup { scene: rec.scene.clone(), task: task.clone(), wins: vec![], fails: vec![] });
                    groups.last_mut().unwrap()
                }
            };
            match ok {
                Some(true) => g.wins.push(rec),
                Some(false) => g.fails.push(rec),
                None => {}
            }
            let _ = tool; // tool 字段在提炼时按条读取
        }

        let mut rep = CompileReport {
            ok: true, compiled: 0, skipped_existing: 0, practice: 0, pitfall: 0, details: vec![],
        };
        for g in &groups {
            // 做法：成功侧达标
            if g.wins.len() >= opts.min_support {
                match self.emit_card(g, "practice", &g.wins, judge, opts)? {
                    Some(id) => {
                        rep.compiled += 1; rep.practice += 1;
                        rep.details.push(format!("做法:{} → #{}", g.task, id));
                        self.mark_sources(&g.wins)?;
                    }
                    None => rep.skipped_existing += 1,
                }
            }
            // 避坑：失败侧达标
            if g.fails.len() >= opts.min_support {
                match self.emit_card(g, "pitfall", &g.fails, judge, opts)? {
                    Some(id) => {
                        rep.compiled += 1; rep.pitfall += 1;
                        rep.details.push(format!("避坑:{} → #{}", g.task, id));
                        self.mark_sources(&g.fails)?;
                    }
                    None => rep.skipped_existing += 1,
                }
            }
        }
        self.audit("experience_compile", &serde_json::json!({
            "compiled": rep.compiled, "skipped": rep.skipped_existing,
            "practice": rep.practice, "pitfall": rep.pitfall,
        }))?;
        Ok(rep)
    }

    /// 列出全部经验卡（知识层 + meta.experience 过滤）
    pub fn list_experiences(&self) -> Result<Vec<ExperienceCard>> {
        let recs = self.list(Some(Tier::Knowledge), None, false, 2000)?;
        Ok(recs.iter().filter_map(card_of).collect())
    }

    /// 采纳反馈：`adopted_ok` = Some(true) 用过成功 / Some(false) 用过失败。
    /// 门控：wins≥2 且 fails==0 → verified；出现任何失败 → 回退 draft。
    pub fn experience_feedback(&self, id: i64, adopted_ok: bool) -> Result<Option<ExperienceCard>> {
        let rec = match self.get(id)? {
            Some(r) => r,
            None => return Ok(None),
        };
        let mut meta = match parse_exp(&rec.meta) {
            Some(m) => m,
            None => return Ok(None),
        };
        if adopted_ok { meta.outcome.wins += 1 } else { meta.outcome.fails += 1 };
        meta.status = gate(&meta.outcome);
        self.update_experience_meta(id, &rec.meta, &meta)?;
        self.audit("experience_feedback", &serde_json::json!({
            "id": id, "ok": adopted_ok, "status": meta.status,
            "wins": meta.outcome.wins, "fails": meta.outcome.fails,
        }))?;
        let mut card = card_of(&rec).unwrap();
        card.wins = meta.outcome.wins;
        card.fails = meta.outcome.fails;
        card.status = meta.status.clone();
        card.success_rate = meta.outcome.success_rate();
        Ok(Some(card))
    }

    // ---- 内部 ----

    /// 生成并入库一张经验卡；已有同 (scene, task, type) 卡时返回 None（跳过）
    fn emit_card(
        &self,
        g: &TrajGroup,
        exp_type: &str,
        side: &[MemoryRecord],
        judge: Option<&dyn LlmJudge>,
        opts: &CompileOpts,
    ) -> Result<Option<i64>> {
        if self.find_existing(&g.scene, &g.task, exp_type)?.is_some() {
            return Ok(None);
        }
        let (title, kind) = match exp_type {
            "practice" => (format!("经验·做法:{}", g.task), MemoryKind::Workflow),
            _ => (format!("经验·避坑:{}", g.task), MemoryKind::Case),
        };
        let (trigger, steps) = distill(g, exp_type, side, judge, opts.clip);
        let body = match exp_type {
            "practice" => format!("【做法】任务「{}」的可复用做法。\n适用:{}\n要点:\n{}", g.task, trigger,
                steps.iter().enumerate().map(|(i, s)| format!("{}. {}", i + 1, s)).collect::<Vec<_>>().join("\n")),
            _ => format!("【避坑】任务「{}」的失败教训。\n适用:{}\n规避:\n{}", g.task, trigger,
                steps.iter().enumerate().map(|(i, s)| format!("{}. {}", i + 1, s)).collect::<Vec<_>>().join("\n")),
        };
        let mut rec = MemoryRecord::new(Tier::Knowledge, kind, title, body);
        rec.scene = g.scene.clone();
        rec.entities = top_entities(g, side);
        let (wins, fails) = (0, 0); // outcome 只计采纳反馈;编译期证据数由 source_ids 体现
        let meta = ExperienceMeta {
            v: EXPERIENCE_VERSION,
            exp_type: exp_type.into(),
            task: g.task.clone(),
            trigger,
            steps,
            outcome: Outcome { wins, fails },
            source_ids: side.iter().map(|r| r.id).collect(),
            status: "draft".into(),
        };
        rec.meta = serde_json::json!({ "experience": meta });
        rec.confidence = (0.5 + 0.05 * side.len().min(8) as f64).min(0.9);
        let (id, outcomes) = self.put_knowledge_with_conflicts(rec, judge, 0.8)?;
        self.repair_merged_meta(id, &outcomes)?;
        Ok(Some(id))
    }

    /// 冲突仲裁若把新卡 merge/supersede 掉，幸存卡（版本链后继）会缺失 meta.experience
    /// （core 的合并路径不搬运业务 meta）。这里把经验字段补写到幸存卡上，保持经验体系连续。
    fn repair_merged_meta(&self, new_id: i64, outcomes: &[crate::conflict::ConflictOutcome]) -> Result<()> {
        let merged = outcomes
            .iter()
            .any(|o| o.resolution.as_str() == "merge" || o.resolution.as_str() == "supersede");
        if !merged {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        let survivor: Option<(i64, String)> = conn
            .query_row(
                "SELECT id, meta FROM memories WHERE (id = ?1 OR version_of = ?1) AND is_current = 1 AND tombstone = 0 ORDER BY id DESC LIMIT 1",
                rusqlite::params![new_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let Some((sid, meta_str)) = survivor else { return Ok(()) };
        if sid == new_id {
            return Ok(()); // 新卡本身幸存，无需修复
        }
        let mut obj: serde_json::Value = serde_json::from_str(&meta_str).unwrap_or_else(|_| serde_json::json!({}));
        if obj.is_null() || !obj.is_object() {
            obj = serde_json::json!({});
        }
        if obj.get("experience").is_some() {
            return Ok(()); // 幸存卡已有经验字段
        }
        drop(conn); // read_exp_meta 内部要再拿 conn 锁，必须先释放
        let exp = self.read_exp_meta(new_id)?;
        let Some(exp) = exp else { return Ok(()) };
        obj["experience"] = serde_json::to_value(&exp).unwrap_or(serde_json::Value::Null);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE memories SET meta = ?2 WHERE id = ?1",
            rusqlite::params![sid, obj.to_string()],
        )?;
        drop(conn);
        self.audit("experience_compile", &serde_json::json!({"repaired_survivor": sid, "from": new_id}))?;
        Ok(())
    }

    fn read_exp_meta(&self, id: i64) -> Result<Option<ExperienceMeta>> {
        Ok(self.get(id)?.and_then(|r| parse_exp(&r.meta)))
    }

    fn find_existing(&self, scene: &str, task: &str, exp_type: &str) -> Result<Option<i64>> {
        for r in self.list(Some(Tier::Knowledge), Some(scene), false, 500)? {
            if let Some(m) = parse_exp(&r.meta) {
                if m.task == task && m.exp_type == exp_type {
                    return Ok(Some(r.id));
                }
            }
        }
        Ok(None)
    }

    /// 源轨迹打防重编标记（保留原 meta 其余字段，同 promotion.rs 的 distilled 模式）
    fn mark_sources(&self, recs: &[MemoryRecord]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        for r in recs {
            let mut obj = serde_json::from_str::<serde_json::Value>(&serde_json::to_string(&r.meta).unwrap_or_else(|_| "{}".into()))
                .unwrap_or_else(|_| serde_json::json!({}));
            if !obj.is_object() { obj = serde_json::json!({}); }
            obj[SRC_MARK] = serde_json::json!(true);
            conn.execute(
                "UPDATE memories SET meta = ?2 WHERE id = ?1",
                rusqlite::params![r.id, obj.to_string()],
            )?;
        }
        Ok(())
    }

    fn update_experience_meta(&self, id: i64, old_meta: &serde_json::Value, exp: &ExperienceMeta) -> Result<()> {
        let mut obj = old_meta.clone();
        if !obj.is_object() { obj = serde_json::json!({}); }
        obj["experience"] = serde_json::to_value(exp).unwrap_or(serde_json::Value::Null);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE memories SET meta = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id, obj.to_string(), chrono::Utc::now().timestamp()],
        )?;
        Ok(())
    }
}

/// 门控状态机
fn gate(o: &Outcome) -> String {
    if o.fails == 0 && o.wins >= VERIFY_WINS { "verified" } else { "draft" }.into()
}

fn parse_exp(meta: &serde_json::Value) -> Option<ExperienceMeta> {
    serde_json::from_value(meta.get("experience")?.clone()).ok()
}

/// 公共访问器：从记忆 meta 中取出经验卡字段（server 层格式化用）
pub fn experience_of(rec: &MemoryRecord) -> Option<ExperienceMeta> {
    parse_exp(&rec.meta)
}

fn is_marked(meta: &serde_json::Value, key: &str) -> bool {
    meta.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// 从轨迹 meta 提取 (tool, task, ok)
fn traj_fields(meta: &serde_json::Value) -> (Option<String>, Option<String>, Option<bool>) {
    (
        meta.get("tool").and_then(|v| v.as_str()).map(String::from),
        meta.get("task").and_then(|v| v.as_str()).map(String::from),
        meta.get("ok").and_then(|v| v.as_bool()),
    )
}

fn card_of(r: &MemoryRecord) -> Option<ExperienceCard> {
    let m = parse_exp(&r.meta)?;
    Some(ExperienceCard {
        id: r.id,
        title: r.title.clone(),
        scene: r.scene.clone(),
        content: r.content.clone(),
        exp_type: m.exp_type.clone(),
        task: m.task.clone(),
        trigger: m.trigger.clone(),
        steps: m.steps.clone(),
        wins: m.outcome.wins,
        fails: m.outcome.fails,
        success_rate: m.outcome.success_rate(),
        status: m.status.clone(),
        source_ids: m.source_ids.clone(),
        created_at: r.created_at,
    })
}

fn top_entities(g: &TrajGroup, side: &[MemoryRecord]) -> Vec<String> {
    let mut ents = vec![g.task.clone()];
    for r in side.iter().take(3) {
        if let Some(tool) = traj_fields(&r.meta).0 {
            if !ents.contains(&tool) {
                ents.push(tool);
            }
        }
    }
    ents
}

/// 提炼 trigger+steps:LLM 优先,规则兜底
fn distill(
    g: &TrajGroup,
    exp_type: &str,
    side: &[MemoryRecord],
    judge: Option<&dyn LlmJudge>,
    clip: usize,
) -> (String, Vec<String>) {
    if let Some(j) = judge {
        if let Some((t, s)) = llm_distill(j, g, exp_type, side, clip) {
            return (t, s);
        }
    }
    rule_distill(g, exp_type, side, clip)
}

fn llm_distill(
    j: &dyn LlmJudge,
    g: &TrajGroup,
    exp_type: &str,
    side: &[MemoryRecord],
    clip: usize,
) -> Option<(String, Vec<String>)> {
    let sys = match exp_type {
        "practice" => "你是 OS Agent 的经验提炼器。输入同一任务的若干条成功工具调用轨迹,提炼一条可复用做法。输出严格 JSON:{\"trigger\":\"何时适用(一句话)\",\"steps\":[\"要点\",...]}。steps 2-5 条、每条一句话。不要输出 JSON 以外的任何内容。",
        _ => "你是 OS Agent 的经验提炼器。输入同一任务的若干条失败工具调用轨迹,提炼一份规避清单。输出严格 JSON:{\"trigger\":\"何时会踩坑(一句话)\",\"steps\":[\"规避要点\",...]}。steps 2-5 条、每条一句话。不要输出 JSON 以外的任何内容。",
    };
    let mut user = format!("任务:{}\n轨迹:\n", g.task);
    for r in side.iter().take(6) {
        user.push_str(&format!("- {}\n", char_clip(&r.content, clip)));
    }
    let out = j.complete(sys, &user)?;
    let st = out.find('{')?;
    let en = out.rfind('}')?;
    let v: serde_json::Value = serde_json::from_str(&out[st..=en]).ok()?;
    let trigger = v.get("trigger")?.as_str()?.trim().to_string();
    let steps: Vec<String> = v.get("steps")?.as_array()?
        .iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty()).take(5).collect();
    (trigger.len() >= 2 && steps.len() >= 1).then_some((trigger, steps))
}

fn rule_distill(
    g: &TrajGroup,
    exp_type: &str,
    side: &[MemoryRecord],
    clip: usize,
) -> (String, Vec<String>) {
    // 统计各工具在同侧的出现次数
    let mut tools: Vec<(String, usize)> = Vec::new();
    for r in side {
        if let Some(t) = traj_fields(&r.meta).0 {
            match tools.iter_mut().find(|(x, _)| x == &t) {
                Some((_, n)) => *n += 1,
                None => tools.push((t, 1)),
            }
        }
    }
    tools.sort_by(|a, b| b.1.cmp(&a.1));
    let trigger = match exp_type {
        "practice" => format!("当需要完成任务「{}」时", g.task),
        _ => format!("执行任务「{}」时注意", g.task),
    };
    let mut steps: Vec<String> = Vec::new();
    for (t, n) in tools.iter().take(2) {
        match exp_type {
            "practice" => steps.push(format!("优先使用工具 `{}`(该任务下成功 {} 次)", t, n)),
            _ => steps.push(format!("工具 `{}` 在该任务下曾失败 {} 次,改用前先确认", t, n)),
        }
    }
    for r in side.iter().take(3) {
        if let Some(seg) = output_clip(&r.content) {
            steps.push(seg);
        }
    }
    if steps.is_empty() {
        steps.push(char_clip(&side[0].content, clip));
    }
    steps.truncate(5);
    (trigger, steps)
}

/// 从入库正文中截取"输出:"片段作为步骤素材
fn output_clip(content: &str) -> Option<String> {
    let idx = content.find("输出:")?;
    let seg = content[idx + "输出:".len()..].trim();
    let seg = seg.split('。').next().unwrap_or(seg).trim();
    (!seg.is_empty()).then(|| format!("参考输出:{}", char_clip(seg, 80)))
}

fn char_clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_hook::NoLlm;
    use crate::model::MemoryKind;

    fn store() -> MemoryStore {
        let dir = tempfile::tempdir().unwrap().into_path();
        MemoryStore::open(&dir.join("m.db"), &dir.join("fts"), Box::new(crate::embedding::HashEmbedder::new(64))).unwrap()
    }

    fn traj(s: &MemoryStore, tool: &str, task: &str, ok: bool, out: &str) -> i64 {
        let mut rec = MemoryRecord::new(
            Tier::Episodic, MemoryKind::ToolResult,
            format!("工具调用 {tool}"),
            format!("工具 `{tool}` 执行{}。任务:{task}。输出:{out}", if ok { "成功" } else { "失败" }),
        );
        rec.scene = "agent".into();
        rec.meta = serde_json::json!({"tool": tool, "task": task, "ok": ok});
        s.put(rec).unwrap()
    }

    #[test]
    fn test_compile_practice_and_pitfall() {
        let s = store();
        traj(&s, "libreoffice", "doc_edit", true, "导出 pdf 成功");
        traj(&s, "libreoffice", "doc_edit", true, "转档完成");
        traj(&s, "soffice-cli", "doc_edit", false, "参数不识别退出");
        let rep = s.compile_experiences(Some(&NoLlm), &CompileOpts::default()).unwrap();
        assert_eq!(rep.practice, 1, "应出一张做法卡");
        assert_eq!(rep.pitfall, 1, "应出一张避坑卡");
        let cards = s.list_experiences().unwrap();
        assert_eq!(cards.len(), 2);
        assert!(cards.iter().all(|c| c.status == "draft"));

        // 幂等:源轨迹已标记,重编译无分组可消费
        let rep2 = s.compile_experiences(Some(&NoLlm), &CompileOpts::default()).unwrap();
        assert_eq!(rep2.compiled, 0);
        assert_eq!(rep2.skipped_existing, 0);
        assert_eq!(s.list_experiences().unwrap().len(), 2);
    }

    #[test]
    fn test_gate_state_machine() {
        let s = store();
        traj(&s, "wps", "doc_edit", true, "ok1");
        traj(&s, "wps", "doc_edit", true, "ok2");
        s.compile_experiences(Some(&NoLlm), &CompileOpts::default()).unwrap();
        let id = s.list_experiences().unwrap().into_iter()
            .find(|c| c.exp_type == "practice").unwrap().id;
        // 一次成功:仍 draft
        let c = s.experience_feedback(id, true).unwrap().unwrap();
        assert_eq!(c.status, "draft");
        // 二次成功:verified
        let c = s.experience_feedback(id, true).unwrap().unwrap();
        assert_eq!(c.status, "verified");
        // 出现失败:回退 draft,计数保留
        let c = s.experience_feedback(id, false).unwrap().unwrap();
        assert_eq!(c.status, "draft");
        assert_eq!((c.wins, c.fails), (2, 1));
        // 非经验卡 id
        assert!(s.experience_feedback(99999, true).unwrap().is_none());
    }

    #[test]
    fn test_repair_merged_meta() {
        // 直接构造"新卡被合并、幸存卡缺 meta"的场景，验证修复路径（含锁顺序回归）
        let s = store();
        traj(&s, "wps", "export", true, "导出成功");
        let mut new_card = MemoryRecord::new(
            Tier::Knowledge, MemoryKind::Workflow,
            "经验·做法:export", "【做法】任务「export」的可复用做法。\n适用:x\n要点:\n1. 用 wps",
        );
        new_card.scene = "agent".into();
        new_card.entities = vec!["export".into(), "wps".into()];
        new_card.meta = serde_json::json!({"experience": ExperienceMeta {
            v: EXPERIENCE_VERSION, exp_type: "practice".into(), task: "export".into(),
            trigger: "当需要 export".into(), steps: vec!["用 wps".into()],
            outcome: Outcome::default(), source_ids: vec![], status: "draft".into(),
        }});
        let new_id = s.put(new_card).unwrap();
        // 模拟仲裁结果：幸存卡是新卡版本链的后继，且无 experience 字段
        let conn = s.conn.lock().unwrap();
        conn.execute(
            "UPDATE memories SET is_current = 0 WHERE id = ?1",
            rusqlite::params![new_id],
        ).unwrap();
        conn.execute(
            "INSERT INTO memories(tier, kind, title, content, scene, source, confidence, sensitivity, meta, created_at, updated_at, version_of, is_current, entity_keys) VALUES ('knowledge','workflow','合并卡','正文','agent','test',0.8,'none','{}',strftime('%s','now'),strftime('%s','now'),?1,1,'[]')",
            rusqlite::params![new_id],
        ).unwrap();
        drop(conn);
        // 幸存者≠新卡 + outcome 含 supersede → 触发修复
        use crate::conflict::{ConflictOutcome, ConflictType, Resolution};
        let outcomes = vec![ConflictOutcome {
            ctype: ConflictType::MethodObsolete,
            resolution: Resolution::Supersede,
            reason: "test".into(),
            decided_by: "test",
        }];
        s.repair_merged_meta(new_id, &outcomes).unwrap();
        let cards = s.list_experiences().unwrap();
        assert_eq!(cards.len(), 1, "幸存卡应带经验字段进入列表");
        assert_eq!(cards[0].exp_type, "practice");
        assert_eq!(s.experience_feedback(cards[0].id, true).unwrap().unwrap().wins, 1);
    }

    #[test]
    fn test_source_mark_prevents_recompile() {
        let s = store();
        traj(&s, "a", "t1", true, "x");
        s.compile_experiences(Some(&NoLlm), &CompileOpts::default()).unwrap();
        traj(&s, "a", "t1", true, "y"); // 新轨迹,但旧卡已存在 → skip,不新建
        let rep = s.compile_experiences(Some(&NoLlm), &CompileOpts::default()).unwrap();
        assert_eq!(rep.compiled, 0);
        assert_eq!(s.list_experiences().unwrap().len(), 1);
    }

    #[test]
    fn test_merge_keeps_experience_meta() {
        // 同任务同工具的成功/失败卡易被仲裁合并；幸存卡必须继承 meta.experience
        let s = store();
        traj(&s, "wps", "export", true, "导出成功");
        traj(&s, "wps", "export", false, "导出超时失败");
        s.compile_experiences(Some(&NoLlm), &CompileOpts::default()).unwrap();
        let cards = s.list_experiences().unwrap();
        // 无论仲裁结果是共存还是合并,经验卡都必须可见且可反馈
        assert!(!cards.is_empty(), "合并后经验卡不应从体系掉队");
        for c in &cards {
            assert!(c.status == "draft" || c.status == "verified");
            let fb = s.experience_feedback(c.id, true).unwrap().unwrap();
            assert_eq!(fb.wins, 1);
        }
    }
}
