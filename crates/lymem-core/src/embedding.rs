//! 嵌入抽象：核心库只依赖该 trait，具体后端由 lymem-kylin 提供。

use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// 嵌入器：把文本映射为定长向量。
/// 实现方必须线程安全；嵌入失败应返回错误而非 panic。
pub trait Embedder: Send + Sync {
    /// 向量维度（建库时确定，必须与存储一致）
    fn dim(&self) -> usize;
    /// 批量嵌入
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// 单条嵌入便捷方法
    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.embed(&[text])?.into_iter().next().unwrap_or_default())
    }
}

/// OpenAI 兼容的嵌入响应载荷（/v1/embeddings 代理用）
#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAiEmbeddingResponse {
    pub object: String,
    pub data: Vec<OpenAiEmbeddingData>,
    pub model: String,
    pub usage: OpenAiEmbeddingUsage,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAiEmbeddingData {
    pub object: String,
    pub index: usize,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAiEmbeddingUsage {
    pub prompt_tokens: u64,
    pub total_tokens: u64,
}

/// 测试/离线兜底嵌入器：特征哈希（feature hashing）。
/// 确定性、零依赖，语义能力有限，仅用于单测和无麒麟环境冒烟。
pub struct HashEmbedder {
    pub dim: usize,
}

impl HashEmbedder {
    pub fn new(dim: usize) -> Self {
        HashEmbedder { dim }
    }
    fn hash_token(token: &str, salt: usize) -> usize {
        // FNV-1a 变体，加盐避免相邻槽位碰撞
        let mut h: usize = 0xcbf29ce484222325usize ^ salt;
        for b in token.as_bytes() {
            h ^= *b as usize;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }
    fn tokenize(text: &str) -> Vec<String> {
        // 英文按词、中文按 2-gram 切分，与检索侧分析器保持接近
        let mut out = Vec::new();
        let mut ascii_buf = String::new();
        let mut cjk_buf: Vec<char> = Vec::new();
        let flush_ascii = |buf: &mut String, out: &mut Vec<String>| {
            if !buf.is_empty() {
                out.push(buf.to_lowercase());
                buf.clear();
            }
        };
        let flush_cjk = |buf: &mut Vec<char>, out: &mut Vec<String>| {
            if buf.is_empty() {
                return;
            }
            if buf.len() == 1 {
                out.push(buf.iter().collect());
            } else {
                for w in buf.windows(2) {
                    out.push(w.iter().collect());
                }
            }
            buf.clear();
        };
        for c in text.chars() {
            if c.is_ascii_alphanumeric() {
                ascii_buf.push(c);
            } else {
                flush_ascii(&mut ascii_buf, &mut out);
                if ('\u{4e00}'..='\u{9fff}').contains(&c) {
                    cjk_buf.push(c);
                } else {
                    flush_cjk(&mut cjk_buf, &mut out);
                }
            }
        }
        flush_ascii(&mut ascii_buf, &mut out);
        flush_cjk(&mut cjk_buf, &mut out);
        out
    }
}

impl Embedder for HashEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            let mut v = vec![0f32; self.dim];
            for tok in Self::tokenize(t) {
                let idx1 = Self::hash_token(&tok, 1) % self.dim;
                let idx2 = Self::hash_token(&tok, 2) % self.dim;
                v[idx1] += 1.0;
                v[idx2] += 0.5;
            }
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for x in v.iter_mut() {
                    *x /= norm;
                }
            }
            out.push(v);
        }
        Ok(out)
    }
}

/// 向量工具：f32 数组 <-> 小端字节（sqlite-vec blob 格式）
pub fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for f in v {
        b.extend_from_slice(&f.to_le_bytes());
    }
    b
}

pub fn blob_to_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let dot: f32 = a[..n].iter().zip(&b[..n]).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// 便捷：构造嵌入错误
pub fn embed_err(msg: impl Into<String>) -> CoreError {
    CoreError::Embed(msg.into())
}
