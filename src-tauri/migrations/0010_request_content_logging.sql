-- 0010 请求内容记录：requests 概要加上下文列（channel 可空以记录渠道未定的失败请求），
--     另建 request_details 明细表（请求/转发/响应/错误完整 JSON，按需单行加载，列表查询不拖大字段）。
--
-- 实现说明：SQLite 不能直接去掉列的 NOT NULL 或改列序，需重建表；被 FK 引用的父表不能直接
-- DROP（security_logs.request_id → requests.id），因此先把 security_logs 重建为不带外键的
-- 版本（引用一致性由应用层保证），再重命名式重建 requests。

-- ① security_logs 重建（去掉指向 requests 的外键；级联由应用层删除或保留孤儿行）
CREATE TABLE security_logs_new (
    id             TEXT PRIMARY KEY,
    request_id     TEXT NOT NULL,
    stage          TEXT NOT NULL,
    rule_id        TEXT NOT NULL,
    category       TEXT NOT NULL,
    risk           TEXT NOT NULL,
    snippet_masked TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    action         TEXT NOT NULL DEFAULT 'detected'
);
INSERT INTO security_logs_new (id, request_id, stage, rule_id, category, risk, snippet_masked, created_at, action)
SELECT id, request_id, stage, rule_id, category, risk, snippet_masked, created_at, action FROM security_logs;
DROP TABLE security_logs;
ALTER TABLE security_logs_new RENAME TO security_logs;
CREATE INDEX IF NOT EXISTS idx_security_logs_request ON security_logs(request_id);

-- ② requests 重建：channel_id/channel_name 允许 NULL（调度前失败渠道未定）、
--    新增 protocol / stream / client_ua / attempts
ALTER TABLE requests RENAME TO requests_old;

CREATE TABLE requests_new (
    id                TEXT PRIMARY KEY,
    timestamp         TEXT NOT NULL,
    channel_id        TEXT,
    channel_name      TEXT,
    key_id            TEXT,
    key_name          TEXT,
    model             TEXT NOT NULL DEFAULT '',
    protocol          TEXT NOT NULL DEFAULT 'openai',
    stream            INTEGER NOT NULL DEFAULT 0,
    client_ua         TEXT NOT NULL DEFAULT '',
    attempts          INTEGER NOT NULL DEFAULT 1,
    status            INTEGER NOT NULL,
    latency_ms        INTEGER NOT NULL DEFAULT 0,
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens      INTEGER NOT NULL DEFAULT 0,
    cache_hit_tokens  INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens  INTEGER NOT NULL DEFAULT 0,
    cost              REAL NOT NULL DEFAULT 0,
    error             TEXT,
    FOREIGN KEY (channel_id) REFERENCES channels(id),
    FOREIGN KEY (key_id) REFERENCES keys(id)
);

INSERT INTO requests_new (id, timestamp, channel_id, channel_name, key_id, key_name, model, status, latency_ms, prompt_tokens, completion_tokens, total_tokens, cache_hit_tokens, reasoning_tokens, cost, error)
SELECT id, timestamp, channel_id, channel_name, key_id, key_name, model, status, latency_ms, prompt_tokens, completion_tokens, total_tokens, cache_hit_tokens, reasoning_tokens, cost, error FROM requests_old;

DROP TABLE requests_old;
ALTER TABLE requests_new RENAME TO requests;

CREATE INDEX IF NOT EXISTS idx_requests_timestamp ON requests(timestamp);
CREATE INDEX IF NOT EXISTS idx_requests_channel  ON requests(channel_id);
CREATE INDEX IF NOT EXISTS idx_requests_key      ON requests(key_id);
CREATE INDEX IF NOT EXISTS idx_requests_model    ON requests(model);

-- ③ 内容明细表：一对一行挂在 requests 下（大字段独立存放，按需加载；删除请求时级联删除明细）
CREATE TABLE IF NOT EXISTS request_details (
    request_id    TEXT PRIMARY KEY,
    -- 收到的原始请求体（客户端协议原文，经全量脱敏后落库，保证日志不成为泄密通道）
    request_body  TEXT NOT NULL DEFAULT '',
    -- 实际发给上游的请求体（内部 OpenAI 格式：脱敏 + 协议转换 + stream_options 注入后）
    upstream_body TEXT NOT NULL DEFAULT '',
    -- 成功响应体（非流式 = 上游响应原文；流式 = 由 SSE delta 重建的完整响应）
    response_body TEXT NOT NULL DEFAULT '',
    -- 失败响应体（上游返回的错误 JSON / 网关生成的错误 JSON）
    error_body    TEXT NOT NULL DEFAULT '',
    FOREIGN KEY (request_id) REFERENCES requests(id) ON DELETE CASCADE
);
