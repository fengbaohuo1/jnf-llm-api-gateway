# LLM API Gateway — Agent 运行记忆

## 项目定位
本地 LLM API 网关管理界面（Tauri 2 + React 19 + TypeScript + Vite + Tailwind CSS 4 + SQLite/sqlx）。
已接入本地 SQLite 持久化：前端各页面通过 Tauri 命令调用 Rust 后端读写数据库，替代最初的 mock。

## 关键命令（在仓库根目录执行）
- `npm run dev` — 起 Vite 前端（端口 1420）
- `npm run tauri dev` — 起完整桌面应用（前端 + Rust，首次会建库建表）
- `npm run build` / `npx tsc --noEmit` — 前端类型检查与构建
- cargo 相关命令需在 `src-tauri/` 目录下（`cargo check`）

## 目录结构约定
- `src/pages/` — 路由页面（Dashboard/Usage/Channels/Keys/Logs/Settings）
- `src/components/` — 通用组件（Sidebar、Modal、Badge、ui.tsx）
- `src/api/` — 前端到后端命令的类型化封装（invoke 包装 + 字段映射）
- `src/store/useAppStore.ts` — Zustand 全局状态，动作调用 `api/`
- `src/types/` — 前端共享领域类型
- `src/data/mock.ts` — 仅保留渠道类型枚举等常量（历史 mock 数据可删）
- `src-tauri/src/` — Rust 后端（按层分目录）
  - `db/`：连接池初始化 + 建表迁移
  - `models/`：serde/sqlx 结构体（camelCase 序列化给前端）
  - `commands/`：所有 `#[tauri::command]` CRUD 命令
    - `mod.rs`：`DbState`（含 `pool()` 访问器）+ 公共辅助函数
    - `channels.rs` / `keys.rs` / `requests.rs` / `security.rs` / `settings.rs` / `stats.rs`
  - `adapters/`：**LLM 渠道协议适配器（适配器模式）**
    - `mod.rs`：`ChannelAdapter` trait（async_trait）+ 统一 `ChatRequest/ChatResponse/TestResult` + 工厂 `create_adapter`
    - `openai_compat.rs`：**协议基类** `OpenAiCompatible` trait（默认方法实现 OpenAI 兼容协议，`compat_*` 命名避免与 ChannelAdapter 同名冲突）
    - `openai.rs`：OpenAI 适配器（实现基类，厂商配置：默认端点 api.openai.com/v1、测试模型 gpt-3.5-turbo）
    - `deepseek.rs`：DeepSeek 适配器（实现基类，厂商配置：默认端点 api.deepseek.com、测试模型 deepseek-chat；与 OpenAI 完全独立，不引用 OpenAiAdapter）
  - `scheduler/`：**故障转移调度器**（多渠道同模型时构建有序故障转移队列）
    - `select_channels(pool, model) -> Vec<ChannelChoice>`：过滤（active + 模型匹配）→
      priority 降序 → 同优先级组内 weight **无放回轮盘赌**随机排序
    - **设计取向：没有故障就不要负载均衡**——正常请求固定走队列第一个（主渠道），
      主渠道失败才按队列顺序尝试备用渠道（开关 `failover.enabled`，默认 true；
      上限 `failover.max_attempts`）
    - 渠道表新增 `priority`（越大越先）/ `weight`（组内越大越可能先被选中）字段
    - 调用方：proxy 转发层（每次请求路由时调用）；依赖 rand 0.9
    - 测试：`cargo test scheduler`（内存 SQLite，5 个用例；dev-dependencies 加了 tokio）
  - `security/`：**安全扫描与脱敏模块**（转发层的集成点）
    - `SecurityScanner` trait（async_trait）→ 内置 `RuleScanner`（正则规则集：密钥/密码/手机号/证件号/邮箱 + 提示注入特征）
    - 命中后替换为 `[MASKED:<rule_id>]`；`security.mode`：replace（脱敏后转发，默认）/ block（high 风险拦截）
    - 测试：`cargo test security`（6 个用例）
  - `proxy/`：**转发代理（axum 本地 HTTP 服务）**
    - `POST http://127.0.0.1:{server.port}/v1/chat/completions`，随应用启动常驻
    - 完整链路：鉴权（Bearer sk-llmapi-* → hash 查 keys → 状态/过期/配额）→
      请求侧安全扫描 → `select_channels` 选队列 → **透传转发**（请求体原样发上游，只改 URL/Authorization，保留 tools 等扩展字段）→
      故障转移循环（5xx/429/超时/网络可转移，4xx 直接返回）→ 响应侧安全扫描 → 安全结果合并落库 → 记账
    - 记账：`commands::requests::log_request`（proxy 与 insert_request 命令共用），成本 = total_tokens/1000 × 渠道单价
    - 错误统一 OpenAI 兼容格式 `{"error": {message, type, code}}`；stream=true 暂返回 400
- `src-tauri/migrations/` — 0001 五表 + 0002 priority/weight + 0003 单价与 security_logs

## 数据库架构（重要）
- **SQLite**，用 **sqlx**（`runtime-tokio` + `derive` + `migrate` feature）。
- 数据库文件位于应用数据目录 `app_data_dir/llm_gateway.db`（`lib.rs` setup 里初始化连接池并 manage）。
- **已移除 `tauri-plugin-sql` 与 rusqlite**，统一走 sqlx（避免两套 SQLite 栈）。
- 核心 5 张表：
  - `channels`：渠道（type/base_url/model/status/api_key_encrypted 加密存/latency_ms/priority/weight/price_per_1k_tokens）
  - `keys`：接入密钥（**只存 key_hash + key_prefix**，不存明文；`sk-llmapi-*` 一次性返回）
  - `requests`：请求日志（**冗余存 channel_name/key_name** 便于日志展示筛选，同时保留外键 id 聚合统计）
  - `settings`：KV 设置项
  - `daily_stats`：每日聚合（date 主键），由 `insert_request` 同步累加
- 第 6 张表 `security_logs`：安全事件日志（request_id/stage request|response/rule_id/category/risk/snippet_masked，一对多挂在 requests 下）
- 前端命令约定：字段序列化为 camelCase；Tauri 命令参数自动 camelCase→snake_case。

## 架构与决策记录
- **数据层**：Zustand 动作调用 `api.ts` → invoke 后端命令 → 刷新本地状态；页面不直接碰数据库。
- **密钥安全**：`keys` 表只存 SHA-256 哈希 + 展示前缀，完整明文仅在创建时返回一次。
- **渠道 API Key**：存 `api_key_encrypted` 字段（当前为占位，未做真正加密，**后续需补加密**）。
- **故障转移（调度器）设计取向（重要）**：个人网关**没有故障就不要负载均衡**——
  正常请求固定走主渠道（优先级最高），前缀缓存 100% 命中、账单完全可预期；
  主渠道失败才按队列顺序尝试备用渠道（多出来的渠道只用于容灾，不主动分摊流量）。
  队列排序：过滤 → priority 降序 → 同组 weight 权重随机（无放回轮盘赌）。
  开关 `failover.enabled`（默认 true）+ `failover.max_attempts`。详见 `scheduler/` 与笔记文档。
- **转发代理（proxy/）设计取向（重要）**：本地 HTTP 服务（axum），入口
  `POST http://127.0.0.1:{server.port}/v1/chat/completions`，随应用启动常驻。
  - **透传而非字段映射**：请求体 JSON 原样转发上游（只改 URL 与 Authorization），
    保证 tools/response_format 等扩展字段 100% 兼容；适配器层保留给渠道测试与协议转换。
  - 可转移错误 = 上游 5xx/429/超时/网络错误；4xx 视为配置错误直接返回（转移无意义）。
  - 安全扫描（security/）：请求侧脱敏后转发、响应侧脱敏后返回，两侧 findings 合并写入 security_logs；
    `security.mode`：replace（默认，脱敏后照常）/ block（high 风险拦截）。
  - 记账：成功才扣配额与计费（成本 = total_tokens/1000 × 渠道单价），失败尝试也记 requests 日志（status=0，error 注明第几次尝试/转移）。
  - stream=true 第一版返回 400 不支持。
- **Tailwind CSS 4**：无 tailwind.config.js，靠 `@tailwindcss/vite` + `@import "tailwindcss"` 自动扫描。
- **路由**：集中在 `src/App.tsx`（React Router v7）。
- **远程仓库**：Gitee 走 **HTTPS**（`https://gitee.com/jianingfei/llm-api-gateway.git`），
  本机未给 Gitee 配置 SSH 公钥；Windows 凭据管理器存有 Gitee 凭据，push 无需输密码。

## 命名与代码约定
- 密钥格式：`sk-llmapi-` + 20 位小写字母数字后缀（后端 `commands.rs` 的 `gen_key_secret`）
- 渠道类型：OpenAI / DeepSeek / Claude / Gemini / Custom
- 页面组件导出命名与路由一致；共享类型放 `src/types.ts`
- Rust 命令函数名前缀与表对应（`list_channels`、`create_key`、`today_stats` 等）

## 已知限制 / 当前进度
- 前后端已接通，CRUD + 统计走真实 SQLite
- 渠道测试 `test_channel` 已通过适配器**真实请求上游**（OpenAI/DeepSeek/Custom 兼容协议）
- ✅ **转发代理已实现**（`proxy/`：axum 本地 HTTP 服务，鉴权 → 请求侧安全扫描 → 调度器 →
  透传转发 + 故障转移循环 → 响应侧安全扫描合并 → 记账），随应用启动常驻 127.0.0.1:8787
- ✅ **安全模块已实现**（`security/`：SecurityScanner trait + RuleScanner 内置规则集，6 个单测通过）
- ✅ **故障转移调度器已接入转发层**（`select_channels` 由 proxy 每次请求调用），5 个单测通过
- ✅ channels 表已加 priority/weight/price_per_1k_tokens；设置页有"故障转移"与"代理与安全"Tab
- ✅ 记账统一走 `commands::requests::log_request`（proxy 与 insert_request 命令共用）
- Claude/Gemini 适配器尚未实现（工厂返回"尚未实现"错误）
- 渠道密钥存密为占位，未做真实加密
- 流式 SSE 未做（stream=true 返回 400）；模型级路由未做（渠道单 model 字段 + 调度器过滤已覆盖）
- 下一步候选：Claude/Gemini 适配器（原生协议转换）、流式支持、渠道密钥真实加密、安全规则可配置化、日志页安全事件展示

## 适配器架构（重要）
- `adapters/` 用**适配器模式**：`ChannelAdapter` trait（`#[async_trait]`）定义统一契约
  （`chat_completion` / `test`），各厂商协议差异封装在各自实现里。
- 统一模型：`ChatRequest`（model/messages/temperature/max_tokens/stream）、
  `ChatResponse`（id/model/content/usage/latency_ms）、`TestResult`。
- 工厂 `create_adapter(channel_type, base_url, api_key)`：OpenAI/Custom →
  `OpenAiAdapter`；DeepSeek → `DeepSeekAdapter`（各自实现 `OpenAiCompatible` 基类，
  独立类型互不引用）；Claude/Gemini 待实现。
- `OpenAiCompatible` 是"协议基类"（Rust 无继承，用 trait 默认方法实现）：各厂商适配器
  只需提供厂商配置（默认端点/默认模型/密钥/客户端）即获得完整 OpenAI 兼容协议实现。
- 协议基类方法用 `compat_*` 命名（`compat_chat_completion`/`compat_test`），
  避免与 `ChannelAdapter` 同名方法在 supertrait 场景下的调用歧义（E0034）。
- HTTP 客户端 `reqwest`（rustls-tls）；错误统一为 `AdapterError`（thiserror）。
- 注意：Rust 2021 edition 下 trait 的 `async fn` 不能直接用作 `dyn` trait object，
  必须用 `async-trait` crate。

## 踩坑备忘
- `tsconfig.json` 开了 `strict` + `noUnusedLocals` + `noUnusedParameters`，
  新增代码必须无未使用变量/参数，否则 `tsc` 报错。
- sqlx 的 `FromRow` 需要 `derive` feature；`migrate!` 需要 `migrate` feature。
- Tauri 命令是 `async fn` + `State<'_, DbState>`，连接池在 `lib.rs` setup 中用 `tauri::async_runtime::block_on` 初始化。
- 本开发沙箱中 `cargo check` 的 `exit code 1` 是 PowerShell 把 stderr 当错误的误报；看到 `Finished` 即编译成功。
