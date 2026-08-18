-- 0001 初始表结构：LLM API 网关核心五表
CREATE TABLE IF NOT EXISTS channels (
    id                TEXT PRIMARY KEY,
    name              TEXT NOT NULL,
    type              TEXT NOT NULL,
    base_url          TEXT NOT NULL,
    model             TEXT NOT NULL DEFAULT '',
    status            TEXT NOT NULL DEFAULT 'active',
    api_key_encrypted TEXT NOT NULL DEFAULT '',
    latency_ms        INTEGER,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS keys (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    key_hash    TEXT NOT NULL UNIQUE,
    key_prefix  TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active',
    quota_limit INTEGER NOT NULL DEFAULT -1,
    used_tokens INTEGER NOT NULL DEFAULT 0,
    expires_at  TEXT,
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS requests (
    id                TEXT PRIMARY KEY,
    timestamp         TEXT NOT NULL,
    channel_id        TEXT NOT NULL,
    channel_name      TEXT NOT NULL,
    key_id            TEXT,
    key_name          TEXT,
    model             TEXT NOT NULL,
    status            INTEGER NOT NULL,
    latency_ms        INTEGER NOT NULL,
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens      INTEGER NOT NULL DEFAULT 0,
    cost              REAL NOT NULL DEFAULT 0,
    error             TEXT,
    FOREIGN KEY (channel_id) REFERENCES channels(id),
    FOREIGN KEY (key_id) REFERENCES keys(id)
);
CREATE INDEX IF NOT EXISTS idx_requests_timestamp ON requests(timestamp);
CREATE INDEX IF NOT EXISTS idx_requests_channel  ON requests(channel_id);
CREATE INDEX IF NOT EXISTS idx_requests_key      ON requests(key_id);
CREATE INDEX IF NOT EXISTS idx_requests_model    ON requests(model);

CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS daily_stats (
    date           TEXT PRIMARY KEY,
    requests       INTEGER NOT NULL DEFAULT 0,
    tokens         INTEGER NOT NULL DEFAULT 0,
    cost           REAL NOT NULL DEFAULT 0,
    avg_latency_ms REAL NOT NULL DEFAULT 0
);
