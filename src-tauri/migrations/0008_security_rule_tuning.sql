-- 0008 规则校准：收紧 base64 与凭据赋值规则的误报面
-- （迁移不可变：0007 种子保持原样，此处通过新迁移修正规则内容）
UPDATE security_rules SET pattern = '[A-Za-z0-9+/]{48,}[+/][A-Za-z0-9+/]{12,}={0,2}|[A-Za-z0-9+/]{56,}={1,2}'
 WHERE id = 'base64_blob';

UPDATE security_rules SET pattern = '(?i)\b(password|passwd|pwd|api[_-]?key|secret|auth_token|access_token)\s*[=:：]\s*[A-Za-z0-9_\-./+]{8,}'
 WHERE id = 'credential_assignment';
