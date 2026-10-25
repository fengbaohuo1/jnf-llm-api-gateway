-- 渠道按模型定价：JSON 对象 model -> 价格（元/1k tokens；DeepSeek 含空闲时段 offPeak）
-- 空对象 = 使用渠道级默认单价
ALTER TABLE channels ADD COLUMN model_prices TEXT NOT NULL DEFAULT '{}';
