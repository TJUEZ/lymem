//! 全文索引（tantivy BM25），自定义中文二元语法分析器。

use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, INDEXED, STORED, STRING, Value};
use tantivy::tokenizer::{Token, TokenStream, Tokenizer};
use tantivy::{doc, Index, IndexReader, IndexWriter, TantivyDocument, Term};

/// 中文二元 + 英文词元的分析器：兼顾召回与索引体积，
/// 无需外置分词词典，适合端侧轻量化部署。
#[derive(Clone, Default)]
pub struct CjkBigramTokenizer;

impl Tokenizer for CjkBigramTokenizer {
    type TokenStream<'a> = CjkTokenStream;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> CjkTokenStream {
        CjkTokenStream {
            tokens: pre_tokenize(text),
            cursor: 0,
            token: Token::default(),
        }
    }
}

/// 预切词：英文/数字连串为一个词，连续中文按 bigram 展开
fn pre_tokenize(text: &str) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let (byte_start, c) = chars[i];
        if c.is_ascii_alphanumeric() {
            let mut j = i;
            while j < chars.len() && chars[j].1.is_alphanumeric() {
                j += 1;
            }
            let byte_end = chars[j - 1].0 + chars[j - 1].1.len_utf8();
            out.push((text[byte_start..byte_end].to_lowercase(), byte_start, byte_end));
            i = j;
        } else if ('\u{4e00}'..='\u{9fff}').contains(&c) {
            // 中文区间：滑动二元
            let mut j = i;
            while j < chars.len() && ('\u{4e00}'..='\u{9fff}').contains(&chars[j].1) {
                j += 1;
            }
            if j - i == 1 {
                let (_, ch) = chars[i];
                out.push((ch.to_string(), byte_start, byte_start + ch.len_utf8()));
            } else {
                for w in i..j - 1 {
                    let s = chars[w].0;
                    let e = chars[w + 1].0 + chars[w + 1].1.len_utf8();
                    out.push((text[s..e].to_string(), s, e));
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

pub struct CjkTokenStream {
    tokens: Vec<(String, usize, usize)>,
    cursor: usize,
    token: Token,
}

impl TokenStream for CjkTokenStream {
    fn advance(&mut self) -> bool {
        if self.cursor >= self.tokens.len() {
            return false;
        }
        let (ref text, from, to) = self.tokens[self.cursor];
        self.token = Token {
            offset_from: from,
            offset_to: to,
            position: self.cursor,
            text: text.clone(),
            position_length: 1,
        };
        self.cursor += 1;
        true
    }

    fn token(&self) -> &Token {
        &self.token
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.token
    }
}

/// 全文索引封装：一份记忆一个文档，按 chunk 建文档以支持片段级命中
pub struct FtsIndex {
    index: Index,
    writer: IndexWriter,
    reader: IndexReader,
    f_memory: Field,
    f_chunk: Field,
    f_content: Field,
    f_tier: Field,
    f_scene: Field,
}

pub const TOKENIZER_NAME: &str = "cjk_bigram";

impl FtsIndex {
    /// 在目录上创建或打开索引
    pub fn open(dir: &Path) -> tantivy::Result<FtsIndex> {
        let mut sb = Schema::builder();
        let f_memory = sb.add_u64_field("memory_id", INDEXED | STORED);
        let f_chunk = sb.add_u64_field("chunk_id", INDEXED | STORED);
        let indexing = TextFieldIndexing::default()
            .set_tokenizer(TOKENIZER_NAME)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions);
        let text_opts = TextOptions::default()
            .set_indexing_options(indexing)
            .set_stored();
        let f_content = sb.add_text_field("content", text_opts);
        let f_tier = sb.add_text_field("tier", STRING);
        let f_scene = sb.add_text_field("scene", STRING);
        let schema = sb.build();

        let index = if dir.join("meta.json").exists() {
            Index::open_in_dir(dir)?
        } else {
            std::fs::create_dir_all(dir)?;
            Index::create_in_dir(dir, schema)?
        };
        index.tokenizers().register(TOKENIZER_NAME, CjkBigramTokenizer);
        let writer = index.writer(32 * 1024 * 1024)?;
        let reader = index.reader()?;
        Ok(FtsIndex {
            index,
            writer,
            reader,
            f_memory,
            f_chunk,
            f_content,
            f_tier,
            f_scene,
        })
    }

    /// 写入一个 chunk 文档
    pub fn add_chunk(&mut self, memory_id: i64, chunk_id: i64, tier: &str, scene: &str, content: &str) -> tantivy::Result<()> {
        self.writer.add_document(doc!(
            self.f_memory => memory_id as u64,
            self.f_chunk => chunk_id as u64,
            self.f_tier => tier,
            self.f_scene => scene,
            self.f_content => content,
        ))?;
        Ok(())
    }

    /// 删除某记忆的全部 chunk 文档
    pub fn delete_memory(&mut self, memory_id: i64) -> tantivy::Result<()> {
        self.writer.delete_term(Term::from_field_u64(self.f_memory, memory_id as u64));
        Ok(())
    }

    pub fn commit(&mut self) -> tantivy::Result<()> {
        self.writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    /// BM25 检索：返回 (memory_id, chunk_id, 片段)，按得分降序
    pub fn search(&self, query: &str, tier: Option<&str>, limit: usize) -> tantivy::Result<Vec<(i64, i64, f32, String)>> {
        let searcher = self.reader.searcher();
        let mut parser = QueryParser::for_index(&self.index, vec![self.f_content]);
        // 默认析取（OR）召回优先：此前误设合取导致 BM25 通道在长查询下几乎无命中
        let q = parser.parse_query(query)?;
        let top = searcher.search(&q, &TopDocs::with_limit(limit))?;
        let mut out = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let Ok(doc) = searcher.doc::<TantivyDocument>(addr) else { continue };
            let memory_id = doc.get_first(self.f_memory).and_then(|v| v.as_u64()).unwrap_or(0) as i64;
            let chunk_id = doc.get_first(self.f_chunk).and_then(|v| v.as_u64()).unwrap_or(0) as i64;
            let tier_v = doc.get_first(self.f_tier).and_then(|v| v.as_str()).unwrap_or("").to_string();
            if let Some(t) = tier {
                if tier_v != t {
                    continue;
                }
            }
            let snippet = doc
                .get_first(self.f_content)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            out.push((memory_id, chunk_id, score, snippet));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pre_tokenize() {
        let toks = pre_tokenize("Rust 记忆模块开发");
        let texts: Vec<&str> = toks.iter().map(|t| t.0.as_str()).collect();
        assert!(texts.contains(&"rust"));
        assert!(texts.contains(&"记忆"));
        assert!(texts.contains(&"忆模"));
    }

    #[test]
    fn test_fts_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut fts = FtsIndex::open(dir.path()).unwrap();
        fts.add_chunk(1, 10, "knowledge", "coding", "银河麒麟操作系统的记忆模块采用 Rust 开发").unwrap();
        fts.add_chunk(2, 20, "knowledge", "office", "办公场景偏好使用 WPS 文档格式").unwrap();
        fts.commit().unwrap();
        let hits = fts.search("记忆 模块", None, 5).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].0, 1);
        let hits2 = fts.search("WPS 办公", None, 5).unwrap();
        assert_eq!(hits2[0].0, 2);
        fts.delete_memory(1).unwrap();
        fts.commit().unwrap();
        assert!(fts.search("记忆 模块", None, 5).unwrap().is_empty());
    }
}
