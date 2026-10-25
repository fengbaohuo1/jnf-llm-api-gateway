use tauri::State;

use crate::commands::DbState;
use crate::models::Channel;

use super::util::{mask, new_id, now_iso, record_admin_audit};
use super::pool;

#[tauri::command]
pub async fn list_channels(state: State<'_, DbState>) -> Result<Vec<Channel>, String> {
    let rows = sqlx::query_as::<_, (String, String, String, String, String, String, String, String, Option<i64>, i64, i64, f64, f64, f64, String)>(
        "SELECT id, name, type, protocol, base_url, model, status, api_key_encrypted, latency_ms, priority, weight, price_per_1k_tokens, cache_price_per_1k_tokens, reasoning_price_per_1k_tokens, created_at FROM channels ORDER BY created_at DESC",
    )
    .fetch_all(pool(&state))
    .await
    .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .map(|r| Channel {
            id: r.0,
            name: r.1,
            type_: r.2,
            protocol: r.3,
            base_url: r.4,
            model: r.5,
            status: r.6,
            api_key_masked: mask(&r.7),
            latency_ms: r.8,
            priority: r.9,
            weight: r.10,
            price_per_1k_tokens: r.11,
            cache_price_per_1k_tokens: r.12,
            reasoning_price_per_1k_tokens: r.13,
            created_at: r.14,
        })
        .collect())
}

#[tauri::command]
pub async fn create_channel(
    state: State<'_, DbState>,
    name: String,
    type_: String,
    protocol: String,
    base_url: String,
    model: String,
    api_key: String,
    status: String,
    priority: i64,
    weight: i64,
    price_per_1k_tokens: f64,
    cache_price_per_1k_tokens: f64,
    reasoning_price_per_1k_tokens: f64,
) -> Result<Channel, String> {
    let id = new_id("ch");
    let created = now_iso();
    sqlx::query(
        "INSERT INTO channels (id, name, type, protocol, base_url, model, status, api_key_encrypted, latency_ms, priority, weight, price_per_1k_tokens, cache_price_per_1k_tokens, reasoning_price_per_1k_tokens, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,NULL,?9,?10,?11,?12,?13,?14,?14)",
    )
    .bind(&id).bind(&name).bind(&type_).bind(&protocol).bind(&base_url).bind(&model).bind(&status).bind(&api_key).bind(priority).bind(weight).bind(price_per_1k_tokens).bind(cache_price_per_1k_tokens).bind(reasoning_price_per_1k_tokens).bind(&created)
    .execute(pool(&state))
    .await
    .map_err(|e| e.to_string())?;
    record_admin_audit(pool(&state), "create", "channel", Some(&id), Some(&name)).await;

    Ok(Channel {
        id,
        name,
        type_,
        protocol,
        base_url,
        model,
        status,
        api_key_masked: mask(&api_key),
        latency_ms: None,
        priority,
        weight,
        price_per_1k_tokens,
        cache_price_per_1k_tokens,
        reasoning_price_per_1k_tokens,
        created_at: created,
    })
}

#[tauri::command]
pub async fn update_channel(
    state: State<'_, DbState>,
    id: String,
    name: String,
    type_: String,
    protocol: String,
    base_url: String,
    model: String,
    status: String,
    api_key: String,
    priority: i64,
    weight: i64,
    price_per_1k_tokens: f64,
    cache_price_per_1k_tokens: f64,
    reasoning_price_per_1k_tokens: f64,
) -> Result<(), String> {
    // 前端编辑时提交完整表单，直接全量更新（api_key 传空字符串表示不改动密钥）。
    sqlx::query(
        "UPDATE channels SET name = ?2, type = ?3, protocol = ?4, base_url = ?5, model = ?6, status = ?7,
                api_key_encrypted = CASE WHEN ?8 = '' THEN api_key_encrypted ELSE ?8 END,
                priority = ?9, weight = ?10, price_per_1k_tokens = ?11, cache_price_per_1k_tokens = ?12, reasoning_price_per_1k_tokens = ?13, updated_at = ?14
         WHERE id = ?1",
    )
    .bind(id.as_str())
    .bind(name.as_str())
    .bind(type_)
    .bind(protocol)
    .bind(base_url)
    .bind(model)
    .bind(status)
    .bind(api_key)
    .bind(priority)
    .bind(weight)
    .bind(price_per_1k_tokens)
    .bind(cache_price_per_1k_tokens)
    .bind(reasoning_price_per_1k_tokens)
    .bind(now_iso())
    .execute(pool(&state))
    .await
    .map_err(|e| e.to_string())?;
    record_admin_audit(pool(&state), "update", "channel", Some(&id), Some(&name)).await;
    Ok(())
}

#[tauri::command]
pub async fn delete_channel(state: State<'_, DbState>, id: String) -> Result<(), String> {
    let db = pool(&state);
    // requests.channel_id 带外键且历史日志需要保留：先把引用置空再删除（channel_name 冗余保留）
    let mut tx = db.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("UPDATE requests SET channel_id = NULL WHERE channel_id = ?1")
        .bind(&id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM channels WHERE id = ?1")
        .bind(&id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    record_admin_audit(db, "delete", "channel", Some(&id), None).await;
    Ok(())
}

#[tauri::command]
pub async fn toggle_channel_status(state: State<'_, DbState>, id: String) -> Result<(), String> {
    sqlx::query("UPDATE channels SET status = CASE WHEN status = 'active' THEN 'disabled' ELSE 'active' END, updated_at = ?2 WHERE id = ?1")
        .bind(&id)
        .bind(now_iso())
        .execute(pool(&state))
        .await
        .map_err(|e| e.to_string())?;
    record_admin_audit(pool(&state), "toggle", "channel", Some(&id), None).await;
    Ok(())
}

#[tauri::command]
pub async fn test_channel(state: State<'_, DbState>, id: String) -> Result<serde_json::Value, String> {
    // 1. 从数据库读取渠道配置（含显式协议；为空时按渠道类型推导）
    let row: Option<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT type, protocol, base_url, api_key_encrypted, model FROM channels WHERE id = ?1",
    )
    .bind(&id)
    .fetch_optional(pool(&state))
    .await
    .map_err(|e| e.to_string())?;

    let Some((ctype, protocol, base_url, api_key, model)) = row else {
        return Err("渠道不存在".into());
    };
    let effective = crate::adapters::effective_protocol(&ctype, &protocol);

    // 2. 创建适配器并真实测试（必须使用渠道配置的模型，否则会拿厂商默认模型测出 400）
    let adapter = crate::adapters::create_adapter(&effective, &base_url, &api_key)
        .map_err(|e| e.to_string())?;
    // 失败不再抛长错误串，改为结构化返回：ok=false + 简短错误码 + 完整 message
    let result = match adapter.test(Some(&model)).await {
        Ok(r) => r,
        Err(e) => {
            let code = adapter_error_code(&e);
            let message = e.to_string();
            sqlx::query("UPDATE channels SET status = 'error', latency_ms = NULL, updated_at = ?2 WHERE id = ?1")
                .bind(&id)
                .bind(now_iso())
                .execute(pool(&state))
                .await
                .map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({
                "ok": false,
                "latencyMs": null,
                "message": message,
                "code": code,
                "model": model,
                "protocol": effective,
            }));
        }
    };

    // 3. 更新渠道状态与延迟
    if result.ok {
        let latency: Option<i64> = result.latency_ms.map(|v| v as i64);
        sqlx::query("UPDATE channels SET status = 'active', latency_ms = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(&id)
            .bind(latency)
            .bind(now_iso())
            .execute(pool(&state))
            .await
            .map_err(|e| e.to_string())?;
    } else {
        sqlx::query("UPDATE channels SET status = 'error', latency_ms = NULL, updated_at = ?2 WHERE id = ?1")
            .bind(&id)
            .bind(now_iso())
            .execute(pool(&state))
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(serde_json::json!({
        "ok": result.ok,
        "latencyMs": result.latency_ms,
        "message": result.message,
        "code": null,
        "model": model,
        "protocol": effective,
    }))
}

/// 把适配器错误映射为简短展示码（HTTP 状态码 / 网络 / 解析 / 配置）
fn adapter_error_code(e: &crate::adapters::AdapterError) -> String {
    match e {
        crate::adapters::AdapterError::Upstream { status, .. } => format!("HTTP {status}"),
        crate::adapters::AdapterError::Http(err) => {
            if err.is_timeout() {
                "超时".into()
            } else if err.is_connect() {
                "网络错误".into()
            } else {
                "请求失败".into()
            }
        }
        crate::adapters::AdapterError::Parse(_) => "解析失败".into(),
        crate::adapters::AdapterError::Config(_) => "配置错误".into(),
    }
}

// ---------------- 模型列表拉取（添加渠道时选模型用） ----------------

/// 上游 `GET /models` 的候选地址：base 已带 `/v1` 时只试 `{base}/models`；
/// 否则依次试 `{base}/models`、`{base}/v1/models`（兼容用户填域名或填到 /v1 的两种习惯）。
pub(crate) fn models_url_candidates(base: &str) -> Vec<String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }
    if base.ends_with("/v1") {
        vec![format!("{base}/models")]
    } else {
        vec![format!("{base}/models"), format!("{base}/v1/models")]
    }
}

/// 从表单或现有渠道解析 Base URL / API Key（编辑时前端不回填 Key，需按 channel_id 读取）
async fn resolve_channel_inputs(
    pool: &sqlx::SqlitePool,
    base_url: Option<String>,
    api_key: Option<String>,
    channel_id: Option<String>,
) -> Result<(String, String), String> {
    let mut base = base_url.unwrap_or_default().trim().trim_end_matches('/').to_string();
    let mut key = api_key.unwrap_or_default().trim().to_string();
    if base.is_empty() || key.is_empty() {
        if let Some(id) = channel_id {
            let row: Option<(String, String)> = sqlx::query_as(
                "SELECT base_url, api_key_encrypted FROM channels WHERE id = ?1",
            )
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
            if let Some((b, k)) = row {
                if base.is_empty() {
                    base = b.trim().trim_end_matches('/').to_string();
                }
                if key.is_empty() {
                    key = k.trim().to_string();
                }
            }
        }
    }
    if base.is_empty() {
        return Err("请先填写 Base URL".into());
    }
    if key.is_empty() {
        return Err("请先填写 API Key".into());
    }
    Ok((base, key))
}

/// 请求一个 /models 地址并解析 `data[].id`
async fn fetch_models_at(
    client: &reqwest::Client,
    url: &str,
    key: &str,
) -> Result<Vec<String>, String> {
    let resp = client
        .get(url)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "请求超时".to_string()
            } else if e.is_connect() {
                "网络错误".to_string()
            } else {
                format!("请求失败: {e}")
            }
        })?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let snippet: String = body.trim().chars().take(160).collect();
        return Err(format!("HTTP {} — {}", status.as_u16(), snippet));
    }
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| "响应不是合法 JSON".to_string())?;
    let mut models: Vec<String> = v
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(|s| s.to_string()))
                .take(200)
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err("上游未返回任何模型（可手动填写模型 ID）".to_string());
    }
    Ok(models)
}

/// 拉取上游模型列表（添加/编辑渠道时选择模型，避免手敲模型 ID）。
/// base_url / api_key 留空时按 channel_id 从已保存渠道读取。
#[tauri::command]
pub async fn fetch_channel_models(
    state: State<'_, DbState>,
    base_url: Option<String>,
    api_key: Option<String>,
    channel_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let (base, key) = resolve_channel_inputs(pool(&state), base_url, api_key, channel_id).await?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    let mut last_error = "未尝试任何地址".to_string();
    for url in models_url_candidates(&base) {
        match fetch_models_at(&client, &url, &key).await {
            Ok(models) => {
                return Ok(serde_json::json!({ "models": models, "url": url }));
            }
            Err(e) => last_error = format!("{url} → {e}"),
        }
    }
    Err(format!("获取模型列表失败：{last_error}"))
}

#[cfg(test)]
mod tests {
    use super::models_url_candidates;

    #[test]
    fn models_url_handles_v1_suffix() {
        assert_eq!(
            models_url_candidates("https://api.openai.com/v1"),
            vec!["https://api.openai.com/v1/models"]
        );
        assert_eq!(
            models_url_candidates("https://api.deepseek.com"),
            vec![
                "https://api.deepseek.com/models",
                "https://api.deepseek.com/v1/models"
            ]
        );
        assert!(models_url_candidates("   ").is_empty());
    }
}
