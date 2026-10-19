-- 0011 请求风险等级：记录每个请求（请求侧 + 响应侧）命中的最高安全风险。
-- 用途：日志按风险筛选/展示；配合 log.store_level 决定是否落库完整内容明细
--      （request_details 只有达到阈值的请求才写入，元数据与计费始终记录）。
ALTER TABLE requests ADD COLUMN risk_level TEXT NOT NULL DEFAULT 'none';

CREATE INDEX IF NOT EXISTS idx_requests_risk ON requests(risk_level);
