# LLM API Gateway — Agent 运行记忆

## 项目定位
本地 LLM API 网关（Tauri 2 + React 19 + TS strict + Vite + Tailwind 4 + SQLite/sqlx）。
前端管理页（Dashboard/Usage/Channels/Keys/Logs/Settings）+ 内嵌 axum HTTP 代理（OpenAI 兼容入口）。

## 关键命令
- `npm run tauri dev` — 完整应用（前端 + Rust，首次建库建表）；`npm run dev` — 仅前端（1420 端口）
- `npx tsc --noEmit` — 前端类型检查（strict + noUnusedLocals，新增代码必须无未使用变量）
- `cargo check` / `cargo test` — 在 `src-tauri/` 下执行；cargo 的 exit 1 是 PowerShell stderr 误报，见 `Finished` 即成功

## 后端结构（src-tauri/src/，按层分目录）
- `db/`：连接池 + 迁移；`models/`：serde 结构体（camelCase 给前端）
- `commands/`：Tauri 命令（channels/keys/requests/security/settings/stats），`mod.rs` 只放 `DbState`，`util.rs` 放公共工具（now_iso/hash_key/new_id/gen_key_secret/mask）
- `adapters/`：渠道协议适配器（适配器模式）：`ChannelAdapter` trait + 统一模型；`OpenAiCompatible` 是"协议基类"（trait 默认方法，`compat_*` 命名规避 E0034）；工厂 `create_adapter`；OpenAI/DeepSeek/Custom 已实现，Claude/Gemini 待实现
- `scheduler/`：故障转移调度器 `select_channels(pool, model)`：过滤（active + 模型匹配）→ priority 降序 → 同组 weight 无放回轮盘赌随机
- `security/`：安全扫描集成点 `SecurityScanner` trait（`mod.rs`）+ 内置正则规则 `RuleScanner`（`rules.rs`，密钥/密码/证件/手机号/邮箱/提示注入）
- `proxy/`：转发代理（axum），按职责拆文件：`mod.rs`（ProxyState/PreparedRequest/serve/路由）、`auth.rs`（鉴权）、`settings.rs`（运行设置）、`handlers.rs`（处理器 + prepare_request + 响应构造）、`forward.rs`（透传转发 + 错误分类）、`stream.rs`（SSE 透传 + 统计 + 记账）
- 迁移：0001 五表 + 0002 priority/weight + 0003 单价与 security_logs（第 6 张表）

## 核心设计决策
- **没有故障就不要负载均衡**：正常固定走主渠道（priority 最高），失败才按序转移 ≤ `failover.max_attempts`；可转移错误 = 5xx/429/超时/网络，4xx 直接返回
- **透传而非字段映射**：请求体原样发上游（只改 URL/Authorization），保留 tools 等扩展字段；适配器层留给测试与协议转换
- **SSE 流式**：注入 `stream_options.include_usage` 拿精确 token 统计，无则本地估算（1 token ≈ 4 ASCII 或 1.5 非 ASCII 字符）；delta 逐条脱敏；流结束记账
- **安全**：`security.mode` replace（脱敏后照常，默认）/ block（high 风险拦截）；请求侧 + 响应侧 findings 合并写 security_logs
- **记账**：`commands::requests::log_request` 统一（proxy 与命令共用）；**缓存/思考感知计费**——`extract_usage` 解析明细（缓存：DeepSeek `prompt_cache_hit_tokens` / OpenAI `prompt_tokens_details.cached_tokens`；思考：`completion_tokens_details.reasoning_tokens`），成本 = 未命中输入与普通输出 × 正常单价 + 命中 × `cache_price_per_1k_tokens` + 思考 × `reasoning_price_per_1k_tokens`（0 = 回落正常价）；requests 表记 `cache_hit_tokens`/`reasoning_tokens`，成功扣配额计费，失败尝试也记日志（status=0）
- **密钥**：`sk-llmapi-` + 20 位；keys 表只存 SHA-256 hash + key_prefix（明文仅创建时返回一次）；渠道 `api_key_encrypted` 为明文占位，**未做真实加密**

## 约定与踩坑
- Tauri 命令参数自动 camelCase→snake_case；错误统一 OpenAI 格式 `{"error":{message,type,code}}`
- sqlx `FromRow` 需 derive feature；`migrate!` 需 migrate feature；trait 的 async fn 必须 async-trait（Rust 2021）
- 内存 SQLite 测试池需 `max_connections(1)`；Obsidian 引用块内表格不渲染
- 远程：Gitee HTTPS（凭据管理器有凭据）；PowerShell 5.1 不支持 `&&`，commit 消息用 `-F` 文件避免引号截断

## 当前进度
- ✅ 代理全链路：鉴权 → 请求侧脱敏 → 调度 → 透传 + 故障转移 → 响应侧脱敏 → 合并落库 → 记账
- ✅ 路由：`POST /v1/chat/completions`（非流式/流式分流）、`GET /v1/models`、`GET /health`；32 个单测全绿
- ✅ 缓存/思考感知计费：迁移 0004/0005（缓存单价 + 思考单价 + requests 明细列），日志页展示命中与思考 token
- ⏳ 候选：Claude/Gemini 适配器、渠道密钥真实加密、安全规则可配置化、日志页安全事件展示、前端代理状态页
