//! 把经验卡与偏好编译为 Agent 可直接加载的标准 `SKILL.md` 技能目录。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::experience::ExperienceCard;
use crate::preference::PreferenceView;

pub const SKILL_NAME: &str = "lymem-experience";

#[derive(Debug, Clone, Serialize)]
pub struct SkillTarget {
    pub id: String,
    pub agent: String,
    pub path: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillRelease {
    pub target: String,
    pub path: String,
    pub version: String,
    pub cards: usize,
    pub preferences: usize,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillManifest {
    pub schema: u8,
    pub name: String,
    pub version: String,
    pub generated_at: String,
    pub managed_by: String,
    /// SKILL.md 全文内容指纹（fnv1a64），发布后可校验文件是否被篡改
    pub digest: String,
    pub cards: Vec<i64>,
    pub preferences: Vec<String>,
}

pub fn targets() -> Vec<SkillTarget> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let defs = [
        ("codex", "Codex", ".codex/skills"),
        ("claude", "Claude Code", ".claude/skills"),
        ("opencode", "OpenCode", ".config/opencode/skills"),
        ("dsh", "DeepSeek Harness", ".agents/skills"),
    ];
    defs.into_iter().map(|(id, agent, base)| {
        let path = PathBuf::from(&home).join(base).join(SKILL_NAME);
        SkillTarget { id: id.into(), agent: agent.into(), installed: path.join("SKILL.md").is_file(), path: path.to_string_lossy().into() }
    }).collect()
}

pub fn render(cards: &[ExperienceCard], prefs: &[PreferenceView], version: &str) -> String {
    let mut out = format!("---\nname: {SKILL_NAME}\ndescription: lymem 自动维护的可执行经验与用户偏好；在执行相似任务前加载，完成后通过 MCP memory_feedback 回报结果。\nmetadata:\n  version: {version}\n  managed-by: lymem\n---\n\n# lymem 经验技能\n\n本文件由 lymem 从执行轨迹和偏好版本链自动编译。先匹配适用条件，再执行步骤；避坑规则优先于做法。\n\n## 使用协议\n\n1. 开始任务前调用 `memory_search` 补充最新上下文。\n2. 只采用适用条件匹配当前任务的经验。\n3. 完成后调用 `memory_feedback` 回报经验复用成败。\n4. 实时事实与本技能冲突时以实时事实为准，并写回新记忆。\n");
    if !prefs.is_empty() {
        out.push_str("\n## 当前偏好\n\n");
        for p in prefs { out.push_str(&format!("- `{}` = {}（场景：{}，置信度 {:.2}）\n", p.key, p.value, p.scenes.join("/"), p.confidence)); }
    }
    let mut practices: Vec<&ExperienceCard> = cards.iter().filter(|c| c.exp_type == "practice").collect();
    let mut pitfalls: Vec<&ExperienceCard> = cards.iter().filter(|c| c.exp_type == "pitfall").collect();
    practices.sort_by_key(|c| c.id); pitfalls.sort_by_key(|c| c.id);
    for (title, list) in [("可执行做法", practices), ("避坑规则", pitfalls)] {
        if list.is_empty() { continue; }
        out.push_str(&format!("\n## {title}\n"));
        for c in list {
            out.push_str(&format!("\n### {}\n\n- 适用：{}\n- 状态：{}；复用成功 {} 次，失败 {} 次\n- 来源：lymem #{}，证据轨迹 {:?}\n", c.title, c.trigger, c.status, c.wins, c.fails, c.id, c.source_ids));
            for (i, step) in c.steps.iter().enumerate() { out.push_str(&format!("{}. {}\n", i + 1, step)); }
        }
    }
    out
}

pub fn publish(target: &SkillTarget, cards: &[ExperienceCard], prefs: &[PreferenceView]) -> Result<SkillRelease> {
    let root = Path::new(&target.path);
    std::fs::create_dir_all(root.join("resources")).map_err(ioerr)?;
    let generated_at = chrono::Utc::now();
    let version = generated_at.format("%Y%m%dT%H%M%S%3fZ").to_string();
    let body = render(cards, prefs, &version);
    let versions = root.join(".lymem-versions");
    std::fs::create_dir_all(&versions).map_err(ioerr)?;
    if root.join("SKILL.md").is_file() {
        std::fs::copy(root.join("SKILL.md"), versions.join(format!("{version}.previous.md"))).map_err(ioerr)?;
    }
    let manifest = SkillManifest {
        schema: 1,
        name: SKILL_NAME.into(),
        version: version.clone(),
        generated_at: generated_at.to_rfc3339(),
        managed_by: "lymem".into(),
        digest: content_digest(&body),
        cards: cards.iter().map(|c| c.id).collect(),
        preferences: prefs.iter().map(|p| p.key.clone()).collect(),
    };
    // 同一文件系统内用 rename，避免 Agent 恰好读取到半个技能文件。
    atomic_write(&root.join("SKILL.md"), body.as_bytes())?;
    atomic_write(
        &root.join("resources/manifest.json"),
        &serde_json::to_vec_pretty(&manifest).map_err(|e| CoreError::InvalidInput(e.to_string()))?,
    )?;
    Ok(SkillRelease { target: target.id.clone(), path: target.path.clone(), version, cards: cards.len(), preferences: prefs.len(), bytes_written: body.len() as u64 })
}

pub fn rollback(target: &SkillTarget) -> Result<bool> {
    let versions = Path::new(&target.path).join(".lymem-versions");
    let Ok(entries) = std::fs::read_dir(&versions) else { return Ok(false) };
    let mut files: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
    files.sort();
    let Some(prev) = files.pop() else { return Ok(false) };
    let body = std::fs::read(&prev).map_err(ioerr)?;
    atomic_write(&Path::new(&target.path).join("SKILL.md"), &body)?;
    std::fs::remove_file(prev).map_err(ioerr)?;
    Ok(true)
}

pub fn remove(target: &SkillTarget) -> Result<bool> {
    let root = Path::new(&target.path);
    if !root.exists() { return Ok(false) }
    std::fs::remove_dir_all(root).map_err(ioerr)?;
    Ok(true)
}

fn ioerr(e: std::io::Error) -> CoreError { CoreError::InvalidInput(e.to_string()) }

/// SKILL.md 全文的 FNV-1a 64 位内容指纹，用于发布后校验完整性
fn content_digest(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{h:016x}")
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(ioerr)?;
    std::fs::rename(&tmp, path).map_err(ioerr)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renders_valid_skill_frontmatter() {
        let text = render(&[], &[], "v1");
        assert!(text.starts_with("---\nname: lymem-experience\n"));
        assert!(text.contains("## 使用协议"));
    }
}
