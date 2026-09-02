//! 繁→简归一化（检索侧基础设施）。
//!
//! 场景：端侧知识库常含繁体文本（古籍、港台文档），而 OS Agent 查询为简体。
//! 中文优化 BM25 的 CJK 二元分词与向量嵌入在简繁不一致时全部失配，
//! 因此入库与查询两侧统一做繁→简单字归一化（OpenCC TSCharacters 表，
//! 取每键首个变体；简繁同形字原样保留）。

mod t2s_table;

/// 判断是否需要归一化：存在表内键字符才转换，纯简体/ASCII 输入零成本返回。
pub fn to_simplified(s: &str) -> String {
    if !s.chars().any(|c| t2s_table::T2S.binary_search_by(|p| p.0.cmp(&(c as u32))).is_ok()) {
        return s.to_string();
    }
    s.chars()
        .map(|c| match t2s_table::T2S.binary_search_by(|p| p.0.cmp(&(c as u32))) {
            Ok(i) => char::from_u32(t2s_table::T2S[i].1).unwrap_or(c),
            Err(_) => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::to_simplified;

    #[test]
    fn test_t2s() {
        assert_eq!(to_simplified("龐令明臺櫬決死戰"), "庞令明台榇决死战");
        assert_eq!(to_simplified("諸葛亮糧車"), "诸葛亮粮车");
        // 简体/英文/数字原样
        assert_eq!(to_simplified("诸葛亮 export pdf 42"), "诸葛亮 export pdf 42");
    }
}
