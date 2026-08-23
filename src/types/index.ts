// ---- Shared domain types for the LLM API Gateway UI ----

export type ChannelType = "OpenAI" | "DeepSeek" | "Claude" | "Gemini" | "Custom";

export type ChannelStatus = "active" | "disabled" | "error";

export interface Channel {
  id: string;
  name: string;
  type: ChannelType;
  baseUrl: string;
  model: string;
  status: ChannelStatus;
  apiKeyMasked: string;
  latencyMs: number | null;
  priority: number; // 故障转移优先级，越大越先
  weight: number; // 同优先级权重，越大越可能先被选中
  pricePer1kTokens: number; // 每 1k token 成本（元），转发层记账用
  createdAt: string;
}

export interface ApiKey {
  id: string;
  name: string;
  key: string; // sk-llmapi-...（列表展示时为 key_prefix 开头片段）
  status: "active" | "disabled";
  quotaLimit: number; // tokens allowed (-1 = unlimited)
  usedTokens: number;
  createdAt: string;
  expiresAt: string | null;
}

export interface RequestLog {
  id: string;
  timestamp: string;
  channel: string;
  model: string;
  key: string;
  status: number;
  latencyMs: number;
  tokens: number;
  cost: number; // USD
}

export interface SecurityLog {
  id: string;
  requestId: string;
  stage: "request" | "response";
  ruleId: string;
  category: string;
  risk: "low" | "medium" | "high";
  snippetMasked: string;
  createdAt: string;
}
