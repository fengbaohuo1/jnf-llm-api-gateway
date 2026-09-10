# Token 计费与用量统计

## 单价体系

渠道的单价按"每 1k token"存储在数据库（界面输入与展示统一为"每 M token"，换算只发生在界面层）：

- **输入单价**：普通输入（未命中缓存的 prompt）的价格；
- **缓存命中单价**：prompt 前缀命中缓存时的价格，通常远低于普通输入；
- **输出单价**：模型生成部分的单价；其中思考（reasoning/thinking）token 若渠道单独计价，可配置独立的思考单价。

成本计算公式：普通输入 token 数 × 输入单价 + 缓存命中 token 数 × 缓存单价 + 普通输出 × 输出单价 + 思考 token × 思考单价。若缓存或思考单价未配置（为 0），则回落使用对应普通单价。

## 用量明细从哪来

- **非流式**：直接解析上游响应的 `usage` 字段；
- **流式**：透传时注入 `stream_options.include_usage` 让上游在流末尾返回完整 usage；不支持时本地估算（1 token ≈ 4 个 ASCII 字符或 1.5 个非 ASCII 字符）。

不同厂商的字段名不同，网关做统一映射：

- 缓存命中：DeepSeek 系列上报 `prompt_cache_hit_tokens`；OpenAI 系列上报 `prompt_tokens_details.cached_tokens`；
- 思考 token：`completion_tokens_details.reasoning_tokens`。

## 落库与日统计

每次请求完成后：

1. 请求日志写入完整明细：输入 token、命中 token、思考 token、输出 token、总成本、渠道与延迟；
2. 每日统计表按日期累加请求数、各类 token 数与成本，供用量页的日柱状图使用；
3. 密钥配额按实际消耗的 token 扣减，配额用尽后该密钥停止服务。

## 用量页面设计约定

- token 计数一律精确到个位，不缩写为 k/M；
- 成本显示人民币元；
- 柱状图区分缓存命中、普通输入与输出三类 token，悬停可见当日明细。

## 与知识库的关系

知识库问答（RAG）**不经过代理转发**，因此不走 requests 记账表；问答的 token 消耗记录在会话消息行内（tokens_used），两者互不混淆。
