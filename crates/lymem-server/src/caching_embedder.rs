//! 查询向量 LRU 缓存装饰器：包一层 `Embedder`，相同文本直接命中缓存。
//!
//! 收益：检索热路径的查询向量每请求重算（端侧嵌入 ~30ms，占 p50 大头）；
//! `ask` 对同一查询二次嵌入；缓存同时消掉嵌入通道全局锁的重复竞争。
//! 容量按条数上限 + 简单Clock淘汰，避免引入额外依赖。

use lymem_core::embedding::Embedder;
use lymem_core::Result;
use std::collections::HashMap;
use std::sync::Mutex;

const CAPACITY: usize = 512;

pub struct CachingEmbedder {
    inner: Box<dyn Embedder>,
    cache: Mutex<Cache>,
}

struct Cache {
    map: HashMap<u64, Vec<f32>>,
    clock: u64,
    slots: HashMap<u64, u64>, // key -> 最近使用时钟
}

impl CachingEmbedder {
    pub fn new(inner: Box<dyn Embedder>) -> Self {
        Self { inner, cache: Mutex::new(Cache { map: HashMap::new(), clock: 0, slots: HashMap::new() }) }
    }
}

fn hash_text(text: &str) -> u64 {
    // FNV-1a 64：缓存键只求分布均匀，不承担安全属性
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Embedder for CachingEmbedder {
    fn dim(&self) -> usize {
        self.inner.dim()
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut cache = self.cache.lock().unwrap();
        cache.clock += 1;
        let now = cache.clock;
        let mut out: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        let mut miss: Vec<(usize, &str)> = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            let k = hash_text(t);
            let hit = cache.map.get(&k).cloned();
            match hit {
                Some(v) => {
                    cache.slots.insert(k, now);
                    out[i] = Some(v);
                }
                None => miss.push((i, t)),
            }
        }
        if !miss.is_empty() {
            let refs: Vec<&str> = miss.iter().map(|(_, t)| *t).collect();
            let vecs = self.inner.embed(&refs)?;
            for ((i, t), v) in miss.into_iter().zip(vecs.into_iter()) {
                let k = hash_text(t);
                if cache.map.len() >= CAPACITY {
                    // Clock 淘汰：逐出最久未用的一条（分步借用，避免 guard 借用冲突）
                    let oldest = {
                        let slots = &cache.slots;
                        let map = &cache.map;
                        slots.iter()
                            .filter(|(key, _)| map.contains_key(key))
                            .min_by_key(|(_, tm)| **tm)
                            .map(|(key, _)| *key)
                    };
                    if let Some(oldest) = oldest {
                        cache.map.remove(&oldest);
                        cache.slots.remove(&oldest);
                    }
                }
                cache.map.insert(k, v.clone());
                cache.slots.insert(k, now);
                out[i] = Some(v);
            }
        }
        Ok(out.into_iter().map(|v| v.unwrap_or_default()).collect())
    }
}
