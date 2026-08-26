-- 用量拆分明细：daily_stats 增加缓存命中 token 与输出（completion）token 的按日累计，
-- 供用量页区分"命中缓存 / 未命中缓存 / 输出"三部分展示。
ALTER TABLE daily_stats ADD COLUMN cache_hit_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE daily_stats ADD COLUMN completion_tokens INTEGER NOT NULL DEFAULT 0;
