# LLM API Gateway — Agent 运行记忆

## 项目定位
本地 LLM API 网关管理界面（Tauri 2 + React 19 + TypeScript + Vite + Tailwind CSS 4）。
当前为**纯前端初步实现**：数据来自内存 mock + Zustand，尚未接入后端/持久化。

## 关键命令（在仓库根目录执行）
- `npm run dev` — 起 Vite 前端（端口 1420）
- `npm run tauri dev` — 起完整桌面应用（前端 + Rust）
- `npm run build` — 类型检查 + 生产构建
- `npx tsc --noEmit` — 仅前端类型检查
- cargo 相关命令需在 `src-tauri/` 目录下（Cargo.toml 在那里）

## 目录结构约定
- `src/pages/` — 路由页面（Dashboard/Usage/Channels/Keys/Logs/Settings）
- `src/components/` — 通用组件（Sidebar、Modal、Badge、ui.tsx）
- `src/store/useAppStore.ts` — Zustand 全局状态（渠道/密钥/日志的 mock 数据源）
- `src/data/mock.ts` — 模拟数据
- `src/types.ts` — 共享领域类型定义
- `src-tauri/` — Rust 后端（Cargo.toml、src/lib.rs 注册 Tauri 插件）

## 架构与决策记录
- **数据层**：目前用 Zustand + mock 模拟 CRUD；接入真实后端时，只需替换
  `src/store/useAppStore.ts` 里的实现，页面组件无需大改。
- **Tailwind CSS 4**：无 tailwind.config.js，靠 `@tailwindcss/vite` + `@import "tailwindcss"` 自动扫描。
- **路由**：路由集中在 `src/App.tsx`（React Router v7）。
- **远程仓库**：Gitee 走 **HTTPS**（`https://gitee.com/jianingfei/llm-api-gateway.git`），
  本机未给 Gitee 配置 SSH 公钥；Windows 凭据管理器存有 Gitee 凭据，push 无需输密码。

## 命名与代码约定
- 密钥格式：`sk-waliapi-` + 20 位小写字母数字后缀（见 `src/pages/Keys.tsx` 的 generateKey）
- 渠道类型：OpenAI / DeepSeek / Claude / Gemini / Custom
- 页面组件导出命名与路由一致；共享类型放 `src/types.ts`

## 已知限制 / 当前进度
- 前端骨架已完成，功能为初步实现（未接真实数据/后端）
- 下一步候选：接入 Tauri Rust 后端、SQL 持久化、真实代理转发

## 踩坑备忘
- `tsconfig.json` 开了 `strict` + `noUnusedLocals` + `noUnusedParameters`，
  新增代码必须无未使用变量/参数，否则 `tsc` 报错。
