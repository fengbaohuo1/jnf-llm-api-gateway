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
- `commands/`：Tauri 命令（channels/keys/requests/security/settings/stats/kb/mcp），`mod.rs` 放 `DbState` + `AppState`（进程级内存标志，窗口同步回调不能查库），`util.rs` 放公共工具（now_iso/hash_key/new_id/gen_key_secret/mask）
- `config.rs`：配置文件 `settings.json`（app config 目录，Tauri Store 同格式扁平 KV）——**启动导入覆盖 DB（文件权威，改文件重启生效），`set_setting` 写 DB 后全量 dump 回文件（双写），`reload_settings_file` 命令支持热加载**；解析失败备份为 .json.bak 不拖垮启动
- `adapters/`：渠道协议适配器（适配器模式）：`ChannelAdapter` trait + 统一模型；`OpenAiCompatible` 是"协议基类"（trait 默认方法，`compat_*` 命名规避 E0034）；工厂 `create_adapter`；OpenAI/DeepSeek/Custom 已实现，Claude/Gemini 待实现
- `scheduler/`：故障转移调度器 `select_channels(pool, model)`：过滤（active + 模型匹配）→ priority 降序 → 同组 weight 无放回轮盘赌随机
- `security/`：企业级审计引擎——多引擎扫描（策略模式）：`rules.rs`（正则规则引擎 + DB 加载 `load_rules` + 内置默认 `default_defs`）、`substring.rs`（子串引擎，Aho-Corasick 多模式大小写不敏感）、`encoding.rs`（隐写/混淆）、`command.rs`（带毒命令）、`leak.rs`（上下文泄露比对 `check_leak`）、`refresh.rs`（`RefreshScanner` TTL 30s 重载）；`mod.rs` 含 `CompositeScanner` 按 match_type/category 分流 + PDP `decide`（record/mask/block）；`Finding.action` 记录处置动作；**record = 仅记录不修改内容（snippet 标 `[DETECTED:]`），mask/block 替换为 `[MASKED:]`**
- `proxy/`：转发代理（axum），按职责拆文件：`mod.rs`（ProxyState/PreparedRequest/serve/路由）、`auth.rs`（鉴权 + auth_events 审计）、`settings.rs`（运行设置）、`handlers.rs`（处理器 + prepare_request + 响应构造）、`forward.rs`（透传转发 + 错误分类）、`stream.rs`（SSE 透传 + 统计 + 记账）
- `services/`：**服务框架**（`ServiceRegistry` + `Service` trait：id/name/default_enabled/as_any/schema_versions/init/shutdown），服务级 schema 版本化（`service_schema` 表，**Rust 静态数组声明，不走 sqlx migrate**），启用键 `services.<id>.enabled`，运行态 disabled/running/failed，支持热启停与重启
- `services/kb/`：知识库——`parse.rs`（md/txt → 章节流 → 分块）、`data.rs`（CRUD/导入/双 FTS 同步/`ensure_bigram_index`）、`tokenize.rs`（查询分词 + bigram 预处理）、`search.rs`（关键词双路 + RRF + 重排 + 阈值）、`retrieve.rs`（问答检索策略层 + `kb.retrieval.*` 参数）、`embed.rs`（Embedder 抽象 + 维度白名单）、`index.rs`/`hnsw.rs`（自研 HNSW）、`indexing.rs`（embed+建索引任务管道）、`rag.rs`（会话问答）、`eval.rs`（recall@k/MRR 评测）、`schema.rs`（KB_SCHEMA 0001–0005）
- `services/mcp/`：MCP 服务（自研 JSON-RPC 2.0 子集，无 SDK）——`protocol.rs`（initialize/tools/list/tools/call/notifications/ping + 错误码）、`tools.rs`（13 个知识库工具 + 风险类别/参数元信息 + 准入拦截）、`server.rs`（POST /mcp，JSON 或 SSE，Bearer 实时校验，调用记录）、`config.rs`（`services.mcp.*` 配置解析）、`schema.rs`（`mcp_calls` 服务级迁移）、`mod.rs`（监听、实际端口、调用记录读写）
- 迁移：0001 五表 + 0002 priority/weight + 0003 单价与 security_logs + 0004 缓存单价 + 0005 思考单价 + 0006 daily_stats 明细列 + 0007 审计引擎（security_rules/auth_events/admin_audit_logs + security_logs.action）+ 0008 规则校准 + 0009 子串匹配（match_type 列 + 子串种子）+ 0010 请求内容明细（requests 重建 + request_details 大字段表）+ 0011 请求风险等级（requests.risk_level，供日志分级/筛选）；**KB 服务级**：0001 核心表 + 0002 embedding 绑定/状态 + 0003 任务台账 + 0004 RAG 绑定 + 0005 关键词双索引（trigram 补 heading/title、bigram 表、`text_version`）；**MCP 服务级**：0001 `mcp_calls`

## 核心设计决策
- **没有故障就不要负载均衡**：正常固定走主渠道（priority 最高），失败才按序转移 ≤ `failover.max_attempts`；可转移错误 = 5xx/429/超时/网络，4xx 直接返回
- **透传而非字段映射**：请求体原样发上游（只改 URL/Authorization），保留 tools 等扩展字段；适配器层留给测试与协议转换
- **SSE 流式**：注入 `stream_options.include_usage` 拿精确 token 统计，无则本地估算（1 token ≈ 4 ASCII 或 1.5 非 ASCII 字符）；delta 逐条脱敏；流结束记账
- **安全**：`security.mode` replace（脱敏后照常，默认）/ block（high 拦截）；**多引擎扫描**（规则/编码/命令/上下文泄露比对）+ **PDP 决策**（任一规则 action=block 无条件拦截，与全局模式无关）；规则存 DB 可配置（`RefreshScanner` 30s TTL 自动重载，表为空才回退默认规则、全禁用=不扫描）；**扫描范围=全 JSON 字符串叶子**（`scan_json_for_security`：消息正文/多模态 parts/tool_calls.arguments/system/reasoning；流式逐事件扫所有 choices 且 `data:` 带不带空格都解析）；请求侧 + 响应侧 findings 合并写 security_logs（含 action），请求被拦截时也补写安全事件；**审计**：auth_events（含匿名失败，暴力破解检测基础）+ admin_audit_logs（渠道/密钥/设置/规则变更留痕，`util::record_admin_audit` 静默失败）；上游错误体/错误信息落库前先脱敏
- **记账**：`commands::requests::log_request` 统一（proxy 与命令共用），请求行 / 内容明细 / daily_stats / 配额扣减在同一事务内提交；**缓存/思考感知计费**——`extract_usage` 解析明细（缓存：DeepSeek `prompt_cache_hit_tokens` / OpenAI `prompt_tokens_details.cached_tokens`；思考：`completion_tokens_details.reasoning_tokens`），成本 = 未命中输入与普通输出 × 正常单价 + 命中 × `cache_price_per_1k_tokens` + 思考 × `reasoning_price_per_1k_tokens`（0 = 回落正常价）；requests 表记 `cache_hit_tokens`/`reasoning_tokens`，daily_stats 按日累加三部分明细；成功扣配额计费，失败尝试也记日志（status=0）；**上游 4xx 原状态码透传**（`ForwardError.status`），5xx/429 耗尽后返回最后一次上游状态
- **日志分级存储**：`log.store_level`（all/low/medium/high/none）按请求（请求侧 + 响应侧 findings 的最高风险，`security::max_risk`）决定是否落库 `request_details` 内容；低于阈值只保留概要行。概要 / token / 计费 / daily_stats / security_logs 始终记录，不受级别影响；`none` 等价旧 `log.content_enabled=false`（旧键作为回退，启动时自动补齐新键并写回 settings.json）。requests.risk_level 记录最终风险（none 表示无命中，安全扫描关闭时也是 none），日志页据此展示徽标与筛选；响应侧被拦截时也补记请求日志 + 安全事件（status=0，风险 high）
- **界面单位**：价格以"元/M tokens"输入展示（`perM`/`per1k` 换算），存储仍按每 1k；token 计数一律精确到个位（不缩写 k/M）；成本显示 ¥（后端即为元）
- **密钥**：`sk-llmapi-` + 32 位 hex（128 bit 熵）；keys 表只存 SHA-256 hash + key_prefix（明文仅创建/轮换时返回一次）；支持编辑（名称/配额/过期）与轮换（旧钥立即失效、用量保留）；渠道 `api_key_encrypted` 为明文占位，**未做真实加密**
- **知识库检索（关键词双索引）**：`kb_chunks_bg`（unicode61 + `tokenize::bigram_text` 预处理：CJK 2 字滑窗、英文整词）承载中文主力检索，任意 ≥2 字符查询可用；`kb_chunks_fts`（trigram，含 content/heading/title 三列）只服务非 CJK ≥3 字符词元，补英文子串。两路按词元类型**互斥**后 RRF 融合（`1/(60+rank)`，权重 向量 1.0 / 关键词 0.7）。**中文不能按词长把整句送 trigram**：标点切段后仍是一长串，整串短语必然 0 命中（旧实现根因）
- **KB 问答检索策略层**（`retrieve.rs`，与 `search_any` 召回原语分离）：多取候选（top_k×3）→ `rerank`（按名次基准分做乘法加分：两路皆中 ×1.15、标题 ×1.10、heading ×1.05、超长惩罚；每文档 ≤2 块）→ 相关度阈值（`vec_score` 低于阈值且**无关键词分**才丢弃）→ 邻居扩展（命中块 → 下一块 → 上一块拼接，citation 仍指向原块）。参数键 `kb.retrieval.topK/minSimilarity/rrfWeightVector/rrfWeightKeyword/perDocCap/neighborExpand`，改 `settings.json` 即可调（`minSimilarity` 默认 0.45，是按 bge-m3 实测校准的，换嵌入模型要重标）
- **嵌入文本配方版本**：embed 输入 = `标题 + 章节路径 + 正文`（存储仍是原文），版本号写 `kb_index_meta.text_version`；与 `indexing::EMBED_TEXT_VERSION` 不一致时 `search_vector` 报"请重建索引"（问答自动降级关键词）
- **MCP**：只读/写知识库工具 13 个，删除类需 `confirm:true`，写操作与 `kb_ask` 走 `admin_audit_logs`；不暴露渠道绑定工具。**管理面在「MCP 服务」页**（`src/pages/Mcp.tsx`）：监听范围（127.0.0.1 / 0.0.0.0）、端口、Bearer 令牌（生成/掩码/清除，明文只返回一次）、**只读模式**、**逐工具开关**（`services.mcp.tools.<name>.enabled`）、工具清单（类别徽章 只读/写入/危险 + 有外部费用 + 参数表 + inputSchema + curl 示例）、调用记录（`mcp_calls`，**不落参数**）、客户端接入片段、**工具调试台**（走后端同一个 `run_tool`，与外部客户端同一条准入/审计/记录路径）。令牌/只读/工具开关**每请求实时读设置**（改完立即生效）；host/port 在 init 绑定，改动需重启服务。`tools/list` 按 MCP 规范带 `annotations.readOnlyHint/destructiveHint/openWorldHint`
- **桌面收尾**：托盘（tray-icon feature，`TrayIconBuilder` 代码构建，左键单击显示窗口，菜单"显示主窗口/退出"；`app.exit(0)` 才真正退出）；关闭按钮 → 隐藏到托盘（`on_window_event` CloseRequested `api.prevent_close()` + hide，开关 `general.close_to_tray` 存 `AppState` 内存标志）；单实例插件（`tauri-plugin-single-instance` 防多开）；开机自启（`tauri-plugin-autostart`，args 在 Rust `init()` 配置 `--hidden`，前端 `enable()` **无参数**；启动检查 `std::env::args()` 含 `--hidden` 则隐藏窗口进托盘）
- **主题**：`data-theme` 属性 + CSS 变量（`index.css` `@theme` 定义 `--color-app/card/inset/hover/edge/strong/main/sub/muted/faint/accent/accent-soft` 生成 `bg-app`/`text-main`/`border-edge` 等工具类，`[data-theme="dark"]` 覆盖同名变量即全站切换；`@custom-variant dark` 供彩色徽章少量 `dark:` 变体）；`theme.ts` 三态（light/dark/system，system 监听 `prefers-color-scheme` 变化动态切 data-theme，main.tsx 渲染前同步 `applyTheme()` 防闪烁）；灰阶一律用变量类，彩色/强调保留原类

## 约定与踩坑
- Tauri 命令参数自动 camelCase→snake_case；错误统一 OpenAI 格式 `{"error":{message,type,code}}`
- sqlx `FromRow` 需 derive feature；`migrate!` 需 migrate feature；trait 的 async fn 必须 async-trait（Rust 2021）
- 内存 SQLite 测试池需 `max_connections(1)`；Obsidian 引用块内表格不渲染
- **迁移不可变**：已应用的迁移文件严禁修改（sqlx 校验哈希，改了启动即失败）；规则/数据修正必须新增迁移（如 0008 校准规则）
- **Rust regex 不支持 lookaround**（`(?=...)`/`(?<=...)` 编译报错且 `Regex::new().ok()` 会静默丢弃规则）——用"分段 + 交替"表达；新增规则后必须跑测试验证实际生效
- 时间：`util::now_iso`/`today_iso` 为**北京时间**（UTC+8 硬编码，无夏令时），日志/日期/密钥过期校验统一走它；价格输入 `step="any"` 支持任意小数
- **FTS5 trigram 查不到 <3 字符**：`"限流"` 返回 **0 行且不报错**（静默无命中）——中文 2 字词必须另建 bigram 索引；FTS5 内置分词器**没有 bigram**，只能应用层把 CJK 连续段预处理成空格分隔的 2 字滑窗 + `unicode61`，查询侧同样切 bigram 后短语查询（token 相邻性即子串语义）
- **bm25 是负值（越小越相关）**：对关键词命中做乘法加分（×1.1）会把排序**方向翻转**——重排要么用"名次基准分"，要么先把分数转成同向
- sqlx `FromRow` 新增的**非查询列**字段必须加 `#[sqlx(default)]`，否则运行期报缺列；SQLite `-(...)` 表达式是 INTEGER，映射到 Rust `f64` 会报 `mismatched types`，需 `CAST(... AS REAL)`
- CSS：父元素高度为 auto 时子元素百分比高度会解析为 auto → 柱状图/进度条必须给列 `h-full` + 中间层 `flex-1` 定高，百分比才能生效
- **删除渠道/密钥先置空引用**：`requests.channel_id/key_id` 有外键（0010 未设 ON DELETE），直接 DELETE 会 FOREIGN KEY 失败；删除命令在同一事务里先 `UPDATE requests SET channel_id/key_id = NULL` 再删（历史日志行与用量统计保留）
- **MCP 监听 0.0.0.0 必须带令牌**：`init` 与 `mcp_save_config` 均拒绝「0.0.0.0 且 services.mcp.token 为空」的配置（否则局域网可无鉴权调用知识库删除/导入工具）
- **Tauri 2 桌面坑**：`setup` 闭包是同步的，查库必须 `tauri::async_runtime::block_on`；`TrayIcon` 必须 `app.manage()` 保活（drop 即移除托盘）；NSIS 配置**没有** `createDesktopShortcut`/`shortcutName` 字段（桌面快捷方式默认创建）；autostart 插件 args 在 Rust `init()` 配置，前端 `enable()` 无参数
- **批量文本替换防误伤**：`String.Replace("bg-indigo-50","bg-accent-soft")` 会把 `bg-indigo-500` 变成 `bg-accent-soft0`（子串包含）——批量替换后必须 grep 数字后缀产物（`accent-soft0`/`inset0` 等）核查；subagent 批量改样式在此环境不可靠（两轮静默失败），改用脚本 + 人工核查
- 远程：Gitee HTTPS（凭据管理器有凭据）；PowerShell 5.1 不支持 `&&`，commit 消息用 `-F` 文件避免引号截断
- **学习/总结类文档写在个人知识库 `D:\docs\knowledge\llmapi\`（Obsidian vault 根为 `D:\docs`），不要放进本仓库**——仓库只保留代码、`AGENTS.md`（AI 运行记忆）与 `kb/`（语料 + 评测集）。vault 约定：一篇一主题、中文文件名、`[[wikilink]]` 互相引用（已覆盖的主题直接链接，不重复写）、结构为「问题背景 → 核心方案 → 关键代码/配置 → 踩坑记录」、能用 Mermaid 画的别堆文字、概念优先不贴代码备份
- **文档库重构已完成 P0**：决策与执行计划见 vault 根目录《文档库重构方案.md》（已迁到 `D:\docs\`，文档分「理解层 `knowledge/` / 事实层 `projects/`」，靠 `tools/` 下 lint+gen 脚本做自动校验与索引；新增笔记必须带 frontmatter 并跑 `node tools/lint.mjs`；RAG 接入与 wiki 为后续阶段，本期不做）。压缩上下文后接手相关工作，先读那份方案 + vault 的 `AGENTS.md`

## 当前进度
- ✅ 代理全链路：鉴权 → 请求侧脱敏 → 调度 → 透传 + 故障转移 → 响应侧脱敏 → 合并落库 → 记账
- ✅ 路由：`POST /v1/chat/completions`（非流式/流式分流）、`GET /v1/models`、`GET /health`；单测全绿
- ✅ 缓存/思考感知计费：迁移 0004/0005（缓存单价 + 思考单价 + requests 明细列），日志页展示命中与思考 token
- ✅ 用量拆分与界面紧凑化：迁移 0006（daily_stats 明细列），用量页三合一柱状图（命中/未命中/输出）精确计数；价格改元/M；现代细滚动条
- ✅ UI 打磨：窗口默认 1280×800（min 1024×700）；渠道表单"负载均衡/价格"改手风琴折叠；用量页去掉易换行的拆分文字，柱状图悬停浮窗显示当日各类 token（柱子用 `h-40` 定高 + 绝对定位百分比防贴底）；界面术语统一"输出"（reasoning 不再叫"思考"）
- ✅ 密钥强化：`sk-llmapi-` + 32 位 hex（128 bit 熵，UUID v4 全量）；`update_key`（改名称/配额/过期）+ `rotate_key`（作废旧钥换新，用量保留，明文仅一次返回）；Keys 页编辑/轮换按钮与弹窗
- ✅ 安全审计引擎：迁移 0007/0008；多引擎扫描（规则/隐写/命令/上下文泄露）+ PDP 决策 + 规则配置化（30s 自动生效）；auth_events（含匿名失败）+ admin_audit_logs 全量审计；安全页三 Tab（事件统计/规则管理/审计日志）；0008 校准 base64/凭据规则误报
- ✅ 子串匹配引擎：迁移 0009（match_type 列）；Aho-Corasick 多模式大小写不敏感匹配，中文注入短语/危险命令/PEM 私钥头/内网地址种子规则；规则页可选正则/子串；record 语义修正（仅记录不脱敏）
- ✅ 发布收尾：配置文件 settings.json（启动导入 + UI 双写 + 热重载按钮）；系统托盘（左键显示/菜单退出/关闭到托盘开关）+ 单实例；开机自启（--hidden 进托盘）；主题三态（data-theme + CSS 变量全站暗色）；NSIS 打包（currentUser + 简体中文，identifier 修正为 jianingfei.site.llmapi，版本 1.0.0）
- ✅ 知识库：解析/分块（md 章节流 + 段落贪心 + 重叠硬窗）、双 FTS 索引、embedding 渠道绑定（维度白名单）、自研 HNSW + 一致性守卫、embed/建索引任务管道（进度/重试/中断恢复）、RAG 会话问答（引用溯源 `[n]`）、MCP 服务（13 工具）；KB 页四 Tab（文档与索引 / 检索 / 问答 / 评测）
- ✅ 检索质量第一批：关键词双索引（bigram 主力 + trigram 英文子串）、查询拆词 + 停用词、加权 RRF + 保留原始分（`vecScore`/`kwScore`）、融合后确定性重排（标题/双路加分 + 每文档上限）、相关度阈值、邻居块扩展、嵌入文本配方版本化（`text_version`，需重建索引）、`kb_eval` 评测（recall@k/MRR/拒答正确率）+ `kb/evals/sample.json`
- 📁 语料与评测：`kb/samples/`（5 篇中文样例文档，导入用；文档标题 = 文件名去掉扩展名）+ `kb/evals/`（评测用例 JSON，KB 页「评测」Tab 选文件运行）
- ✅ MCP 管理面：`src/pages/Mcp.tsx` + `commands/mcp.rs`（状态/工具清单与开关/配置/令牌/调用记录/调试台）；服务级 `mcp_calls` 表；`tools/list` 带 MCP annotations；只读模式与逐工具开关在 `run_tool` 强制（真实进程实测：禁用/只读/令牌轮换全部生效）
- ✅ 按安全等级存储日志：迁移 0011（requests.risk_level）；`log.store_level` 全存/低风险及以上/中风险及以上/高风险及以上/不存内容，阈值以下只存概要（计费与安全事件不受影响）；日志页风险徽标 + 风险筛选 + 详情说明；流式拦截改记 status=0；设置页「通用设置」可选级别（即时生效）
- ✅ 渠道模型配置便利：`fetch_channel_models`（后端拉上游 `/models`，支持 base 带/不带 /v1、编辑时复用已存 Key）；表单按类型预填 Base URL、常用模型 chips、获取模型列表点选、状态新增时生效、提交错误在弹窗内展示；新增渠道默认状态生效（此前状态字段被忽略）
- ✅ UI/UX 清理：移除未生效的配置项（重试策略 Tab、server.base_path、启用请求日志、默认模型、紧凑模式、表格密度）避免误导；Modal 支持 Esc 关闭 + ARIA；列表表格横向滚动不裁切；请求详情补齐风险信息
- ✅ 验证：`npx tsc --noEmit` 通过；**176 单测全绿**（新增 risk_level 迁移/记账、content_allowed、max_risk、models_url 候选、全树扫描端到端、规则全禁用等用例）
- ✅ 代码审查加固：SSE 单行 1 MiB 上限；非对象请求体拒绝（防协议转换 unwrap panic）；上游错误体/错误信息落库前脱敏；请求侧拦截补写 security_logs；4xx 原状态码透传；规则「全禁用」不再回退默认；账务四步合并事务；删除渠道/密钥先置空历史引用；MCP 0.0.0.0 强制令牌；日志页状态徽标改「成功/失败」并支持手动刷新/风险筛选、Modal 焦点管理、设置 Tab 常驻避免丢编辑
- ⏳ 候选：jieba/bigram 粒度 A/B（用评测集定夺）、rerank 模型、向量索引增量更新、多轮 query 改写、MCP 工具级参数约束（如 kb_search 的 topK 上限可配）、Claude/Gemini 适配器、渠道密钥真实加密、阈值告警（连续鉴权失败锁定/block 通知）、应用图标定制（当前为 Tauri 默认图标）
