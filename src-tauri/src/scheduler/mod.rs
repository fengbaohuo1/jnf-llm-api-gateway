//! 故障转移调度器：当多个渠道都能服务同一个模型时，构建"有序的故障转移队列"。
//!
//! 设计取向（个人网关）：
//! - **没有故障就不要负载均衡**：正常请求固定走队列第一个（主渠道），
//!   前缀缓存 100% 命中、账单完全可预期。
//! - **负载均衡只用于故障转移**：主渠道失败时，按队列顺序尝试备用渠道。
//! - 队列排序：① 过滤（启用 + 模型匹配）→ ② priority 降序 → ③ 同优先级组内按 weight 权重随机
//! - 权重随机是"无放回轮盘赌"：权重越大越可能排前，但每次结果带随机性，避免固定渠道被压垮
//!
//! 调用方：转发代理（proxy 模块）在每次请求路由时调用 `select_channels`。

use rand::seq::SliceRandom;
use rand::Rng;
use sqlx::SqlitePool;

/// 候选渠道（完整配置 + 排序元数据）
#[derive(Clone)]
pub struct ChannelChoice {
    pub id: String,
    pub name: String,
    /// 渠道类型（品牌：OpenAI/DeepSeek/Claude/Gemini/Custom）
    #[allow(dead_code)]
    pub channel_type: String,
    /// 生效协议（显式 protocol；为空时按渠道类型推导）：
    /// openai / deepseek / anthropic / gemini
    pub protocol: String,
    /// 渠道级 DeepSeek 思考模式兼容：转发时给 tool_calls 轮次补空 reasoning_content
    pub compat_reasoning_content: bool,
    pub base_url: String,
    pub api_key: String,
    /// 渠道配置的模型（转发时使用请求的 model；本字段保留用于日志/展示）
    #[allow(dead_code)]
    pub model: String,
    pub priority: i64,
    pub weight: i64,
    /// 每 1k token 成本单价（元），转发层记账用
    pub price_per_1k_tokens: f64,
    /// 每 1k token 缓存命中单价（元），0 = 未配置（按正常单价计费）
    pub cache_price_per_1k_tokens: f64,
    /// 每 1k token 思考（reasoning）单价（元），0 = 未配置（按正常输出单价计费）
    pub reasoning_price_per_1k_tokens: f64,
    /// 按模型定价（模型名 → 价格）；空表 = 用渠道默认单价
    pub model_prices: std::collections::BTreeMap<String, crate::models::ModelPrice>,
}

/// 构建有序故障转移队列：
/// 1. 过滤：status = active 且（模型列表为空 = 任意模型，或请求 model 在渠道模型列表中）；
///    渠道模型列表优先读 `channels.models`（JSON 数组），为空回退旧单模型字段
/// 2. 按 priority 降序排序（严格优先级：高优先级组整体在前）
/// 3. 同优先级组内按 weight 做无放回权重随机（轮盘赌）
pub async fn select_channels(
    pool: &SqlitePool,
    requested_model: &str,
) -> Result<Vec<ChannelChoice>, String> {
    let rows: Vec<(String, String, String, String, i64, String, String, String, String, i64, i64, f64, f64, f64, String)> = sqlx::query_as(
        "SELECT id, name, type, protocol, compat_reasoning_content, base_url, api_key_encrypted, model, models, priority, weight, price_per_1k_tokens, cache_price_per_1k_tokens, reasoning_price_per_1k_tokens, model_prices FROM channels
         WHERE status = 'active'
         ORDER BY priority DESC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let candidates: Vec<ChannelChoice> = rows
        .into_iter()
        .filter(|r| crate::models::channel_serves_model(&r.8, &r.7, requested_model))
        .map(|(id, name, channel_type, protocol, compat_reasoning_content, base_url, api_key, model, _models, priority, weight, price_per_1k_tokens, cache_price_per_1k_tokens, reasoning_price_per_1k_tokens, model_prices)| ChannelChoice {
            id,
            name,
            protocol: crate::adapters::effective_protocol(&channel_type, &protocol),
            compat_reasoning_content: compat_reasoning_content != 0,
            channel_type,
            base_url,
            api_key,
            model,
            priority,
            weight,
            price_per_1k_tokens,
            cache_price_per_1k_tokens,
            reasoning_price_per_1k_tokens,
            model_prices: crate::models::parse_model_prices(&model_prices),
        })
        .collect();

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // 同优先级分组，组内做无放回权重随机
    let mut ordered = Vec::with_capacity(candidates.len());
    let mut start = 0;
    while start < candidates.len() {
        let priority = candidates[start].priority;
        let mut end = start;
        while end < candidates.len() && candidates[end].priority == priority {
            end += 1;
        }

        let mut group = candidates[start..end].to_vec();
        weighted_shuffle_in_place(&mut group);
        ordered.append(&mut group);

        start = end;
    }

    Ok(ordered)
}

/// 无放回权重随机（轮盘赌）：按 weight 概率依次抽出渠道，权重越大越先被抽出。
fn weighted_shuffle_in_place(group: &mut Vec<ChannelChoice>) {
    let mut rng = rand::rng();
    // 预先打乱，避免"同权重"时总是固定顺序
    group.shuffle(&mut rng);

    let mut result = Vec::with_capacity(group.len());
    while !group.is_empty() {
        let total_weight: i64 = group.iter().map(|c| c.weight.max(0)).sum();
        let index = if total_weight > 0 {
            let mut point = rng.random_range(0..total_weight);
            let mut selected = 0;
            for (idx, channel) in group.iter().enumerate() {
                point -= channel.weight.max(0);
                if point < 0 {
                    selected = idx;
                    break;
                }
            }
            selected
        } else {
            0 // 全是 0 权重时取当前顺序第一个
        };
        result.push(group.remove(index));
    }
    *group = result;
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE channels (
                id TEXT PRIMARY KEY, name TEXT, type TEXT, protocol TEXT NOT NULL DEFAULT '',
                compat_reasoning_content INTEGER NOT NULL DEFAULT 1,
                base_url TEXT,
                model TEXT, models TEXT NOT NULL DEFAULT '', status TEXT, api_key_encrypted TEXT,
                latency_ms INTEGER, priority INTEGER, weight INTEGER,
                price_per_1k_tokens REAL DEFAULT 0,
                cache_price_per_1k_tokens REAL DEFAULT 0,
                reasoning_price_per_1k_tokens REAL DEFAULT 0,
                model_prices TEXT NOT NULL DEFAULT '{}',
                created_at TEXT, updated_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_channel_with_type(
        pool: &SqlitePool,
        id: &str,
        channel_type: &str,
        protocol: &str,
        model: &str,
        status: &str,
        priority: i64,
        weight: i64,
    ) {
        sqlx::query(
            "INSERT INTO channels (id, name, type, protocol, base_url, model, status, api_key_encrypted, priority, weight, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'http://x', ?5, ?6, 'k', ?7, ?8, '2025-01-01 00:00:00', '2025-01-01 00:00:00')",
        )
        .bind(id)
        .bind(id)
        .bind(channel_type)
        .bind(protocol)
        .bind(model)
        .bind(status)
        .bind(priority)
        .bind(weight)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_channel(
        pool: &SqlitePool,
        id: &str,
        model: &str,
        status: &str,
        priority: i64,
        weight: i64,
    ) {
        insert_channel_with_type(pool, id, "OpenAI", "", model, status, priority, weight).await;
    }

    #[tokio::test]
    async fn effective_protocol_derived_from_type_or_explicit() {
        let pool = test_pool().await;
        insert_channel_with_type(&pool, "ds", "DeepSeek", "", "deepseek-flash", "active", 0, 1).await;
        insert_channel_with_type(&pool, "custom-ds", "Custom", "deepseek", "deepseek-chat", "active", 0, 1).await;
        insert_channel_with_type(&pool, "plain", "OpenAI", "", "gpt-4o", "active", 0, 1).await;

        let queue = select_channels(&pool, "deepseek-flash").await.unwrap();
        assert_eq!(queue[0].protocol, "deepseek");

        let queue = select_channels(&pool, "deepseek-chat").await.unwrap();
        assert_eq!(queue[0].protocol, "deepseek");
        assert!(queue[0].compat_reasoning_content, "默认开启思考兼容");

        // 渠道级开关关闭后队列中读到 false
        sqlx::query("UPDATE channels SET compat_reasoning_content = 0 WHERE id = 'custom-ds'")
            .execute(&pool)
            .await
            .unwrap();
        let queue = select_channels(&pool, "deepseek-chat").await.unwrap();
        assert!(!queue[0].compat_reasoning_content);

        let queue = select_channels(&pool, "gpt-4o").await.unwrap();
        assert_eq!(queue[0].protocol, "openai");
    }

    #[tokio::test]
    async fn multi_model_channel_matches_each_listed_model() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO channels (id, name, type, protocol, base_url, model, models, status, api_key_encrypted, priority, weight, created_at, updated_at)
             VALUES ('multi', 'multi', 'DeepSeek', '', 'http://x', 'deepseek-chat', '[\"deepseek-chat\",\"deepseek-reasoner\"]', 'active', 'k', 0, 1, '2025-01-01 00:00:00', '2025-01-01 00:00:00')",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(select_channels(&pool, "deepseek-chat").await.unwrap().len(), 1);
        assert_eq!(select_channels(&pool, "deepseek-reasoner").await.unwrap().len(), 1);
        assert!(select_channels(&pool, "gpt-4o").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn filters_disabled_and_model_mismatch() {
        let pool = test_pool().await;
        insert_channel(&pool, "c1", "gpt-4o", "active", 0, 1).await;
        insert_channel(&pool, "c2", "gpt-4o", "disabled", 0, 1).await;
        insert_channel(&pool, "c3", "other-model", "active", 0, 1).await;

        let queue = select_channels(&pool, "gpt-4o").await.unwrap();
        let ids: Vec<&str> = queue.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["c1"]);

        let empty = select_channels(&pool, "nope").await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn empty_model_serves_any() {
        let pool = test_pool().await;
        insert_channel(&pool, "c1", "", "active", 0, 1).await;

        let queue = select_channels(&pool, "anything").await.unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].id, "c1");
    }

    #[tokio::test]
    async fn priority_desc_order() {
        let pool = test_pool().await;
        insert_channel(&pool, "low", "gpt-4o", "active", 1, 1).await;
        insert_channel(&pool, "high", "gpt-4o", "active", 10, 1).await;
        insert_channel(&pool, "mid", "gpt-4o", "active", 5, 1).await;

        let queue = select_channels(&pool, "gpt-4o").await.unwrap();
        let ids: Vec<&str> = queue.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["high", "mid", "low"]);
    }

    #[tokio::test]
    async fn same_priority_keeps_full_set() {
        let pool = test_pool().await;
        for i in 0..10 {
            insert_channel(&pool, &format!("c{i}"), "gpt-4o", "active", 0, 1).await;
        }

        let queue = select_channels(&pool, "gpt-4o").await.unwrap();
        assert_eq!(queue.len(), 10);
        let mut ids: Vec<&str> = queue.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, (0..10).map(|i| format!("c{i}")).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn high_weight_tends_to_front() {
        let pool = test_pool().await;
        insert_channel(&pool, "heavy", "gpt-4o", "active", 0, 100).await;
        insert_channel(&pool, "light", "gpt-4o", "active", 0, 1).await;

        // 权重 100:1，跑 30 次，heavy 绝大多数情况排第一
        let mut heavy_first = 0;
        for _ in 0..30 {
            let queue = select_channels(&pool, "gpt-4o").await.unwrap();
            if queue[0].id == "heavy" {
                heavy_first += 1;
            }
        }
        assert!(heavy_first >= 25, "heavy first only {heavy_first}/30");
    }
}
