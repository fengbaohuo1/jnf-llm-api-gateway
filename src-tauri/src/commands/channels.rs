use tauri::State;

use crate::commands::DbState;
use crate::models::Channel;

use super::util::{mask, new_id, now_iso, record_admin_audit};
use super::pool;

/// channels 表整行（字段较多，用结构体而非元组；sqlx 元组实现上限 16）
#[derive(sqlx::FromRow)]
struct ChannelRow {
    id: String,
    name: String,
    r#type: String,
    protocol: String,
    compat_reasoning_content: i64,
    base_url: String,
    model: String,
    models: String,
    status: String,
    api_key_encrypted: String,
    latency_ms: Option<i64>,
    priority: i64,
    weight: i64,
    price_per_1k_tokens: f64,
    cache_price_per_1k_tokens: f64,
    reasoning_price_per_1k_tokens: f64,
    created_at: String,
}

#[tauri::command]
pub async fn list_channels(state: State<'_, DbState>) -> Result<Vec<Channel>, String> {
    let rows: Vec<ChannelRow> = sqlx::query_as(
        "SELECT id, name, type, protocol, compat_reasoning_content, base_url, model, models, status, api_key_encrypted, latency_ms, priority, weight, price_per_1k_tokens, cache_price_per_1k_tokens, reasoning_price_per_1k_tokens, created_at FROM channels ORDER BY created_at DESC",
    )
    .fetch_all(pool(&state))
    .await
    .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .map(|r| Channel {
            models: crate::models::channel_models(&r.models, &r.model),
            id: r.id,
            name: r.name,
            type_: r.r#type,
            protocol: r.protocol,
            compat_reasoning_content: r.compat_reasoning_content != 0,
            base_url: r.base_url,
            model: r.model,
            status: r.status,
            api_key_masked: mask(&r.api_key_encrypted),
            latency_ms: r.latency_ms,
            priority: r.priority,
            weight: r.weight,
            price_per_1k_tokens: r.price_per_1k_tokens,
            cache_price_per_1k_tokens: r.cache_price_per_1k_tokens,
            reasoning_price_per_1k_tokens: r.reasoning_price_per_1k_tokens,
            created_at: r.created_at,
        })
        .collect())
}

#[tauri::command]
pub async fn create_channel(
    state: State<'_, DbState>,
    name: String,
    type_: String,
    protocol: String,
    compat_reasoning_content: bool,
    base_url: String,
    models: Vec<String>,
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
    let models = crate::models::normalize_models(&models);
    let primary_model = models.first().cloned().unwrap_or_default();
    let models_json = serde_json::to_string(&models).unwrap_or_else(|_| "[]".into());
    sqlx::query(
        "INSERT INTO channels (id, name, type, protocol, compat_reasoning_content, base_url, model, models, status, api_key_encrypted, latency_ms, priority, weight, price_per_1k_tokens, cache_price_per_1k_tokens, reasoning_price_per_1k_tokens, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,NULL,?11,?12,?13,?14,?15,?16,?16)",
    )
    .bind(&id).bind(&name).bind(&type_).bind(&protocol).bind(compat_reasoning_content as i64).bind(&base_url).bind(&primary_model).bind(&models_json).bind(&status).bind(&api_key).bind(priority).bind(weight).bind(price_per_1k_tokens).bind(cache_price_per_1k_tokens).bind(reasoning_price_per_1k_tokens).bind(&created)
    .execute(pool(&state))
    .await
    .map_err(|e| e.to_string())?;
    record_admin_audit(pool(&state), "create", "channel", Some(&id), Some(&name)).await;

    Ok(Channel {
        id,
        name,
        type_,
        protocol,
        compat_reasoning_content,
        base_url,
        model: primary_model,
        models,
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
    compat_reasoning_content: bool,
    base_url: String,
    models: Vec<String>,
    status: String,
    api_key: String,
    priority: i64,
    weight: i64,
    price_per_1k_tokens: f64,
    cache_price_per_1k_tokens: f64,
    reasoning_price_per_1k_tokens: f64,
) -> Result<(), String> {
    // 前端编辑时提交完整表单，直接全量更新（api_key 传空字符串表示不改动密钥）。
    let models = crate::models::normalize_models(&models);
    let primary_model = models.first().cloned().unwrap_or_default();
    let models_json = serde_json::to_string(&models).unwrap_or_else(|_| "[]".into());
    sqlx::query(
        "UPDATE channels SET name = ?2, type = ?3, protocol = ?4, compat_reasoning_content = ?5, base_url = ?6, model = ?7, models = ?8, status = ?9,
                api_key_encrypted = CASE WHEN ?10 = '' THEN api_key_encrypted ELSE ?10 END,
                priority = ?11, weight = ?12, price_per_1k_tokens = ?13, cache_price_per_1k_tokens = ?14, reasoning_price_per_1k_tokens = ?15, updated_at = ?16
         WHERE id = ?1",
    )
    .bind(id.as_str())
    .bind(name.as_str())
    .bind(type_)
    .bind(protocol)
    .bind(compat_reasoning_content as i64)
    .bind(base_url)
    .bind(primary_model)
    .bind(models_json)
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
    let row: Option<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT type, protocol, base_url, api_key_encrypted, model, models FROM channels WHERE id = ?1",
    )
    .bind(&id)
    .fetch_optional(pool(&state))
    .await
    .map_err(|e| e.to_string())?;

    let Some((ctype, protocol, base_url, api_key, model, models)) = row else {
        return Err("渠道不存在".into());
    };
    let effective = crate::adapters::effective_protocol(&ctype, &protocol);
    // 多模型渠道用第一个模型做连通性测试；未配置模型则回退厂商默认模型
    let test_model = crate::models::channel_models(&models, &model)
        .into_iter()
        .next()
        .unwrap_or_default();

    // 2. 创建适配器并真实测试（必须使用渠道配置的模型，否则会拿厂商默认模型测出 400）
    let adapter = crate::adapters::create_adapter(&effective, &base_url, &api_key)
        .map_err(|e| e.to_string())?;
    let test_model_arg = if test_model.is_empty() {
        None
    } else {
        Some(test_model.as_str())
    };
    // 失败不再抛长错误串，改为结构化返回：ok=false + 简短错误码 + 完整 message
    let result = match adapter.test(test_model_arg).await {
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
                "model": test_model,
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
        "model": test_model,
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

// ---------------- 渠道余额查询（目前仅 DeepSeek） ----------------

/// DeepSeek 余额接口候选地址：官方为 `https://api.deepseek.com/user/balance`。
/// 用户可能把 Base URL 填成 `.../v1`（OpenAI 习惯），先尝试去掉 `/v1` 的根地址。
pub(crate) fn deepseek_balance_url_candidates(base: &str) -> Vec<String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }
    let root = base
        .strip_suffix("/v1")
        .map(|s| s.trim_end_matches('/'))
        .filter(|s| !s.is_empty())
        .unwrap_or(base);
    let mut urls = vec![format!("{root}/user/balance")];
    if root != base {
        urls.push(format!("{base}/user/balance"));
    }
    urls
}

/// 解析 DeepSeek `/user/balance` 响应：优先取 CNY，没有则取第一条；
/// 金额字段是字符串（如 "110.00"），统一转成数字给前端。
pub(crate) fn parse_deepseek_balance(body: &str) -> Result<serde_json::Value, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "响应不是合法 JSON".to_string())?;
    let infos = v
        .get("balance_infos")
        .and_then(|b| b.as_array())
        .ok_or_else(|| {
            let msg = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("响应缺少 balance_infos 字段");
            format!("解析余额失败：{msg}")
        })?;
    let Some(first) = infos.first() else {
        return Err("上游未返回余额信息".into());
    };
    let pick = infos
        .iter()
        .find(|i| i.get("currency").and_then(|c| c.as_str()) == Some("CNY"))
        .unwrap_or(first);
    let num = |k: &str| {
        pick.get(k)
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    Ok(serde_json::json!({
        "supported": true,
        "available": v.get("is_available").and_then(|a| a.as_bool()).unwrap_or(false),
        "currency": pick.get("currency").and_then(|c| c.as_str()).unwrap_or("CNY"),
        "totalBalance": num("total_balance"),
        "grantedBalance": num("granted_balance"),
        "toppedUpBalance": num("topped_up_balance"),
        "fetchedAt": now_iso(),
    }))
}

/// 查询渠道余额（目前适配 DeepSeek）：其他协议返回 `{ supported: false }`，由前端隐藏余额区。
#[tauri::command]
pub async fn fetch_channel_balance(
    state: State<'_, DbState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT type, protocol, base_url, api_key_encrypted FROM channels WHERE id = ?1",
    )
    .bind(&id)
    .fetch_optional(pool(&state))
    .await
    .map_err(|e| e.to_string())?;
    let Some((ctype, protocol, base_url, api_key)) = row else {
        return Err("渠道不存在".into());
    };
    let effective = crate::adapters::effective_protocol(&ctype, &protocol);
    if effective != "deepseek" {
        return Ok(serde_json::json!({ "supported": false }));
    }
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return Err("渠道未配置 API Key，无法查询余额".into());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    let mut last_error = String::new();
    for url in deepseek_balance_url_candidates(&base_url) {
        match client.get(&url).bearer_auth(&key).send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    let snippet: String = body.trim().chars().take(200).collect();
                    last_error = format!("HTTP {} — {}", status.as_u16(), snippet);
                    continue;
                }
                return parse_deepseek_balance(&body);
            }
            Err(e) => {
                last_error = if e.is_timeout() {
                    "请求超时".to_string()
                } else if e.is_connect() {
                    "网络错误".to_string()
                } else {
                    format!("请求失败: {e}")
                };
            }
        }
    }
    Err(format!("查询余额失败：{last_error}"))
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
    use super::{
        deepseek_balance_url_candidates, models_url_candidates, parse_deepseek_balance,
    };

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

    #[test]
    fn balance_urls_strip_v1_suffix() {
        assert_eq!(
            deepseek_balance_url_candidates("https://api.deepseek.com"),
            vec!["https://api.deepseek.com/user/balance"]
        );
        assert_eq!(
            deepseek_balance_url_candidates("https://api.deepseek.com/v1/"),
            vec![
                "https://api.deepseek.com/user/balance",
                "https://api.deepseek.com/v1/user/balance"
            ]
        );
        assert!(deepseek_balance_url_candidates("  ").is_empty());
    }

    #[test]
    fn parse_balance_prefers_cny() {
        let body = r#"{
            "is_available": true,
            "balance_infos": [
                {"currency": "USD", "total_balance": "1.50", "granted_balance": "0.00", "topped_up_balance": "1.50"},
                {"currency": "CNY", "total_balance": "110.00", "granted_balance": "10.00", "topped_up_balance": "100.00"}
            ]
        }"#;
        let v = parse_deepseek_balance(body).unwrap();
        assert_eq!(v["supported"], true);
        assert_eq!(v["available"], true);
        assert_eq!(v["currency"], "CNY");
        assert_eq!(v["totalBalance"], 110.0);
        assert_eq!(v["grantedBalance"], 10.0);
        assert_eq!(v["toppedUpBalance"], 100.0);
        assert!(v["fetchedAt"].as_str().is_some());
    }

    #[test]
    fn parse_balance_reports_upstream_error() {
        let err = parse_deepseek_balance(r#"{"error":{"message":"Invalid API key"}}"#).unwrap_err();
        assert!(err.contains("Invalid API key"), "{err}");
        assert!(parse_deepseek_balance("not json").is_err());
        assert!(parse_deepseek_balance(r#"{"balance_infos":[]}"#).is_err());
    }
}
