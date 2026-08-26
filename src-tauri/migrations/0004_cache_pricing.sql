-- 0004 缓存感知计费：渠道缓存命中单价 + 请求日志记录缓存命中 token
-- 缓存命中单价为 0 表示未配置，计费回落到正常单价（不区分缓存）
ALTER TABLE channels ADD COLUMN cache_price_per_1k_tokens REAL NOT NULL DEFAULT 0;

-- 输入 token 中命中前缀缓存的个数（DeepSeek/OpenAI usage 明细），供日志展示与成本核对
ALTER TABLE requests ADD COLUMN cache_hit_tokens INTEGER NOT NULL DEFAULT 0;
