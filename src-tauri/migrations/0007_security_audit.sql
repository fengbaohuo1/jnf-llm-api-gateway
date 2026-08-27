-- 0007 安全审计引擎：规则配置化 + 鉴权事件审计 + 管理操作审计
-- security_logs 增加"处置动作"列：detected / masked / blocked
ALTER TABLE security_logs ADD COLUMN action TEXT NOT NULL DEFAULT 'detected';

-- 安全规则表（可配置：类别/风险/正则/启停/处置动作），由扫描引擎按 category 分流
CREATE TABLE IF NOT EXISTS security_rules (
    id         TEXT PRIMARY KEY,
    category   TEXT NOT NULL,   -- secret / pii / prompt_injection / encoding / command
    risk       TEXT NOT NULL,   -- low / medium / high
    pattern    TEXT NOT NULL,   -- 正则表达式（入库时校验可编译）
    enabled    INTEGER NOT NULL DEFAULT 1,
    action     TEXT NOT NULL DEFAULT 'mask',  -- record（仅记录）/ mask（脱敏）/ block（拦截）
    created_at TEXT NOT NULL
);

-- 鉴权事件审计（含匿名失败，防暴力破解检测的数据基础）
CREATE TABLE IF NOT EXISTS auth_events (
    id         TEXT PRIMARY KEY,
    key_id     TEXT,            -- 失败且密钥无效时为 NULL
    success    INTEGER NOT NULL,
    reason     TEXT,            -- 失败原因：密钥无效/已禁用/已过期/配额用尽
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_auth_events_time ON auth_events(created_at);

-- 管理操作审计：渠道/密钥/设置/规则的所有变更留痕
CREATE TABLE IF NOT EXISTS admin_audit_logs (
    id          TEXT PRIMARY KEY,
    action      TEXT NOT NULL,  -- create / update / delete / toggle / rotate / set
    target_type TEXT NOT NULL,  -- channel / key / setting / security_rule
    target_id   TEXT,
    detail      TEXT,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_admin_audit_time ON admin_audit_logs(created_at);

-- 种子规则（与代码内置一致；可被规则管理界面修改/停用/新增）
INSERT INTO security_rules (id, category, risk, pattern, enabled, action, created_at) VALUES
-- 密钥/凭据泄露（secret）
('api_key_llmapi',       'secret', 'high',   'sk-llmapi-[a-z0-9]{20,}',             1, 'mask',   datetime('now','+8 hours')),
('api_key_generic',      'secret', 'high',   '\bsk-[A-Za-z0-9]{20,}\b',              1, 'mask',   datetime('now','+8 hours')),
('api_key_google',       'secret', 'high',   'AIza[0-9A-Za-z_-]{35}',                1, 'mask',   datetime('now','+8 hours')),
('api_key_github',       'secret', 'high',   'ghp_[A-Za-z0-9]{36,}',                 1, 'mask',   datetime('now','+8 hours')),
('api_key_aws',          'secret', 'high',   'AKIA[0-9A-Z]{16}',                     1, 'mask',   datetime('now','+8 hours')),
('credential_assignment','secret', 'medium', '(?i)\b(password|passwd|pwd|api[_-]?key|secret|token)\s*[=:：]\s*[^\s,;，；]+', 1, 'mask', datetime('now','+8 hours')),
-- 个人身份信息（pii）
('china_id_card',        'pii',    'high',   '\b\d{17}[\dXx]\b',                     1, 'mask',   datetime('now','+8 hours')),
('china_phone',          'pii',    'medium', '\b1[3-9]\d{9}\b',                      1, 'mask',   datetime('now','+8 hours')),
('bank_card',            'pii',    'high',   '\b\d{16,19}\b',                        1, 'mask',   datetime('now','+8 hours')),
('email',                'pii',    'low',    '\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b', 1, 'record', datetime('now','+8 hours')),
-- 提示注入（prompt_injection）
('prompt_injection',     'prompt_injection', 'medium', '(?i)(ignore|disregard|forget|忽略|无视|不要管|别管).{0,30}(previous|above|prior|earlier|instructions?|prompt|system|之前|以上|前面|指令|提示|系统)', 1, 'mask', datetime('now','+8 hours')),
('system_prompt_extract','prompt_injection', 'medium', '(?i)(reveal|show|print|output|输出|显示|泄露|告诉).{0,30}(system prompt|instructions|system instructions|系统提示|系统指令|你的指令)', 1, 'mask', datetime('now','+8 hours')),
-- 隐写/混淆（encoding）
('base64_blob',          'encoding','medium', '[A-Za-z0-9+/]{40,}={0,2}',            1, 'mask',   datetime('now','+8 hours')),
('hex_blob',             'encoding','medium', '\b[0-9a-fA-F]{32,}\b',                1, 'mask',   datetime('now','+8 hours')),
('zero_width',           'encoding','medium', '[\u200b-\u200d\u2060\ufeff]',         1, 'mask',   datetime('now','+8 hours')),
('bidi_override',        'encoding','high',   '[\u202a-\u202e]',                     1, 'mask',   datetime('now','+8 hours')),
('fullwidth_obfuscation','encoding','low',    '[Ａ-Ｚａ-ｚ０-９]',                   1, 'record', datetime('now','+8 hours')),
-- 命令注入/带毒命令（command）
('destructive_shell',    'command', 'high',   '(?i)\brm\s+-rf\b|\bmkfs\b|\bdd\s+if=.*of=/dev/|:\(\)\s*\{.*\}\s*;', 1, 'mask', datetime('now','+8 hours')),
('pipe_to_shell',        'command', 'high',   '(?i)(curl|wget)\s+[^\n|;]*\s*\|\s*(ba)?sh', 1, 'mask', datetime('now','+8 hours')),
('encoded_exec',         'command', 'high',   '(?i)(powershell|pwsh|cmd)\s+[^\n]*-enc(odedcommand)?|Invoke-Expression|\biex\b|base64\s+-d\s*\||certutil\s+-decode', 1, 'mask', datetime('now','+8 hours')),
('script_exec',          'command', 'medium', '(?i)\b(eval|exec|system|popen|os\.system|subprocess\.(run|call|Popen)|spawn)\s*\(', 1, 'mask', datetime('now','+8 hours')),
('reverse_shell',        'command', 'high',   '(?i)/dev/tcp/|nc\s+-e|ncat\s+-e|bash\s+-i\s*>&|socat\s+tcp', 1, 'mask', datetime('now','+8 hours'));
