//! LoCoMo 适配器：超长多轮对话记忆问答基准（10 段对话 / 约 2000 问）。
//! 类别：1=单跳 2=多跳 3=时序 4=开放域 5=对抗。

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::dataset::{anyhow_like::Result, Conversation, Question, Session, Turn};

#[derive(Deserialize)]
struct RawConversation {
    #[serde(rename = "sample_id")]
    sample_id: serde_json::Value,
    conversation: BTreeMap<String, serde_json::Value>,
    qa: Vec<RawQa>,
}

#[derive(Deserialize)]
struct RawQa {
    question: String,
    #[serde(default)]
    answer: Option<serde_json::Value>,
    /// 对抗类问题（category 5）使用该字段：正确行为是回答"未提及"
    #[serde(default)]
    adversarial_answer: Option<serde_json::Value>,
    #[serde(default)]
    evidence: Vec<String>,
    category: serde_json::Value,
}

#[derive(Deserialize)]
struct RawTurn {
    speaker: String,
    #[serde(rename = "dia_id")]
    dia_id: String,
    text: String,
}

pub struct LoCoMo;

impl crate::dataset::Dataset for LoCoMo {
    fn name(&self) -> &'static str {
        "locomo"
    }

    fn load(&self, path: &std::path::Path) -> Result<Vec<Conversation>> {
        let raw: Vec<RawConversation> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        let mut out = Vec::with_capacity(raw.len());
        for rc in raw {
            let id = match rc.sample_id {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s,
                _ => format!("conv{}", out.len()),
            };
            // conversation 是无序 map：按 session_N 数字排序抽取
            let mut session_nums: Vec<u32> = rc
                .conversation
                .keys()
                .filter_map(|k| k.strip_prefix("session_").and_then(|s| s.parse().ok()))
                .collect();
            session_nums.sort_unstable();
            let mut sessions = Vec::new();
            for n in session_nums {
                let turns_v = rc
                    .conversation
                    .get(&format!("session_{n}"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                // session 可能是 turn 数组，也可能是 {dialog: [...]} 结构，两种都兼容
                let turns_json: Vec<serde_json::Value> = match turns_v {
                    serde_json::Value::Array(a) => a,
                    serde_json::Value::Object(o) => o
                        .get("dialog")
                        .and_then(|d| d.as_array())
                        .cloned()
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                let turns: Vec<Turn> = turns_json
                    .iter()
                    .filter_map(|t| {
                        Some(Turn {
                            dia_id: t.get("dia_id")?.as_str()?.to_string(),
                            speaker: t.get("speaker")?.as_str()?.to_string(),
                            text: t.get("text")?.as_str()?.to_string(),
                        })
                    })
                    .collect();
                if !turns.is_empty() {
                    let date_hint = rc
                        .conversation
                        .get(&format!("session_{n}_date_time"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    sessions.push(Session { turns, date_hint });
                }
            }
            let questions: Vec<Question> = rc
                .qa
                .into_iter()
                .map(|q| Question {
                    question: q.question,
                    answer: match q.answer.or(q.adversarial_answer) {
                        Some(serde_json::Value::String(s)) => s,
                        Some(v) => v.to_string(),
                        None => String::new(),
                    },
                    evidence: q.evidence,
                    category: match q.category {
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::String(s) => s,
                        v => v.to_string(),
                    },
                })
                .collect();
            out.push(Conversation { id, sessions, questions });
        }
        Ok(out)
    }
}
