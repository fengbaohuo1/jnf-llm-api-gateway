-- 0014 渠道支持多个模型：
-- channels.models 存 JSON 字符串数组（如 ["deepseek-chat","deepseek-reasoner"]）；
-- 空数组表示「匹配任意模型」。channels.model 保留为「主模型」（数组第一个），
-- 兼容旧逻辑（KB 问答默认模型、展示）与回滚。
ALTER TABLE channels ADD COLUMN models TEXT NOT NULL DEFAULT '';

-- 回填：已有单模型渠道转换为单元素数组（json_array 由 SQLite 内置 JSON1 提供）
UPDATE channels SET models = json_array(model) WHERE model != '';
