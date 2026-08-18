import type {
  ApiKey,
  Channel,
  ChannelType,
  DashboardStats,
  RequestLog,
  UsagePoint,
} from "../types";

// ---- Dashboard ----
export const dashboardStats: DashboardStats = {
  todayRequests: 128_430,
  todayTokens: 4_852_319,
  activeChannels: 12,
  avgLatencyMs: 812,
  trend: { requests: 12.4, tokens: 8.1 },
};

export const usagePoints: UsagePoint[] = [
  { label: "00:00", requests: 1200, tokens: 40_000 },
  { label: "04:00", requests: 800, tokens: 26_000 },
  { label: "08:00", requests: 5400, tokens: 210_000 },
  { label: "12:00", requests: 9800, tokens: 410_000 },
  { label: "16:00", requests: 7200, tokens: 300_000 },
  { label: "20:00", requests: 6100, tokens: 245_000 },
];

// ---- Channels ----
export const channels: Channel[] = [
  { id: "ch_1", name: "OpenAI Main", type: "OpenAI", baseUrl: "https://api.openai.com/v1", model: "gpt-4o", status: "active", apiKeyMasked: "sk-proj-••••1234", latencyMs: 420, createdAt: "2025-01-12" },
  { id: "ch_2", name: "DeepSeek Pro", type: "DeepSeek", baseUrl: "https://api.deepseek.com/v1", model: "deepseek-chat", status: "active", apiKeyMasked: "sk-••••78ab", latencyMs: 380, createdAt: "2025-02-03" },
  { id: "ch_3", name: "Claude Studio", type: "Claude", baseUrl: "https://api.anthropic.com/v1", model: "claude-3-5-sonnet", status: "active", apiKeyMasked: "sk-ant-••••9cde", latencyMs: 890, createdAt: "2025-02-21" },
  { id: "ch_4", name: "Gemini Flash", type: "Gemini", baseUrl: "https://generativelanguage.googleapis.com/v1beta", model: "gemini-1.5-flash", status: "active", apiKeyMasked: "AIza••••fgh1", latencyMs: 640, createdAt: "2025-03-05" },
  { id: "ch_5", name: "OpenAI Legacy", type: "OpenAI", baseUrl: "https://api.openai.com/v1", model: "gpt-3.5-turbo", status: "disabled", apiKeyMasked: "sk-proj-••••5678", latencyMs: null, createdAt: "2025-01-02" },
  { id: "ch_6", name: "DeepSeek Reasoner", type: "DeepSeek", baseUrl: "https://api.deepseek.com/v1", model: "deepseek-reasoner", status: "error", apiKeyMasked: "sk-••••abcd", latencyMs: 2400, createdAt: "2025-03-18" },
  { id: "ch_7", name: "Claude Haiku", type: "Claude", baseUrl: "https://api.anthropic.com/v1", model: "claude-3-5-haiku", status: "active", apiKeyMasked: "sk-ant-••••2468", latencyMs: 520, createdAt: "2025-04-01" },
];

export const channelTypes: ChannelType[] = ["OpenAI", "DeepSeek", "Claude", "Gemini", "Custom"];

// ---- Keys ----
export const apiKeys: ApiKey[] = [
  { id: "k_1", name: "Production", key: "sk-waliapi-1a2b3c4d5e6f7g8h9i0j", status: "active", quotaLimit: -1, usedTokens: 1_240_000, createdAt: "2025-01-15", expiresAt: null },
  { id: "k_2", name: "Staging", key: "sk-waliapi-0f9e8d7c6b5a4f3e2d1c", status: "active", quotaLimit: 1_000_000, usedTokens: 612_000, createdAt: "2025-02-10", expiresAt: "2025-12-31" },
  { id: "k_3", name: "Internal tool", key: "sk-waliapi-11223344556677889900", status: "disabled", quotaLimit: 100_000, usedTokens: 100_000, createdAt: "2025-03-02", expiresAt: "2025-06-30" },
  { id: "k_4", name: "CI pipeline", key: "sk-waliapi-abcdef0123456789fedc", status: "active", quotaLimit: 500_000, usedTokens: 128_000, createdAt: "2025-04-11", expiresAt: null },
];

// ---- Logs ----
export const requestLogs: RequestLog[] = [
  { id: "req_90001", timestamp: "2025-04-15 22:41:03", channel: "OpenAI Main", model: "gpt-4o", key: "sk-waliapi-1a2b", status: 200, latencyMs: 421, tokens: 1842, cost: 0.041 },
  { id: "req_90002", timestamp: "2025-04-15 22:40:58", channel: "DeepSeek Pro", model: "deepseek-chat", key: "sk-waliapi-0f9e", status: 200, latencyMs: 366, tokens: 512, cost: 0.006 },
  { id: "req_90003", timestamp: "2025-04-15 22:40:41", channel: "Claude Studio", model: "claude-3-5-sonnet", key: "sk-waliapi-1a2b", status: 429, latencyMs: 2100, tokens: 0, cost: 0 },
  { id: "req_90004", timestamp: "2025-04-15 22:40:27", channel: "Gemini Flash", model: "gemini-1.5-flash", key: "sk-waliapi-1122", status: 200, latencyMs: 618, tokens: 931, cost: 0.004 },
  { id: "req_90005", timestamp: "2025-04-15 22:39:59", channel: "OpenAI Main", model: "gpt-3.5-turbo", key: "sk-waliapi-abcdef", status: 401, latencyMs: 85, tokens: 0, cost: 0 },
  { id: "req_90006", timestamp: "2025-04-15 22:39:37", channel: "DeepSeek Reasoner", model: "deepseek-reasoner", key: "sk-waliapi-0f9e", status: 500, latencyMs: 3240, tokens: 0, cost: 0 },
  { id: "req_90007", timestamp: "2025-04-15 22:39:12", channel: "Claude Haiku", model: "claude-3-5-haiku", key: "sk-waliapi-1a2b", status: 200, latencyMs: 501, tokens: 740, cost: 0.004 },
  { id: "req_90008", timestamp: "2025-04-15 22:38:56", channel: "OpenAI Main", model: "gpt-4o", key: "sk-waliapi-1122", status: 200, latencyMs: 455, tokens: 2103, cost: 0.048 },
  { id: "req_90009", timestamp: "2025-04-15 22:38:20", channel: "DeepSeek Pro", model: "deepseek-reasoner", key: "sk-waliapi-abcdef", status: 200, latencyMs: 1880, tokens: 1590, cost: 0.027 },
  { id: "req_90010", timestamp: "2025-04-15 22:37:44", channel: "Gemini Flash", model: "gemini-1.5-flash", key: "sk-waliapi-1a2b", status: 403, latencyMs: 220, tokens: 0, cost: 0 },
];
