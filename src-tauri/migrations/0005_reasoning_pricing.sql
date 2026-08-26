-- 0005 思考 token（reasoning）感知计费：渠道思考单价 + 请求日志记录思考 token
-- 思考单价为 0 表示未配置，回落正常输出单价
ALTER TABLE channels ADD COLUMN reasoning_price_per_1k_tokens REAL NOT NULL DEFAULT 0;

-- 输出中用于推理（thinking）的 token 数（DeepSeek-R1 / OpenAI o 系列 usage 明细）
ALTER TABLE requests ADD COLUMN reasoning_tokens INTEGER NOT NULL DEFAULT 0;
