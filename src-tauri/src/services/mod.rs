//! 服务装配框架（轻量插件式服务注册 + 运行时状态管理）。
//!
//! 目标：让知识库、MCP 等后续扩展以**独立服务**注册到网关，核心不感知服务实现。
//! 本应用是单体（单进程 + 单库），本框架解决的是**装配点**问题而非部署问题：
//! 新增服务 = 在 `services/` 下实现 [`Service`] + 在 [`registry`] 登记一行，生命周期、
//! 启用开关、运行时状态、失败隔离由框架统一处理，不再散改 lib.rs / 状态注入 / 迁移清单。
//!
//! ## 运行时状态管理
//! [`ServiceRegistry`] 为进程级注册表（启动时 `app.manage` 托管）：
//! - **活实例**：启动装配后服务实例（`Arc<dyn Service>`）保留在注册表中，服务内部的
//!   状态（知识库索引、MCP 连接等）有容器可放，不会随装配结束被丢弃；
//! - **每服务状态**：`disabled`（开关关闭，未 init）/ `running`（init 成功）/
//!   `failed`（init 失败，保留错误信息）；状态存内存，仅代表**本次运行**的装配结果；
//! - **热启停**：[`ServiceRegistry::apply_enabled`] 运行时启用（立即 init）或停用服务，
//!   不阻塞其它服务，也不要求重启应用；设置键 `services.<id>.enabled` 由命令层同步写入
//!   （settings.json 双写 + 审计），应用重启后按键重放，状态一致；
//! - **热重启**：[`ServiceRegistry::restart`] 对失败服务重跑 init（init 幂等建表），
//!   修复后无需重启应用即可恢复。
//!
//! ## 约定
//! - **启用开关**：设置键 `services.<id>.enabled`（"true"/"false"），无键时用
//!   [`Service::default_enabled`]；装配发生在启动阶段（`start`），开关改动在下次启动
//!   或调用 `apply_enabled` 时生效。
//! - **失败隔离**：某服务 init 失败只记录并跳过，不阻塞其它服务与应用启动。
//! - **服务 schema 版本化**：服务通过 [`Service::schema_versions`] 声明自己的 SQL
//!   迁移（版本名 + 语句数组），框架用 `service_schema` 表记录已应用版本，只执行
//!   未应用部分并包在单个事务内（失败整体回滚、服务置 failed）。服务 schema 不
//!   进入中央 `migrations/` 目录，框架不感知服务内部结构，但演进受版本保护。
//! - **受管资源**：服务只能通过 [`ServiceContext`] 使用资源（当前为连接池；审计/设置/
//!   call_llm 等横切句柄后续在此追加）——这是服务无法绕过核心横切能力的保证。
//!
//! 当前内置服务：`kb`（知识库，占位）、`mcp`（MCP 工具服务，占位）。

pub mod kb;
pub mod mcp;

use std::any::Any;
use std::sync::{Arc, Mutex};

use sqlx::SqlitePool;

/// 服务装配上下文：框架注入的受管资源。
#[derive(Debug)]
pub struct ServiceContext {
    /// SQLite 连接池（全部服务共享同一库；服务自身 schema 在 init 中幂等自建）
    // 占位阶段无消费者（各服务均未使用资源）；契约预留，首个真实服务落地后自然消除
    #[allow(dead_code)]
    pub pool: SqlitePool,
}

/// 服务生命周期契约。新增服务实现本 trait 并在 [`registry`] 登记。
/// `Any` 超 trait + [`Service::as_any`] 允许命令层按具体类型取回活实例
/// （如知识库的内存索引挂在服务 struct 内部）。
#[async_trait::async_trait]
pub trait Service: Any + Send + Sync {
    /// 稳定标识：设置键 `services.<id>.enabled`、日志与未来的端点前缀均以此为准。
    fn id(&self) -> &'static str;
    /// 展示名（日志与服务列表 UI）。
    fn name(&self) -> &'static str;
    /// 一句话描述（服务列表 UI）。
    fn description(&self) -> &'static str;
    /// 设置键不存在时的默认启用状态。
    fn default_enabled(&self) -> bool {
        true
    }
    /// 类型擦除视图：命令层用 `registry.get(id)` + `downcast_ref` 取回具体实例。
    /// 每个实现写一行 `fn as_any(&self) -> &dyn Any { self }`（无法给默认实现：
    /// 默认方法体中 Self 未定大小，不能做 dyn 转换）。
    fn as_any(&self) -> &dyn Any;
    /// 服务级 schema 迁移：`(版本名, SQL 语句数组)`，按声明顺序执行；已应用版本
    /// 由框架记录于 `service_schema` 表并跳过，未应用部分在单个事务内执行
    /// （失败整体回滚，服务置 failed）。默认无。
    fn schema_versions(&self) -> &'static [(&'static str, &'static [&'static str])] {
        &[]
    }
    /// 启动装配/热启用/热重启时调用（schema 应用之后）：预热 / 自检。
    /// Err 仅记录并置本服务为 failed，不阻塞其它服务。
    async fn init(&self, ctx: &ServiceContext) -> Result<(), String>;
}

/// 启用开关的设置键（`services.<id>.enabled`）。
pub fn enabled_key(id: &str) -> String {
    format!("services.{id}.enabled")
}

/// 内置服务注册表：新增服务在这里登记一行。
pub fn registry() -> Vec<Box<dyn Service>> {
    vec![
        Box::new(kb::KnowledgeBaseService::default()),
        Box::new(mcp::McpService),
    ]
}

/// 单个服务本次启动的装配结果（`ServiceRegistry::start` 返回，供启动日志使用）。
#[derive(Debug)]
pub struct ServiceBoot {
    pub id: &'static str,
    pub name: &'static str,
    /// 本服务是否启用（false = 开关关闭，未执行 init）
    pub enabled: bool,
    /// init 是否成功（未启用时为 false 且 error 为 None）
    pub ok: bool,
    /// init 失败原因（仅启用且失败时有值）
    pub error: Option<String>,
}

/// 服务运行时状态（内存态，仅代表本次运行的装配结果；启用与否以设置键为准）。
#[derive(Debug)]
enum RuntimeState {
    /// 开关关闭，未执行 init
    Disabled,
    /// init 成功，服务可用
    Running,
    /// init 失败，保留错误信息
    Failed(String),
}

/// 注册表条目：活实例 + 运行时状态
struct Entry {
    svc: Arc<dyn Service>,
    state: RuntimeState,
}

/// 进程级服务注册表：装配并持有全部服务的活实例与运行时状态。
/// 由 Tauri `app.manage` 托管，命令通过 `State<ServiceRegistry>` 访问。
pub struct ServiceRegistry {
    pool: SqlitePool,
    inner: Mutex<Vec<Entry>>,
}

impl ServiceRegistry {
    /// 用内置注册表创建（未装配）。
    pub fn new(pool: SqlitePool) -> Self {
        Self::from_services(pool, registry())
    }

    /// 用给定服务列表创建（测试注入假服务用）。
    fn from_services(pool: SqlitePool, services: Vec<Box<dyn Service>>) -> Self {
        let entries = services
            .into_iter()
            .map(|s| Entry {
                svc: Arc::from(s),
                state: RuntimeState::Disabled,
            })
            .collect();
        Self {
            pool,
            inner: Mutex::new(entries),
        }
    }

    /// 启动装配：按注册顺序读取启用开关，启用的服务执行 init。
    /// 返回每个服务的装配结果（供启动日志）；实例与状态保留在注册表中。
    pub async fn start(&self) -> Vec<ServiceBoot> {
        let n = self.inner.lock().unwrap().len();
        let mut boots = Vec::with_capacity(n);
        for i in 0..n {
            let svc = self.svc_at(i);
            let id = svc.id();
            let name = svc.name();
            let enabled = enabled_in_settings(&self.pool, svc.as_ref()).await;
            if !enabled {
                self.set_state(i, RuntimeState::Disabled);
                boots.push(ServiceBoot {
                    id,
                    name,
                    enabled: false,
                    ok: false,
                    error: None,
                });
                continue;
            }
            let result = self.init_one(i).await;
            match result {
                Ok(()) => boots.push(ServiceBoot {
                    id,
                    name,
                    enabled: true,
                    ok: true,
                    error: None,
                }),
                Err(e) => boots.push(ServiceBoot {
                    id,
                    name,
                    enabled: true,
                    ok: false,
                    error: Some(e),
                }),
            }
        }
        boots
    }

    /// 当前全部服务的状态快照（供 list_services 命令 / UI）。
    pub fn list(&self) -> Vec<ServiceInfo> {
        let g = self.inner.lock().unwrap();
        g.iter()
            .map(|e| {
                let svc = &e.svc;
                ServiceInfo {
                    id: svc.id(),
                    name: svc.name(),
                    description: svc.description(),
                    enabled: matches!(e.state, RuntimeState::Running | RuntimeState::Failed(_)),
                    status: match &e.state {
                        RuntimeState::Disabled => "disabled",
                        RuntimeState::Running => "running",
                        RuntimeState::Failed(_) => "failed",
                    },
                    error: match &e.state {
                        RuntimeState::Failed(err) => Some(err.clone()),
                        _ => None,
                    },
                }
            })
            .collect()
    }

    /// 运行时启停服务（命令层先同步设置键 `services.<id>.enabled` 再调用本方法）。
    /// enabled=true：立即 init（已 running 则 no-op）；false：置为 disabled。
    pub async fn apply_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        let idx = self.find(id)?;
        if enabled {
            let running = {
                let g = self.inner.lock().unwrap();
                matches!(g[idx].state, RuntimeState::Running)
            };
            if running {
                return Ok(());
            }
            self.init_one(idx).await.map(|_| ())
        } else {
            self.set_state(idx, RuntimeState::Disabled);
            Ok(())
        }
    }

    /// 热重启：对已启用的服务重跑 init（init 幂等，可安全重复）。
    /// 停用中的服务不可重启（先启用）。
    pub async fn restart(&self, id: &str) -> Result<(), String> {
        let idx = self.find(id)?;
        let disabled = {
            let g = self.inner.lock().unwrap();
            matches!(g[idx].state, RuntimeState::Disabled)
        };
        if disabled {
            return Err(format!("服务 {id} 已停用，请先启用再重启"));
        }
        self.init_one(idx).await.map(|_| ())
    }

    /// 取第 i 个服务的活实例（Arc clone，锁外可安全 await init）。
    fn svc_at(&self, i: usize) -> Arc<dyn Service> {
        self.inner.lock().unwrap()[i].svc.clone()
    }

    /// 按 id 取活实例（Arc clone；命令层配合 `as_any().downcast_ref::<T>()`
    /// 取回具体类型，访问服务内部状态如内存索引）。
    pub fn get(&self, id: &str) -> Option<Arc<dyn Service>> {
        let g = self.inner.lock().unwrap();
        g.iter()
            .find(|e| e.svc.id() == id)
            .map(|e| e.svc.clone())
    }

    /// 按 id 查条目下标。
    fn find(&self, id: &str) -> Result<usize, String> {
        let g = self.inner.lock().unwrap();
        g.iter()
            .position(|e| e.svc.id() == id)
            .ok_or_else(|| format!("服务 {id} 不存在"))
    }

    /// 执行一次服务准备（schema 迁移 + init）并更新状态（锁外 await，避免持锁跨 await）。
    async fn init_one(&self, idx: usize) -> Result<(), String> {
        let svc = self.svc_at(idx);
        let result = match self.apply_schema(svc.as_ref()).await {
            Ok(()) => svc
                .init(&ServiceContext {
                    pool: self.pool.clone(),
                })
                .await,
            Err(e) => Err(e),
        };
        match &result {
            Ok(()) => self.set_state(idx, RuntimeState::Running),
            Err(e) => self.set_state(idx, RuntimeState::Failed(e.clone())),
        }
        result
    }

    /// 服务级 schema 迁移：先建框架的 `service_schema` 记录表，找出未应用版本，
    /// 逐版本在单事务内执行全部语句并写记录；任一步失败整体回滚并返回错误。
    async fn apply_schema(&self, svc: &dyn Service) -> Result<(), String> {
        let versions = svc.schema_versions();
        if versions.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS service_schema (
                id TEXT NOT NULL,
                version TEXT NOT NULL,
                applied_at TEXT NOT NULL,
                PRIMARY KEY (id, version)
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| format!("创建 service_schema 表失败: {e}"))?;

        let id = svc.id();
        let mut pending: Vec<(&'static str, &'static [&'static str])> = Vec::new();
        for &(ver, _stmts) in versions {
            let applied: Option<String> = sqlx::query_scalar(
                "SELECT version FROM service_schema WHERE id = ?1 AND version = ?2",
            )
            .bind(id)
            .bind(ver)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            if applied.is_none() {
                pending.push((ver, _stmts));
            }
        }
        if pending.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        for (ver, stmts) in pending {
            for sql in stmts {
                sqlx::query(sql)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| format!("服务 {id} schema 版本 {ver} 执行失败（已回滚）: {e}"))?;
            }
            sqlx::query(
                "INSERT INTO service_schema (id, version, applied_at) VALUES (?1, ?2, ?3)",
            )
            .bind(id)
            .bind(ver)
            .bind(crate::commands::util::now_iso())
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("服务 {id} schema 版本 {ver} 记录失败（已回滚）: {e}"))?;
        }
        tx.commit().await.map_err(|e| e.to_string())
    }

    /// 更新第 i 个服务的状态。
    fn set_state(&self, i: usize, state: RuntimeState) {
        if let Ok(mut g) = self.inner.lock() {
            if let Some(e) = g.get_mut(i) {
                e.state = state;
            }
        }
    }
}

/// 服务运行时状态快照（list_services 命令返回，camelCase 对齐前端）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// 是否启用（running/failed = 启用；disabled = 停用）
    pub enabled: bool,
    /// running | failed | disabled
    pub status: &'static str,
    /// init 失败原因（仅 failed 时有值）
    pub error: Option<String>,
}

/// 读取服务启用开关：设置键存在则以值为准（仅 "true" 视为启用），否则用默认值。
async fn enabled_in_settings(pool: &SqlitePool, s: &dyn Service) -> bool {
    sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?1")
        .bind(enabled_key(s.id()))
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or_else(|| s.default_enabled())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;
    use std::sync::atomic::{AtomicBool, Ordering};

    async fn test_pool(create_settings: bool) -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        if create_settings {
            sqlx::query(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT)",
            )
            .execute(&pool)
            .await
            .unwrap();
        }
        pool
    }

    async fn set_enabled(pool: &SqlitePool, id: &str, value: &str) {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, '2025-01-01 00:00:00')",
        )
        .bind(enabled_key(id))
        .bind(value)
        .execute(pool)
        .await
        .unwrap();
    }

    /// 测试用成功服务
    struct OkService(&'static str);

    #[async_trait::async_trait]
    impl Service for OkService {
        fn id(&self) -> &'static str {
            self.0
        }
        fn name(&self) -> &'static str {
            self.0
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn description(&self) -> &'static str {
            "test"
        }
        async fn init(&self, _ctx: &ServiceContext) -> Result<(), String> {
            Ok(())
        }
    }

    /// 测试用恒定失败服务
    struct BoomService(&'static str, &'static str);

    #[async_trait::async_trait]
    impl Service for BoomService {
        fn id(&self) -> &'static str {
            self.0
        }
        fn name(&self) -> &'static str {
            self.0
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn description(&self) -> &'static str {
            "test"
        }
        async fn init(&self, _ctx: &ServiceContext) -> Result<(), String> {
            Err(self.1.to_string())
        }
    }

    /// 测试用"首次失败、重试成功"服务（模拟热重启恢复）
    struct FlakyService(&'static str);

    #[async_trait::async_trait]
    impl Service for FlakyService {
        fn id(&self) -> &'static str {
            self.0
        }
        fn name(&self) -> &'static str {
            self.0
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn description(&self) -> &'static str {
            "test"
        }
        async fn init(&self, _ctx: &ServiceContext) -> Result<(), String> {
            static ARMED: AtomicBool = AtomicBool::new(true);
            if ARMED.swap(false, Ordering::SeqCst) {
                Err("首次初始化失败".to_string())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn registry_ids_unique_and_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for s in registry() {
            assert!(!s.id().is_empty(), "服务 id 不能为空");
            assert!(seen.insert(s.id()), "服务 id 重复: {}", s.id());
            assert!(!s.name().is_empty(), "服务 {} 缺展示名", s.id());
            assert!(!s.description().is_empty(), "服务 {} 缺描述", s.id());
        }
    }

    #[tokio::test]
    async fn default_all_services_boot_ok() {
        let pool = test_pool(false).await;
        let reg = ServiceRegistry::new(pool);
        let boots = reg.start().await;

        assert_eq!(boots.len(), 2);
        for b in &boots {
            assert!(b.enabled, "{} 应默认启用", b.id);
            assert!(b.ok, "{} 占位 init 应成功", b.id);
            assert!(b.error.is_none());
        }
        let info = reg.list();
        assert_eq!(info.len(), 2);
        for s in &info {
            assert!(s.enabled && s.status == "running" && s.error.is_none(), "{} 状态应为 running", s.id);
        }
    }

    #[tokio::test]
    async fn disabled_service_is_skipped() {
        let pool = test_pool(true).await;
        set_enabled(&pool, "kb", "false").await;
        let reg = ServiceRegistry::new(pool);
        let boots = reg.start().await;

        let kb = boots.iter().find(|b| b.id == "kb").unwrap();
        assert!(!kb.enabled && !kb.ok && kb.error.is_none(), "停用服务不应执行 init");

        let mcp = boots.iter().find(|b| b.id == "mcp").unwrap();
        assert!(mcp.enabled && mcp.ok, "其它服务不受影响");

        let info = reg.list();
        let kb = info.iter().find(|s| s.id == "kb").unwrap();
        assert_eq!(kb.status, "disabled");
        assert!(!kb.enabled);
    }

    #[tokio::test]
    async fn non_true_value_counts_as_disabled() {
        let pool = test_pool(true).await;
        set_enabled(&pool, "mcp", "yes").await; // 脏值视为停用
        let reg = ServiceRegistry::new(pool);
        reg.start().await;
        let info = reg.list();
        let mcp = info.iter().find(|s| s.id == "mcp").unwrap();
        assert!(!mcp.enabled && mcp.status == "disabled");
    }

    #[tokio::test]
    async fn failing_service_does_not_block_others() {
        let pool = test_pool(false).await;
        let reg = ServiceRegistry::from_services(
            pool,
            vec![
                Box::new(BoomService("boom", "磁盘损坏")),
                Box::new(OkService("fine")),
            ],
        );
        let boots = reg.start().await;

        assert_eq!(boots.len(), 2);
        assert_eq!(boots[0].id, "boom");
        assert!(!boots[0].ok);
        assert_eq!(boots[0].error.as_deref(), Some("磁盘损坏"));
        assert_eq!(boots[1].id, "fine");
        assert!(boots[1].enabled && boots[1].ok);

        let info = reg.list();
        let boom = info.iter().find(|s| s.id == "boom").unwrap();
        assert_eq!(boom.status, "failed");
        assert_eq!(boom.error.as_deref(), Some("磁盘损坏"));
        let fine = info.iter().find(|s| s.id == "fine").unwrap();
        assert_eq!(fine.status, "running");
    }

    #[tokio::test]
    async fn apply_enabled_toggles_runtime_state() {
        let pool = test_pool(false).await;
        let reg = ServiceRegistry::new(pool);
        reg.start().await;

        // 停用 kb
        reg.apply_enabled("kb", false).await.unwrap();
        let info = reg.list();
        let kb = info.iter().find(|s| s.id == "kb").unwrap();
        assert!(!kb.enabled && kb.status == "disabled");

        // 重新启用 → 立即 init → running
        reg.apply_enabled("kb", true).await.unwrap();
        let info = reg.list();
        let kb = info.iter().find(|s| s.id == "kb").unwrap();
        assert!(kb.enabled && kb.status == "running" && kb.error.is_none());

        // 未知 id 报错
        assert!(reg.apply_enabled("nope", true).await.is_err());
    }

    #[tokio::test]
    async fn restart_recovers_failed_service() {
        let pool = test_pool(false).await;
        let reg = ServiceRegistry::from_services(pool, vec![Box::new(FlakyService("flaky"))]);
        let boots = reg.start().await;
        assert!(!boots[0].ok, "首次 init 应失败");

        let info = reg.list();
        assert_eq!(info[0].status, "failed");

        // 热重启 → 恢复 running
        reg.restart("flaky").await.unwrap();
        let info = reg.list();
        assert!(info[0].enabled && info[0].status == "running" && info[0].error.is_none());
    }

    #[tokio::test]
    async fn restart_rejected_when_disabled() {
        let pool = test_pool(false).await;
        let reg = ServiceRegistry::new(pool);
        reg.start().await;
        reg.apply_enabled("kb", false).await.unwrap();
        let err = reg.restart("kb").await.unwrap_err();
        assert!(err.contains("已停用"), "停用服务不应允许重启: {err}");
    }

    #[tokio::test]
    async fn registry_get_returns_typed_instance() {
        let pool = test_pool(false).await;
        let reg = ServiceRegistry::from_services(pool, vec![Box::new(OkService("kb-typed"))]);
        reg.start().await;
        let svc = reg.get("kb-typed").expect("按 id 取活实例");
        let typed = svc
            .as_any()
            .downcast_ref::<OkService>()
            .expect("应能 downcast 回具体类型");
        assert_eq!(typed.0, "kb-typed");
        assert!(reg.get("missing").is_none());
    }

    /// 测试用带 schema 的服务（两个版本）
    struct SchemaService(&'static str);

    #[async_trait::async_trait]
    impl Service for SchemaService {
        fn id(&self) -> &'static str {
            self.0
        }
        fn name(&self) -> &'static str {
            self.0
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn description(&self) -> &'static str {
            "test"
        }
        fn schema_versions(&self) -> &'static [(&'static str, &'static [&'static str])] {
            &[
                (
                    "0001",
                    &["CREATE TABLE s_t1 (a TEXT NOT NULL)"],
                ),
                (
                    "0002",
                    &[
                        "CREATE TABLE s_t2 (a TEXT NOT NULL)",
                        "INSERT INTO s_t2 (a) VALUES ('x')",
                    ],
                ),
            ]
        }
        async fn init(&self, _ctx: &ServiceContext) -> Result<(), String> {
            Ok(())
        }
    }

    /// 测试用 schema 中途失败的坏服务（版本内第二条 SQL 非法）
    struct BadSchemaService(&'static str);

    #[async_trait::async_trait]
    impl Service for BadSchemaService {
        fn id(&self) -> &'static str {
            self.0
        }
        fn name(&self) -> &'static str {
            self.0
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn description(&self) -> &'static str {
            "test"
        }
        fn schema_versions(&self) -> &'static [(&'static str, &'static [&'static str])] {
            &[(
                "0001",
                &[
                    "CREATE TABLE b_t (a TEXT NOT NULL)",
                    "INSERT INTO b_t (a) VALUES ('ok')",
                    "THIS IS NOT SQL",
                ],
            )]
        }
        async fn init(&self, _ctx: &ServiceContext) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn schema_bootstraps_once_and_skips_on_restart() {
        let pool = test_pool(false).await;
        let reg = ServiceRegistry::from_services(pool, vec![Box::new(SchemaService("schema"))]);
        let boots = reg.start().await;
        assert!(boots[0].ok, "schema 应用后 init 应成功");

        // 两张表已建、数据已插、版本已记录
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM s_t2")
            .fetch_one(&reg.pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        let versions: Vec<String> =
            sqlx::query_scalar("SELECT version FROM service_schema WHERE id = 'schema' ORDER BY version")
                .fetch_all(&reg.pool)
                .await
                .unwrap();
        assert_eq!(versions, vec!["0001".to_string(), "0002".to_string()]);

        // 热重启：已应用版本跳过，不重复执行（数据不翻倍）
        reg.restart("schema").await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM s_t2")
            .fetch_one(&reg.pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "schema 不应重复执行");
    }

    #[tokio::test]
    async fn schema_failure_rolls_back_and_fails_service() {
        let pool = test_pool(false).await;
        let reg = ServiceRegistry::from_services(
            pool,
            vec![Box::new(BadSchemaService("badschema"))],
        );
        let boots = reg.start().await;
        assert!(!boots[0].ok);
        let err = boots[0].error.as_deref().unwrap_or_default();
        assert!(err.contains("0001") && err.contains("回滚"), "错误应含版本与回滚信息: {err}");

        // 整个版本事务回滚：DDL 也不残留，版本未记录
        let info = reg.list();
        assert_eq!(info[0].status, "failed");
        let tbl: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'b_t'",
        )
        .fetch_optional(&reg.pool)
        .await
        .unwrap();
        assert!(tbl.is_none(), "失败版本的表应随事务回滚消失");
        let v: Option<String> = sqlx::query_scalar(
            "SELECT version FROM service_schema WHERE id = 'badschema'",
        )
        .fetch_optional(&reg.pool)
        .await
        .unwrap();
        assert!(v.is_none());
    }
}
