//! # lymem-kylin（银河麒麟端侧 AI 绑定）
//!
//! 提供符合 `lymem_core::embedding::Embedder` 的麒麟端侧文本嵌入客户端，双通道：
//!
//! 1. **DBus 通道**（`KylinDbusEmbedder`）：走 `kylin-ai-runtime` 的官方 SDK 协议
//!    （`/tmp/.kylin-ai-runtime-unix/<uid>/core-textembedding.sock` 上的
//!    `com.kylin.AiRuntime.CoreTextEmbeddingService`，方法 `init` / `embedding_text`）；
//! 2. **kytensor 直连通道**（`KylinTritonEmbedder`）：当 runtime 引擎不可用时，
//!    直接调用 kytensor 推理后端的 Triton HTTP API（ensemble 模型），均值池化得到句向量。
//!    仍属银河麒麟端侧组件栈（与 SDK 同一后端），作为可靠性兜底。
//!
//! `KylinEmbedder::connect_auto()` 先探测 DBus，失败自动切换 kytensor。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use lymem_core::embedding::{embed_err, Embedder};
use serde_json::json;

const IFACE: &str = "com.kylin.AiRuntime.CoreTextEmbeddingService";
const OBJ_PATH: &str = "/com/kylin/AiRuntime/CoreTextEmbeddingService";

fn current_uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok()))
        })
        .unwrap_or(1000)
}

fn runtime_sock_default() -> String {
    format!("unix:path=/tmp/.kylin-ai-runtime-unix/{}/core-textembedding.sock", current_uid())
}

fn env_or(key: &str, default: String) -> String {
    std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or(default)
}

/// 麒麟嵌入客户端配置（均可环境变量覆盖）
#[derive(Debug, Clone)]
pub struct KylinConfig {
    /// kylin-ai-runtime DBus socket 地址
    pub runtime_sock: String,
    /// kytensor Triton HTTP 基址
    pub kytensor_url: String,
    /// 嵌入模型名
    pub model: String,
    /// 推理依赖的分词器/底座模型（预加载用）
    pub dependency_models: Vec<String>,
    pub timeout: Duration,
}

impl Default for KylinConfig {
    fn default() -> Self {
        KylinConfig {
            runtime_sock: env_or("LYMEM_KYLIN_RUNTIME_SOCK", runtime_sock_default()),
            kytensor_url: env_or("LYMEM_KYLIN_KYTENSOR_URL", "http://127.0.0.1:8000".into()),
            model: env_or("LYMEM_KYLIN_MODEL", "ensemble-embd_gte-base_uint8-text".into()),
            dependency_models: vec![
                "tokenizer_gte-base_uint8-text".into(),
                "embd_gte-base_uint8-text".into(),
            ],
            timeout: Duration::from_secs(60),
        }
    }
}

// ---------------- DBus 通道 ----------------

/// 官方 SDK DBus 协议客户端
pub struct KylinDbusEmbedder {
    proxy: zbus::blocking::Proxy<'static>,
    session_id: i32,
    dim_cache: AtomicUsize,
}

impl KylinDbusEmbedder {
    pub fn connect(cfg: &KylinConfig) -> Result<Self, String> {
        let conn = zbus::blocking::connection::Builder::address(cfg.runtime_sock.as_str())
            .map_err(|e| format!("连接 kylin-ai-runtime 失败: {e}"))?
            .p2p()
            .build()
            .map_err(|e| format!("建立 DBus 连接失败: {e}"))?;
        let proxy = zbus::blocking::Proxy::new(&conn, IFACE, OBJ_PATH, IFACE)
            .map_err(|e| format!("创建代理失败: {e}"))?;
        let (session_id, err): (i32, i32) = proxy
            .call("init", &(json!({"engineName": "Embedding"}).to_string(),))
            .map_err(|e| format!("init 调用失败: {e}"))?;
        if session_id == -1 {
            return Err(format!("init 返回无效会话（errorCode={err}）"));
        }
        Ok(KylinDbusEmbedder {
            proxy,
            session_id,
            dim_cache: AtomicUsize::new(768),
        })
    }
}

impl Embedder for KylinDbusEmbedder {
    fn dim(&self) -> usize {
        self.dim_cache.load(Ordering::Relaxed)
    }
    fn embed(&self, texts: &[&str]) -> lymem_core::Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            let payload = json!({"text": t, "sessionId": self.session_id}).to_string();
            let (resp,): (String,) = self
                .proxy
                .call("embedding_text", &(payload,))
                .map_err(|e| embed_err(format!("embedding_text 调用失败: {e}")))?;
            let v: serde_json::Value =
                serde_json::from_str(&resp).map_err(|e| embed_err(format!("响应解析失败: {e}")))?;
            if v.get("errorCode").and_then(|x| x.as_i64()).unwrap_or(0) != 0 {
                let msg = v.get("errorMessage").and_then(|x| x.as_str()).unwrap_or("unknown");
                return Err(embed_err(format!("麒麟嵌入服务错误: {msg}")));
            }
            let vec: Vec<f32> = v
                .get("vector_result")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|f| f.as_f64().map(|d| d as f32)).collect())
                .ok_or_else(|| embed_err("响应缺少 vector_result"))?;
            if !vec.is_empty() {
                self.dim_cache.store(vec.len(), Ordering::Relaxed);
            }
            out.push(vec);
        }
        Ok(out)
    }
}

// ---------------- kytensor 直连通道 ----------------

/// kytensor Triton HTTP 客户端（兜底通道）
pub struct KylinTritonEmbedder {
    cfg: KylinConfig,
    dim_cache: AtomicUsize,
}

impl KylinTritonEmbedder {
    pub fn connect(cfg: &KylinConfig) -> Result<Self, String> {
        let e = Self {
            cfg: cfg.clone(),
            dim_cache: AtomicUsize::new(768),
        };
        e.ensure_models()?;
        Ok(e)
    }

    /// 显式加载模型（幂等；kytensor 为 explicit 模式，冷启动必须）
    pub fn ensure_models(&self) -> Result<(), String> {
        let mut all = self.cfg.dependency_models.clone();
        all.push(self.cfg.model.clone());
        for m in all {
            let url = format!("{}/v2/repository/models/{}/load", self.cfg.kytensor_url, m);
            match ureq::post(&url).timeout(self.cfg.timeout).send_json(json!({})) {
                Ok(_) => tracing::info!("kytensor 模型已加载: {m}"),
                Err(e) => return Err(format!("加载模型 {m} 失败: {e}")),
            }
        }
        Ok(())
    }

    fn infer_raw(&self, text: &str) -> Result<(Vec<f32>, usize), String> {
        let url = format!("{}/v2/models/{}/infer", self.cfg.kytensor_url, self.cfg.model);
        let body = json!({
            "inputs": [{"name": "product_reviews", "shape": [1], "datatype": "BYTES", "data": [text]}],
            "outputs": [{"name": "logits"}]
        });
        let resp: serde_json::Value = ureq::post(&url)
            .timeout(self.cfg.timeout)
            .send_json(body)
            .map_err(|e| format!("infer 请求失败: {e}"))?
            .into_json()
            .map_err(|e| format!("infer 响应解析失败: {e}"))?;
        let out = resp
            .get("outputs")
            .and_then(|o| o.get(0))
            .ok_or("infer 响应缺少 outputs")?;
        let shape = out
            .get("shape")
            .and_then(|s| s.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_i64()).collect::<Vec<_>>())
            .ok_or("infer 响应缺少 shape")?;
        let data: Vec<f32> = out
            .get("data")
            .and_then(|d| d.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_f64().map(|x| x as f32)).collect())
            .ok_or("infer 响应缺少 data")?;
        // shape = [1, T, D]：对 token 维做均值池化得到句向量
        if shape.len() == 3 && shape[1] > 0 && shape[2] > 0 {
            let (t_len, d_len) = (shape[1] as usize, shape[2] as usize);
            if data.len() != t_len * d_len {
                return Err(format!("data 长度 {} 与 shape 不符", data.len()));
            }
            let mut pooled = vec![0f32; d_len];
            for t in 0..t_len {
                for d in 0..d_len {
                    pooled[d] += data[t * d_len + d];
                }
            }
            for p in pooled.iter_mut() {
                *p /= t_len as f32;
            }
            Ok((pooled, d_len))
        } else {
            Err(format!("意外的输出 shape: {shape:?}"))
        }
    }
}

impl Embedder for KylinTritonEmbedder {
    fn dim(&self) -> usize {
        self.dim_cache.load(Ordering::Relaxed)
    }
    fn embed(&self, texts: &[&str]) -> lymem_core::Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            let (vec, dim) = self.infer_raw(t).map_err(lymem_core::embedding::embed_err)?;
            self.dim_cache.store(dim, Ordering::Relaxed);
            out.push(vec);
        }
        Ok(out)
    }
}

// ---------------- 自动选择 ----------------

enum KylinChannel {
    Dbus(KylinDbusEmbedder, Option<KylinTritonEmbedder>),
    Triton(KylinTritonEmbedder),
}

/// 自动通道嵌入器：DBus 优先，kytensor 兜底；DBus 运行期出错自动降级。
/// 通道封装在 Mutex 中以支持运行期切换（自愈）。
pub struct KylinEmbedder {
    channel: std::sync::Mutex<KylinChannel>,
    dim_cache: AtomicUsize,
}

impl KylinEmbedder {
    /// 探测并连接可用通道。
    /// DBus 探测在独立线程中执行并施加硬超时：部分机型的 runtime 引擎
    /// 自启动起即卡死（调用无响应），zbus 对 p2p 连接无默认超时，
    /// 必须靠外部看门狗避免启动挂起。
    pub fn connect_auto() -> Result<Self, String> {
        let cfg = KylinConfig::default();
        let probe_cfg = cfg.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let r = KylinDbusEmbedder::connect(&probe_cfg).and_then(|d| match d.embed_one("lymem 连接探测") {
                Ok(_) => Ok(d),
                Err(e) => Err(format!("探测调用失败: {e}")),
            });
            let _ = tx.send(r);
        });
        match rx.recv_timeout(Duration::from_secs(8)) {
            Ok(Ok(d)) => {
                tracing::info!("麒麟嵌入：DBus SDK 通道就绪");
                Ok(KylinEmbedder::from_channel(KylinChannel::Dbus(d, None)))
            }
            Ok(Err(e)) => {
                tracing::warn!("DBus 通道不可用（{e}），切换 kytensor 直连");
                KylinTritonEmbedder::connect(&cfg).map(|t| KylinEmbedder::from_channel(KylinChannel::Triton(t)))
            }
            Err(_) => {
                tracing::warn!("DBus 通道探测超时（引擎可能卡死），切换 kytensor 直连；探测线程将随进程退出回收");
                KylinTritonEmbedder::connect(&cfg).map(|t| KylinEmbedder::from_channel(KylinChannel::Triton(t)))
            }
        }
    }

    pub fn which(&self) -> &'static str {
        match self.channel.lock().as_deref() {
            Ok(KylinChannel::Dbus(..)) => "kylin-dbus",
            Ok(KylinChannel::Triton(_)) => "kylin-kytensor",
            Err(_) => "kylin-unknown",
        }
    }
}

impl KylinEmbedder {
    fn from_channel(ch: KylinChannel) -> Self {
        KylinEmbedder {
            channel: std::sync::Mutex::new(ch),
            dim_cache: AtomicUsize::new(768),
        }
    }

    fn refresh_dim(&self, v: &[Vec<f32>]) {
        if let Some(first) = v.first() {
            if !first.is_empty() {
                self.dim_cache.store(first.len(), Ordering::Relaxed);
            }
        }
    }

    /// DBus 通道运行期连续出错时降级到 kytensor 直连（运行时引擎偶发崩溃/
    /// 后端拒连的自愈能力），降级动作只做一次。
    fn embed_with_failover(
        inner: &mut KylinChannel,
        dim_cache: &AtomicUsize,
        texts: &[&str],
    ) -> lymem_core::Result<Vec<Vec<f32>>> {
        let done = |v: Vec<Vec<f32>>, dim_cache: &AtomicUsize| {
            if let Some(first) = v.first() {
                if !first.is_empty() {
                    dim_cache.store(first.len(), Ordering::Relaxed);
                }
            }
            v
        };
        match inner {
            KylinChannel::Triton(t) => Ok(done(t.embed(texts)?, dim_cache)),
            KylinChannel::Dbus(d, fallback) => match d.embed(texts) {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::warn!("DBus 嵌入失败（{e}），重试一次");
                    if let Ok(v) = d.embed(texts) {
                        return Ok(v);
                    }
                    // 降级 kytensor（只构造一次）
                    let triton = match fallback.take() {
                        Some(t) => Some(t),
                        None => {
                            tracing::warn!("降级到 kytensor 直连通道");
                            KylinTritonEmbedder::connect(&KylinConfig::default()).ok()
                        }
                    };
                    match triton {
                        Some(t) => {
                            let res = t.embed(texts);
                            if res.is_ok() {
                                // 永久切换到 kytensor 通道
                                *inner = KylinChannel::Triton(t);
                            }
                            res
                        }
                        None => Err(lymem_core::embedding::embed_err(format!(
                            "DBus 失败且 kytensor 兜底不可用: {e}"
                        ))),
                    }
                }
            },
        }
    }
}

impl Embedder for KylinEmbedder {
    fn dim(&self) -> usize {
        self.dim_cache.load(Ordering::Relaxed)
    }
    fn embed(&self, texts: &[&str]) -> lymem_core::Result<Vec<Vec<f32>>> {
        let mut inner = self
            .channel
            .lock()
            .map_err(|_| lymem_core::embedding::embed_err("通道锁中毒"))?;
        Self::embed_with_failover(&mut inner, &self.dim_cache, texts)
    }
}
