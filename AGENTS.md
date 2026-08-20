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
    - `channels.rs` / `keys.rs` / `requests.rs` / `settings.rs` / `stats.rs`
  - `adapters/`：**LLM 渠道协议适配器（适配器模式）**
    - `mod.rs`：`ChannelAdapter` trait（async_trait）+ 统一 `ChatRequest/ChatResponse/TestResult` + 工厂 `create_adapter`
    - `openai.rs`：OpenAI 兼容协议适配器（覆盖 OpenAI/Custom）
    - `deepseek.rs`：DeepSeek 适配器（组合复用 OpenAiAdapter，默认端点 api.deepseek.com、测试模型 deepseek-chat）
- `src-tauri/migrations/0001_init.sql` — 核心表结构

## 数据库架构（重要）
- **SQLite**，用 **sqlx**（`runtime-tokio` + `derive` + `migrate` feature）。
- 数据库文件位于应用数据目录 `app_data_dir/llm_gateway.db`（`lib.rs` setup 里初始化连接池并 manage）。
- **已移除 `tauri-plugin-sql` 与 rusqlite**，统一走 sqlx（避免两套 SQLite 栈）。
- 核心 5 张表：
  - `channels`：渠道（type/base_url/model/status/api_key_encrypted 加密存/latency_ms）
  - `keys`：接入密钥（**只存 key_hash + key_prefix**，不存明文；`sk-waliapi-*` 一次性返回）
  - `requests`：请求日志（**冗余存 channel_name/key_name** 便于日志展示筛选，同时保留外键 id 聚合统计）
  - `settings`：KV 设置项
  - `daily_stats`：每日聚合（date 主键），由 `insert_request` 同步累加
- 前端命令约定：字段序列化为 camelCase；Tauri 命令参数自动 camelCase→snake_case。

## 架构与决策记录
- **数据层**：Zustand 动作调用 `api.ts` → invoke 后端命令 → 刷新本地状态；页面不直接碰数据库。
- **密钥安全**：`keys` 表只存 SHA-256 哈希 + 展示前缀，完整明文仅在创建时返回一次。
- **渠道 API Key**：存 `api_key_encrypted` 字段（当前为占位，未做真正加密，**后续需补加密**）。
- **Tailwind CSS 4**：无 tailwind.config.js，靠 `@tailwindcss/vite` + `@import "tailwindcss"` 自动扫描。
- **路由**：集中在 `src/App.tsx`（React Router v7）。
- **远程仓库**：Gitee 走 **HTTPS**（`https://gitee.com/jianingfei/llm-api-gateway.git`），
  本机未给 Gitee 配置 SSH 公钥；Windows 凭据管理器存有 Gitee 凭据，push 无需输密码。

## 命名与代码约定
- 密钥格式：`sk-waliapi-` + 20 位小写字母数字后缀（后端 `commands.rs` 的 `gen_key_secret`）
- 渠道类型：OpenAI / DeepSeek / Claude / Gemini / Custom
- 页面组件导出命名与路由一致；共享类型放 `src/types.ts`
- Rust 命令函数名前缀与表对应（`list_channels`、`create_key`、`today_stats` 等）

## 已知限制 / 当前进度
- 前后端已接通，CRUD + 统计走真实 SQLite
- 渠道测试 `test_channel` 已通过适配器**真实请求上游**（OpenAI/DeepSeek/Custom 兼容协议）
- Claude/Gemini 适配器尚未实现（工厂返回"尚未实现"错误）
- 网关**真实请求转发**（chat_completion 命令暴露给前端）尚未做
- 渠道密钥存密为占位，未做真实加密
- 下一步候选：Claude/Gemini 适配器、真实转发命令、密钥字段加密、模型级路由、流式支持

## 适配器架构（重要）
- `adapters/` 用**适配器模式**：`ChannelAdapter` trait（`#[async_trait]`）定义统一契约
  （`chat_completion` / `test`），各厂商协议差异封装在各自实现里。
- 统一模型：`ChatRequest`（model/messages/temperature/max_tokens/stream）、
  `ChatResponse`（id/model/content/usage/latency_ms）、`TestResult`。
- 工厂 `create_adapter(channel_type, base_url, api_key)`：OpenAI/Custom →
  `OpenAiAdapter`；DeepSeek → `DeepSeekAdapter`（组合复用 OpenAI 协议逻辑）；
  Claude/Gemini 待实现。
- HTTP 客户端 `reqwest`（rustls-tls）；错误统一为 `AdapterError`（thiserror）。
- 注意：Rust 2021 edition 下 trait 的 `async fn` 不能直接用作 `dyn` trait object，
  必须用 `async-trait` crate。

## 踩坑备忘
- `tsconfig.json` 开了 `strict` + `noUnusedLocals` + `noUnusedParameters`，
  新增代码必须无未使用变量/参数，否则 `tsc` 报错。
- sqlx 的 `FromRow` 需要 `derive` feature；`migrate!` 需要 `migrate` feature。
- Tauri 命令是 `async fn` + `State<'_, DbState>`，连接池在 `lib.rs` setup 中用 `tauri::async_runtime::block_on` 初始化。
- 本开发沙箱中 `cargo check` 的 `exit code 1` 是 PowerShell 把 stderr 当错误的误报；看到 `Finished` 即编译成功。
