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
  createdAt: string;
}

export interface ApiKey {
  id: string;
  name: string;
  key: string; // sk-waliapi-...
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
