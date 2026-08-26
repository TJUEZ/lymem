//! 敏感信息识别与脱敏（赛题条款 5 的规则层）。
//!
//! 纯 Rust 规则：手机号/身份证/银行卡/邮箱/IP/常见密钥形态/口令赋值。
//! LLM 复核打标在 ingest 管道中可选叠加（见 llm_hook）。

use once_set::OnceInit;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SensitiveFinding {
    /// 规则类别：phone / idcard / bankcard / email / ip / apikey / password
    pub kind: String,
    /// 命中原文（入库的审计里做部分遮蔽）
    pub matched: String,
    /// 字节偏移区间
    pub start: usize,
    pub end: usize,
}

// 懒加载静态正则集合
mod once_set {
    use std::sync::OnceLock;
    pub struct OnceInit<T>(OnceLock<T>);
    impl<T> OnceInit<T> {
        pub const fn new() -> Self {
            OnceInit(OnceLock::new())
        }
        pub fn get_or_init(&self, f: impl FnOnce() -> T) -> &T {
            self.0.get_or_init(f)
        }
    }
}

struct Rules {
    phone: Regex,
    idcard: Regex,
    bankcard: Regex,
    email: Regex,
    ip: Regex,
    apikey: Regex,
    password: Regex,
}

static RULES: OnceInit<Rules> = OnceInit::new();

fn rules() -> &'static Rules {
    RULES.get_or_init(|| {
        // Rust regex 不支持 lookaround：用「边界字符消费 + 捕获组」表达前后非数字约束
        Rules {
            // 大陆手机号：1[3-9] 开头 11 位
            phone: Regex::new(r"(?:^|[^0-9])(1[3-9][0-9]{9})(?:$|[^0-9])").unwrap(),
            // 身份证 18 位（末位可为 X）
            idcard: Regex::new(r"(?:^|[^0-9Xx])([0-9]{17}[0-9Xx])(?:$|[^0-9Xx])").unwrap(),
            // 银行卡：16~19 位数字，允许 4 位分组空格
            bankcard: Regex::new(r"(?:^|[^0-9])([0-9]{4}(?: ?[0-9]{4}){2,3}[0-9]{3})(?:$|[^0-9])").unwrap(),
            email: Regex::new(r"([0-9A-Za-z._%+-]+@[0-9A-Za-z.-]+\.[A-Za-z]{2,})").unwrap(),
            ip: Regex::new(r"(?:^|[^0-9.])((?:[0-9]{1,3}\.){3}[0-9]{1,3})(?:$|[^0-9.])").unwrap(),
            // 常见密钥形态：sk- / ghp_ / AKIA / xoxb- / eyJ(JWT)
            apikey: Regex::new(
                r"(sk-[A-Za-z0-9_-]{16,}|ghp_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|xoxb-[A-Za-z0-9-]{20,}|eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{10,})",
            )
            .unwrap(),
            // 口令赋值：password=xxx / 密码：xxx
            password: Regex::new(
                r#"(?i)(password|passwd|pwd|secret|token|api[_-]?key|密码|口令|密钥)\s*[:=]\s*("[^"]{4,}"|\S{4,})"#,
            )
            .unwrap(),
        }
    })
}

/// 扫描文本中的敏感信息
pub fn scan(text: &str) -> Vec<SensitiveFinding> {
    let mut out = Vec::new();
    let r = rules();
    // 带边界消费的模式：取第 1 捕获组（真正的敏感串）
    fn group_finding(kind: &str, m: regex::Match) -> SensitiveFinding {
        SensitiveFinding {
            kind: kind.to_string(),
            matched: m.as_str().to_string(),
            start: m.start(),
            end: m.end(),
        }
    }
    for c in r.idcard.captures_iter(text) {
        if let Some(m) = c.get(1) {
            out.push(group_finding("idcard", m));
        }
    }
    for c in r.bankcard.captures_iter(text) {
        if let Some(m) = c.get(1) {
            out.push(group_finding("bankcard", m));
        }
    }
    for c in r.phone.captures_iter(text) {
        if let Some(m) = c.get(1) {
            out.push(group_finding("phone", m));
        }
    }
    for m in r.apikey.find_iter(text) {
        out.push(group_finding("apikey", m));
    }
    for c in r.password.captures_iter(text) {
        if let Some(m) = c.get(1) {
            out.push(group_finding("password", m));
        }
    }
    for m in r.email.find_iter(text) {
        out.push(group_finding("email", m));
    }
    for c in r.ip.captures_iter(text) {
        if let Some(m) = c.get(1) {
            out.push(group_finding("ip", m));
        }
    }
    out.sort_by_key(|f| f.start);
    out
}

/// 按类别确定阻断等级：密钥/口令/身份证/银行卡 → 阻断；其余 → 敏感
pub fn level_of(findings: &[SensitiveFinding]) -> crate::model::Sensitivity {
    use crate::model::Sensitivity;
    if findings
        .iter()
        .any(|f| matches!(f.kind.as_str(), "apikey" | "password" | "idcard" | "bankcard"))
    {
        Sensitivity::Blocked
    } else if !findings.is_empty() {
        Sensitivity::Sensitive
    } else {
        Sensitivity::None
    }
}

/// 抽取文本中被引号/书名号包裹的主题词（遗忘指令规则解析用）
pub fn scan_free_quoted(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    let mut buf = String::new();
    let mut in_quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match in_quote {
            None => {
                if c == '"' || c == '“' || c == '《' || c == '‘' {
                    in_quote = Some(match c {
                        '“' => '”',
                        '《' => '》',
                        '‘' => '’',
                        _ => '"',
                    });
                    buf.clear();
                }
            }
            Some(close) => {
                if c == close {
                    let t = buf.trim().to_string();
                    if !t.is_empty() && t.chars().count() <= 40 {
                        out.push(t);
                    }
                    in_quote = None;
                } else {
                    buf.push(c);
                }
            }
        }
    }
    out
}

/// 脱敏：将命中片段替换为 [类型:长度] 占位符
pub fn redact(text: &str) -> String {
    let findings = scan(text);
    if findings.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut pos = 0usize;
    for f in findings {
        if f.start < pos {
            continue; // 跳过重叠命中
        }
        out.push_str(&text[pos..f.start]);
        out.push_str(&format!("[{}:{}]", f.kind, f.end - f.start));
        pos = f.end;
    }
    out.push_str(&text[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Sensitivity;

    #[test]
    fn test_scan_and_redact() {
        let text = "联系我 13812345678，邮箱 a@b.com，key: sk-abcdefghijklmnopqrst";
        let f = scan(text);
        let kinds: Vec<_> = f.iter().map(|x| x.kind.as_str()).collect();
        assert!(kinds.contains(&"phone"));
        assert!(kinds.contains(&"email"));
        assert!(kinds.contains(&"apikey"));
        assert_eq!(level_of(&f), Sensitivity::Blocked);
        let red = redact(text);
        assert!(!red.contains("13812345678"));
        assert!(!red.contains("sk-abcdefghijklmnopqrst"));
        assert!(red.contains("[phone:11]"));
    }

    #[test]
    fn test_idcard() {
        let f = scan("身份证号 110101199003077658 备案");
        assert!(f.iter().any(|x| x.kind == "idcard"));
    }
}
