-- 0013 渠道级 DeepSeek 思考模式兼容开关：
-- 原先该兼容是全局设置（compat.deepseek_reasoning），现下放到渠道管理，
-- 由 DeepSeek 渠道自己决定是否给 tool_calls 轮次补空 reasoning_content。
-- 旧渠道默认 1（开启），与原来的默认行为保持一致。
ALTER TABLE channels ADD COLUMN compat_reasoning_content INTEGER NOT NULL DEFAULT 1;
