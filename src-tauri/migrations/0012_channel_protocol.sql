-- 0012 渠道协议：把「渠道类型（品牌）」与「上游协议」解耦。
-- 之前 Claude/Gemini/Custom 共用 OpenAI 兼容适配器，无法为自定义渠道显式选择协议；
-- 现在 protocol 为空表示按 type 自动推导（OpenAI/Custom→openai，DeepSeek→deepseek，
-- Claude→anthropic，Gemini→gemini），非空则显式指定 openai / deepseek / anthropic / gemini。
ALTER TABLE channels ADD COLUMN protocol TEXT NOT NULL DEFAULT '';
