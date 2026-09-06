//! Embedding 抽象与实现。
//!
//! [`Embedder`] trait 屏蔽来源差异；库级绑定（channel/model/dim）存于
//! `kb_knowledge_bases`（0002 字段），换绑即作废旧向量。
//! - [`ChannelEmbedder`]：走网关渠道（OpenAI 兼容 `POST {base}/embeddings`），
//!   复用 channels 表配置，与渠道/记账体系同源；
//! - [`DummyEmbedder`]：确定性伪向量（同文本同向量），离线开发与单测用，不烧钱。

use std::time::Duration;

use reqwest::Client;
use sqlx::SqlitePool;

use super::data::{self, KbBase};

/// 常见 embedding 模型 → 维度（渠道响应不带维度，绑定前必须确定）。
/// 顺序敏感：先匹配更具体的名字（bge-small-en 先于 bge-small）。
const KNOWN_MODELS: &[(&str, usize)] = &[
    ("text-embedding-3-large", 3072),
    ("text-embedding-3-small", 1536),
    ("text-embedding-ada-002", 1536),
    ("bge-m3", 1024),
    ("bge-large", 1024),
    ("bge-small-en", 384),
    ("bge-small", 512),
    ("m3e-large", 1024),
    ("m3e-base", 768),
    ("m3e-small", 512),
    ("jina-embeddings-v3", 1024),
];

/// 解析模型名 → 向量维度（大小写不敏感、子串匹配，兼容 "BAAI/bge-m3" 等厂商前缀）
pub fn resolve_embedding_dim(model: &str) -> Result<usize, String> {
    let m = model.trim().to_lowercase();
    for (key, dim) in KNOWN_MODELS {
        if m.contains(key) {
            return Ok(*dim);
        }
    }
    Err(format!(
        "无法确定模型 {model} 的向量维度；支持 OpenAI text-embedding 系列 / BGE / M3E / Jina（如 text-embedding-3-small、BAAI/bge-m3）"
    ))
}

/// 文本批量 → 向量（顺序对应）
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    fn dimensions(&self) -> usize;
    /// 人类可读描述（错误/日志用）
    fn describe(&self) -> String;
    /// 向量化一批文本；单个失败整体 Err
    async fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String>;
}

/// L2 归一化（cosine 语义下使距离可比；幂等，已归一化向量无副作用）
pub(crate) fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// 确定性伪向量：相同文本 → 相同归一化向量（离线开发/单测用，不依赖外呼渠道）
#[cfg(test)]
pub struct DummyEmbedder {
    pub dims: usize,
}

#[cfg(test)]
#[async_trait::async_trait]
impl Embedder for DummyEmbedder {
    fn dimensions(&self) -> usize {
        self.dims
    }
    fn describe(&self) -> String {
        format!("dummy({})", self.dims)
    }
    async fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        Ok(texts.iter().map(|t| dummy_vector(t, self.dims)).collect())
    }
}

/// 确定性伪向量生成（xorshift + DefaultHasher 固定种子；同文本必同向量）。
/// 仅测试使用：真实链路走 ChannelEmbedder。
#[cfg(test)]
pub(crate) fn dummy_vector(text: &str, dims: usize) -> Vec<f32> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    let mut seed = h.finish();
    let mut v = Vec::with_capacity(dims);
    for _ in 0..dims {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        v.push((seed as f64 / u64::MAX as f64) as f32 * 2.0 - 1.0);
    }
    normalize(v)
}

/// 渠道外呼实现：POST {base_url}/embeddings（OpenAI 兼容）
pub struct ChannelEmbedder {
    pub channel_id: String,
    base_url: String,
    api_key: String,
    pub model: String,
    pub dims: usize,
    client: Client,
}

impl ChannelEmbedder {
    /// 从 channels 表构造：渠道须存在且 active；模型维度须可解析
    pub async fn from_channel(
        pool: &SqlitePool,
        channel_id: &str,
        model: &str,
    ) -> Result<Self, String> {
        let row = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT type, base_url, api_key_encrypted, status FROM channels WHERE id = ?1",
        )
        .bind(channel_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
        let Some((_ctype, base_url, api_key, status)) = row else {
            return Err(format!("embedding 渠道不存在：{channel_id}"));
        };
        if status != "active" {
            return Err(format!("embedding 渠道已停用：{channel_id}"));
        }
        if base_url.trim().is_empty() || api_key.trim().is_empty() {
            return Err(format!("渠道 {channel_id} 缺少 base_url 或密钥"));
        }
        let dims = resolve_embedding_dim(model)?;
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?;
        Ok(Self {
            channel_id: channel_id.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model: model.trim().to_string(),
            dims,
            client,
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/embeddings", self.base_url)
    }
}

#[async_trait::async_trait]
impl Embedder for ChannelEmbedder {
    fn dimensions(&self) -> usize {
        self.dims
    }
    fn describe(&self) -> String {
        format!("渠道 {} · 模型 {}", self.channel_id, self.model)
    }
    async fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        let body = serde_json::json!({ "model": self.model, "input": texts });
        let resp = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("embedding 请求失败: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let clipped: String = text.chars().take(300).collect();
            return Err(format!("embedding 渠道返回 {status}: {clipped}"));
        }
        let parsed: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("embedding 响应解析失败: {e}"))?;
        let data = parsed
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or("embedding 响应缺少 data 数组")?;
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for item in data {
            let raw = item
                .get("embedding")
                .and_then(|v| v.as_array())
                .ok_or("embedding 响应项缺少 embedding 数组")?;
            let mut v = Vec::with_capacity(raw.len());
            for x in raw {
                v.push(x.as_f64().ok_or("embedding 向量含非数字")? as f32);
            }
            out.push(normalize(v));
        }
        if out.len() != texts.len() {
            return Err(format!(
                "embedding 返回数量不符：请求 {} 条，返回 {} 条",
                texts.len(),
                out.len()
            ));
        }
        Ok(out)
    }
}

// ---- 库级绑定管理（embedding_model / embedding_channel_id / embedding_dim）----

/// 绑定 embedding 渠道与模型。校验：渠道存在且 active、模型维度可解析。
/// 换绑语义：**作废该库全部旧向量**（清 chunks.embedding + 索引元数据），
/// 之后需重新构建索引（build_index 只 embed 缺向量块，幂等友好）。
pub async fn bind_embedding(
    pool: &SqlitePool,
    kb_id: &str,
    channel_id: &str,
    model: &str,
) -> Result<KbBase, String> {
    if data::read_base(pool, kb_id).await?.is_none() {
        return Err("知识库不存在".to_string());
    }
    // 顺带校验：渠道可用 + 模型维度可解析（from_channel 同套校验）
    let _probe = ChannelEmbedder::from_channel(pool, channel_id, model).await?;
    let dims = resolve_embedding_dim(model)?;
    let now = crate::commands::util::now_iso();
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query(
        "UPDATE kb_knowledge_bases SET embedding_model=?1, embedding_channel_id=?2, \
         embedding_dim=?3, index_status='none', updated_at=?4 WHERE id=?5",
    )
    .bind(model.trim())
    .bind(channel_id)
    .bind(dims as i64)
    .bind(&now)
    .bind(kb_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("绑定失败: {e}"))?;
    // 作废旧向量与索引记录
    sqlx::query(
        "UPDATE kb_chunks SET embedding = NULL WHERE document_id IN \
         (SELECT id FROM kb_documents WHERE kb_id = ?1)",
    )
    .bind(kb_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM kb_index_meta WHERE kb_id = ?1")
        .bind(kb_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(data::read_base(pool, kb_id).await?.expect("绑定后读取失败"))
}

/// 解除绑定：清空模型/渠道/维度并作废全部向量与索引
pub async fn unbind_embedding(pool: &SqlitePool, kb_id: &str) -> Result<KbBase, String> {
    if data::read_base(pool, kb_id).await?.is_none() {
        return Err("知识库不存在".to_string());
    }
    let now = crate::commands::util::now_iso();
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query(
        "UPDATE kb_knowledge_bases SET embedding_model=NULL, embedding_channel_id=NULL, \
         embedding_dim=0, index_status='none', updated_at=?1 WHERE id=?2",
    )
    .bind(&now)
    .bind(kb_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query(
        "UPDATE kb_chunks SET embedding = NULL WHERE document_id IN \
         (SELECT id FROM kb_documents WHERE kb_id = ?1)",
    )
    .bind(kb_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM kb_index_meta WHERE kb_id = ?1")
        .bind(kb_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(data::read_base(pool, kb_id).await?.expect("解绑后读取失败"))
}

/// 读取库的绑定（channel_id, model）；未绑定给出引导性错误
pub async fn bound_binding(pool: &SqlitePool, kb_id: &str) -> Result<(String, String), String> {
    let Some(base) = data::read_base(pool, kb_id).await? else {
        return Err("知识库不存在".to_string());
    };
    let channel_id = base.embedding_channel_id.ok_or_else(|| {
        "知识库尚未绑定 embedding 渠道，请先绑定（渠道 + embedding 模型）".to_string()
    })?;
    let model = base
        .embedding_model
        .ok_or("知识库缺少 embedding 模型配置，请重新绑定")?;
    Ok((channel_id, model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dims_resolution_matches_common_models() {
        assert_eq!(resolve_embedding_dim("text-embedding-3-small").unwrap(), 1536);
        assert_eq!(resolve_embedding_dim("text-embedding-3-large").unwrap(), 3072);
        assert_eq!(resolve_embedding_dim("BAAI/bge-m3").unwrap(), 1024);
        assert_eq!(resolve_embedding_dim("bge-large-zh-v1.5").unwrap(), 1024);
        assert_eq!(resolve_embedding_dim("bge-small-en-v1.5").unwrap(), 384);
        assert_eq!(resolve_embedding_dim("bge-small-zh-v1.5").unwrap(), 512);
        assert_eq!(resolve_embedding_dim("m3e-base").unwrap(), 768);
        assert!(resolve_embedding_dim("unknown-model-x").is_err());
    }

    #[tokio::test]
    async fn dummy_is_deterministic_and_normalized() {
        let d = DummyEmbedder { dims: 64 };
        let a = d.embed_texts(&["同一句话", "另一句"]).await.unwrap();
        let b = d.embed_texts(&["同一句话", "第三句"]).await.unwrap();
        assert_eq!(a[0], b[0], "相同文本应产出相同向量");
        assert_eq!(a.len(), 2);
        for v in a.iter().chain(b.iter()) {
            assert_eq!(v.len(), 64);
            let norm: f32 = v.iter().map(|x| x * x).sum();
            assert!((norm - 1.0).abs() < 1e-3, "向量应已归一化: {norm}");
        }
    }
}
