-- 0003 转发代理与安全模块：渠道单价 + 安全事件日志
ALTER TABLE channels ADD COLUMN price_per_1k_tokens REAL NOT NULL DEFAULT 0;

-- 安全事件日志：一次请求的请求侧/响应侧扫描发现，一对多（一个请求可能命中多条规则）
CREATE TABLE IF NOT EXISTS security_logs (
    id             TEXT PRIMARY KEY,
    request_id     TEXT NOT NULL,
    stage          TEXT NOT NULL,          -- request / response
    rule_id        TEXT NOT NULL,
    category       TEXT NOT NULL,          -- secret / pii / prompt_injection
    risk           TEXT NOT NULL,          -- low / medium / high
    snippet_masked TEXT NOT NULL,          -- 脱敏后的命中片段（展示用，不存原文）
    created_at     TEXT NOT NULL,
    FOREIGN KEY (request_id) REFERENCES requests(id)
);
CREATE INDEX IF NOT EXISTS idx_security_logs_request ON security_logs(request_id);
