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
- `commands/`：Tauri 命令（channels/keys/requests/security/settings/stats），`mod.rs` 放 `DbState` + `AppState`（进程级内存标志，窗口同步回调不能查库），`util.rs` 放公共工具（now_iso/hash_key/new_id/gen_key_secret/mask）
- `config.rs`：配置文件 `settings.json`（app config 目录，Tauri Store 同格式扁平 KV）——**启动导入覆盖 DB（文件权威，改文件重启生效），`set_setting` 写 DB 后全量 dump 回文件（双写），`reload_settings_file` 命令支持热加载**；解析失败备份为 .json.bak 不拖垮启动
- `adapters/`：渠道协议适配器（适配器模式）：`ChannelAdapter` trait + 统一模型；`OpenAiCompatible` 是"协议基类"（trait 默认方法，`compat_*` 命名规避 E0034）；工厂 `create_adapter`；OpenAI/DeepSeek/Custom 已实现，Claude/Gemini 待实现
- `scheduler/`：故障转移调度器 `select_channels(pool, model)`：过滤（active + 模型匹配）→ priority 降序 → 同组 weight 无放回轮盘赌随机
- `security/`：企业级审计引擎——多引擎扫描（策略模式）：`rules.rs`（正则规则引擎 + DB 加载 `load_rules` + 内置默认 `default_defs`）、`substring.rs`（子串引擎，Aho-Corasick 多模式大小写不敏感）、`encoding.rs`（隐写/混淆）、`command.rs`（带毒命令）、`leak.rs`（上下文泄露比对 `check_leak`）、`refresh.rs`（`RefreshScanner` TTL 30s 重载）；`mod.rs` 含 `CompositeScanner` 按 match_type/category 分流 + PDP `decide`（record/mask/block）；`Finding.action` 记录处置动作；**record = 仅记录不修改内容（snippet 标 `[DETECTED:]`），mask/block 替换为 `[MASKED:]`**
- `proxy/`：转发代理（axum），按职责拆文件：`mod.rs`（ProxyState/PreparedRequest/serve/路由）、`auth.rs`（鉴权 + auth_events 审计）、`settings.rs`（运行设置）、`handlers.rs`（处理器 + prepare_request + 响应构造）、`forward.rs`（透传转发 + 错误分类）、`stream.rs`（SSE 透传 + 统计 + 记账）
- 迁移：0001 五表 + 0002 priority/weight + 0003 单价与 security_logs + 0004 缓存单价 + 0005 思考单价 + 0006 daily_stats 明细列 + 0007 审计引擎（security_rules/auth_events/admin_audit_logs + security_logs.action）+ 0008 规则校准 + 0009 子串匹配（match_type 列 + 子串种子）

## 核心设计决策
- **没有故障就不要负载均衡**：正常固定走主渠道（priority 最高），失败才按序转移 ≤ `failover.max_attempts`；可转移错误 = 5xx/429/超时/网络，4xx 直接返回
- **透传而非字段映射**：请求体原样发上游（只改 URL/Authorization），保留 tools 等扩展字段；适配器层留给测试与协议转换
- **SSE 流式**：注入 `stream_options.include_usage` 拿精确 token 统计，无则本地估算（1 token ≈ 4 ASCII 或 1.5 非 ASCII 字符）；delta 逐条脱敏；流结束记账
- **安全**：`security.mode` replace（脱敏后照常，默认）/ block（high 拦截）；**多引擎扫描**（规则/编码/命令/上下文泄露比对）+ **PDP 决策**（任一规则 action=block 无条件拦截，与全局模式无关）；规则存 DB 可配置（`RefreshScanner` 30s TTL 自动重载）；请求侧 + 响应侧 findings 合并写 security_logs（含 action）；**审计**：auth_events（含匿名失败，暴力破解检测基础）+ admin_audit_logs（渠道/密钥/设置/规则变更留痕，`util::record_admin_audit` 静默失败）
- **记账**：`commands::requests::log_request` 统一（proxy 与命令共用）；**缓存/思考感知计费**——`extract_usage` 解析明细（缓存：DeepSeek `prompt_cache_hit_tokens` / OpenAI `prompt_tokens_details.cached_tokens`；思考：`completion_tokens_details.reasoning_tokens`），成本 = 未命中输入与普通输出 × 正常单价 + 命中 × `cache_price_per_1k_tokens` + 思考 × `reasoning_price_per_1k_tokens`（0 = 回落正常价）；requests 表记 `cache_hit_tokens`/`reasoning_tokens`，daily_stats 按日累加三部分明细；成功扣配额计费，失败尝试也记日志（status=0）
- **界面单位**：价格以"元/M tokens"输入展示（`perM`/`per1k` 换算），存储仍按每 1k；token 计数一律精确到个位（不缩写 k/M）；成本显示 ¥（后端即为元）
- **密钥**：`sk-llmapi-` + 32 位 hex（128 bit 熵）；keys 表只存 SHA-256 hash + key_prefix（明文仅创建/轮换时返回一次）；支持编辑（名称/配额/过期）与轮换（旧钥立即失效、用量保留）；渠道 `api_key_encrypted` 为明文占位，**未做真实加密**
- **桌面收尾**：托盘（tray-icon feature，`TrayIconBuilder` 代码构建，左键单击显示窗口，菜单"显示主窗口/退出"；`app.exit(0)` 才真正退出）；关闭按钮 → 隐藏到托盘（`on_window_event` CloseRequested `api.prevent_close()` + hide，开关 `general.close_to_tray` 存 `AppState` 内存标志）；单实例插件（`tauri-plugin-single-instance` 防多开）；开机自启（`tauri-plugin-autostart`，args 在 Rust `init()` 配置 `--hidden`，前端 `enable()` **无参数**；启动检查 `std::env::args()` 含 `--hidden` 则隐藏窗口进托盘）
- **主题**：`data-theme` 属性 + CSS 变量（`index.css` `@theme` 定义 `--color-app/card/inset/hover/edge/strong/main/sub/muted/faint/accent/accent-soft` 生成 `bg-app`/`text-main`/`border-edge` 等工具类，`[data-theme="dark"]` 覆盖同名变量即全站切换；`@custom-variant dark` 供彩色徽章少量 `dark:` 变体）；`theme.ts` 三态（light/dark/system，system 监听 `prefers-color-scheme` 变化动态切 data-theme，main.tsx 渲染前同步 `applyTheme()` 防闪烁）；灰阶一律用变量类，彩色/强调保留原类

## 约定与踩坑
- Tauri 命令参数自动 camelCase→snake_case；错误统一 OpenAI 格式 `{"error":{message,type,code}}`
- sqlx `FromRow` 需 derive feature；`migrate!` 需 migrate feature；trait 的 async fn 必须 async-trait（Rust 2021）
- 内存 SQLite 测试池需 `max_connections(1)`；Obsidian 引用块内表格不渲染
- **迁移不可变**：已应用的迁移文件严禁修改（sqlx 校验哈希，改了启动即失败）；规则/数据修正必须新增迁移（如 0008 校准规则）
- **Rust regex 不支持 lookaround**（`(?=...)`/`(?<=...)` 编译报错且 `Regex::new().ok()` 会静默丢弃规则）——用"分段 + 交替"表达；新增规则后必须跑测试验证实际生效
- 时间：`util::now_iso`/`today_iso` 为**北京时间**（UTC+8 硬编码，无夏令时），日志/日期/密钥过期校验统一走它；价格输入 `step="any"` 支持任意小数
- CSS：父元素高度为 auto 时子元素百分比高度会解析为 auto → 柱状图/进度条必须给列 `h-full` + 中间层 `flex-1` 定高，百分比才能生效
- **Tauri 2 桌面坑**：`setup` 闭包是同步的，查库必须 `tauri::async_runtime::block_on`；`TrayIcon` 必须 `app.manage()` 保活（drop 即移除托盘）；NSIS 配置**没有** `createDesktopShortcut`/`shortcutName` 字段（桌面快捷方式默认创建）；autostart 插件 args 在 Rust `init()` 配置，前端 `enable()` 无参数
- **批量文本替换防误伤**：`String.Replace("bg-indigo-50","bg-accent-soft")` 会把 `bg-indigo-500` 变成 `bg-accent-soft0`（子串包含）——批量替换后必须 grep 数字后缀产物（`accent-soft0`/`inset0` 等）核查；subagent 批量改样式在此环境不可靠（两轮静默失败），改用脚本 + 人工核查
- 远程：Gitee HTTPS（凭据管理器有凭据）；PowerShell 5.1 不支持 `&&`，commit 消息用 `-F` 文件避免引号截断

## 当前进度
- ✅ 代理全链路：鉴权 → 请求侧脱敏 → 调度 → 透传 + 故障转移 → 响应侧脱敏 → 合并落库 → 记账
- ✅ 路由：`POST /v1/chat/completions`（非流式/流式分流）、`GET /v1/models`、`GET /health`；52 个单测全绿
- ✅ 缓存/思考感知计费：迁移 0004/0005（缓存单价 + 思考单价 + requests 明细列），日志页展示命中与思考 token
- ✅ 用量拆分与界面紧凑化：迁移 0006（daily_stats 明细列），用量页三合一柱状图（命中/未命中/输出）精确计数；价格改元/M；现代细滚动条
- ✅ UI 打磨：窗口默认 1280×800（min 1024×700）；渠道表单"负载均衡/价格"改手风琴折叠；用量页去掉易换行的拆分文字，柱状图悬停浮窗显示当日各类 token（柱子用 `h-40` 定高 + 绝对定位百分比防贴底）；界面术语统一"输出"（reasoning 不再叫"思考"）
- ✅ 密钥强化：`sk-llmapi-` + 32 位 hex（128 bit 熵，UUID v4 全量）；`update_key`（改名称/配额/过期）+ `rotate_key`（作废旧钥换新，用量保留，明文仅一次返回）；Keys 页编辑/轮换按钮与弹窗
- ✅ 安全审计引擎：迁移 0007/0008；多引擎扫描（规则/隐写/命令/上下文泄露）+ PDP 决策 + 规则配置化（30s 自动生效）；auth_events（含匿名失败）+ admin_audit_logs 全量审计；安全页三 Tab（事件统计/规则管理/审计日志）；0008 校准 base64/凭据规则误报
- ✅ 子串匹配引擎：迁移 0009（match_type 列）；Aho-Corasick 多模式大小写不敏感匹配，中文注入短语/危险命令/PEM 私钥头/内网地址种子规则；规则页可选正则/子串；record 语义修正（仅记录不脱敏）
- ✅ 发布收尾：配置文件 settings.json（启动导入 + UI 双写 + 热重载按钮）；系统托盘（左键显示/菜单退出/关闭到托盘开关）+ 单实例；开机自启（--hidden 进托盘）；主题三态（data-theme + CSS 变量全站暗色）；NSIS 打包（currentUser + 简体中文，identifier 修正为 jianingfei.site.llmapi，版本 1.0.0）
- ⏳ 候选：Claude/Gemini 适配器、渠道密钥真实加密、阈值告警（连续鉴权失败锁定/block 通知）、应用图标定制（当前为 Tauri 默认图标）
