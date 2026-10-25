import { invoke } from "@tauri-apps/api/core";
import type {
  ApiKey,
  Channel,
  ChannelBalance,
  ChannelStatus,
  ChannelType,
  FetchedModelPrices,
  ModelPrice,
  RequestLog,
  RequestDetail,
  SecurityLog,
  SecurityRule,
  ServiceInfo,
  KnowledgeBase,
  KbStatus,
  KbDocument,
  KbChunk,
  KbTask,
  SearchHit,
  KbSearchMode,
  KbSession,
  KbMessage,
  ChatReply,
  EvalReport,
  McpStatus,
  McpToolInfo,
  McpCall,
  McpCallResult,
} from "../types";

// 后端命令返回的字段是 camelCase；这里映射回前端结构。
const mapChannel = (c: RawChannel): Channel => ({
  id: c.id,
  name: c.name,
  type: c.type as ChannelType,
  protocol: (c.protocol ?? "") as Channel["protocol"],
  compatReasoningContent: c.compatReasoningContent ?? true,
  baseUrl: c.baseUrl,
  model: c.model,
  models: c.models ?? (c.model ? [c.model] : []),
  status: c.status as ChannelStatus,
  apiKeyMasked: c.apiKeyMasked,
  latencyMs: c.latencyMs ?? null,
  priority: c.priority,
  weight: c.weight,
  pricePer1kTokens: c.pricePer1kTokens,
  cachePricePer1kTokens: c.cachePricePer1kTokens,
  reasoningPricePer1kTokens: c.reasoningPricePer1kTokens,
  modelPrices: c.modelPrices ?? {},
  createdAt: c.createdAt,
});

const mapKey = (k: RawKey): ApiKey => ({
  id: k.id,
  name: k.name,
  // key_prefix 已是含前缀的完整开头（如 sk-llmapi-1a2b...），直接展示，不再重复拼接
  key: k.keyPrefix,
  status: k.status as ApiKey["status"],
  quotaLimit: k.quotaLimit,
  usedTokens: k.usedTokens,
  createdAt: k.createdAt,
  expiresAt: k.expiresAt ?? null,
});

const mapLog = (l: RawLog): RequestLog => ({
  id: l.id,
  timestamp: l.timestamp,
  channel: l.channelName ?? "",
  model: l.model,
  key: l.keyName ?? "",
  protocol: l.protocol,
  stream: l.stream,
  clientUa: l.clientUa,
  attempts: l.attempts,
  riskLevel: (l.riskLevel || "none") as RequestLog["riskLevel"],
  status: l.status,
  latencyMs: l.latencyMs,
  promptTokens: l.promptTokens,
  tokens: l.totalTokens,
  cacheHitTokens: l.cacheHitTokens,
  reasoningTokens: l.reasoningTokens,
  cost: l.cost,
});

const mapDetail = (d: RawDetail): RequestDetail => ({
  requestId: d.requestId,
  requestBody: d.requestBody,
  upstreamBody: d.upstreamBody,
  responseBody: d.responseBody,
  errorBody: d.errorBody,
});

// ---- Channels ----
export const api = {
  listChannels: async (): Promise<Channel[]> =>
    (await invoke<RawChannel[]>("list_channels")).map(mapChannel),

  createChannel: async (input: {
    name: string;
    type: string;
    protocol: string;
    compatReasoningContent: boolean;
    baseUrl: string;
    models: string[];
    apiKey: string;
    status: string;
    priority: number;
    weight: number;
    pricePer1kTokens: number;
    cachePricePer1kTokens: number;
    reasoningPricePer1kTokens: number;
    modelPrices?: Record<string, ModelPrice>;
  }): Promise<Channel> => {
    const c = await invoke<RawChannel>("create_channel", { ...input });
    return mapChannel(c);
  },

  updateChannel: async (input: {
    id: string;
    name: string;
    type: string;
    protocol: string;
    compatReasoningContent: boolean;
    baseUrl: string;
    models: string[];
    status: string;
    apiKey: string;
    priority: number;
    weight: number;
    pricePer1kTokens: number;
    cachePricePer1kTokens: number;
    reasoningPricePer1kTokens: number;
    modelPrices?: Record<string, ModelPrice>;
  }): Promise<void> => invoke("update_channel", { ...input }),

  deleteChannel: (id: string) => invoke("delete_channel", { id }),
  toggleChannelStatus: (id: string) => invoke("toggle_channel_status", { id }),
  testChannel: (id: string) =>
    invoke<{
      ok: boolean;
      latencyMs: number | null;
      message: string;
      model: string;
      code: string | null;
      protocol: string;
    }>("test_channel", { id }),
  /** 拉取上游模型列表（添加/编辑渠道时选模型用；apiKey/channelId 按需提供） */
  fetchChannelModels: (input: { baseUrl?: string; apiKey?: string; channelId?: string }) =>
    invoke<{ models: string[]; url: string }>("fetch_channel_models", { ...input }),

  /** 查询渠道余额（目前仅 DeepSeek；其他厂商返回 supported=false） */
  fetchChannelBalance: (id: string) => invoke<ChannelBalance>("fetch_channel_balance", { id }),

  /** 同步 DeepSeek 官方模型价格（官方无价格 API：抓文档页，失败回退内置目录） */
  fetchDeepseekPrices: () => invoke<FetchedModelPrices>("fetch_deepseek_prices"),

  // ---- Keys ----
  listKeys: async (): Promise<ApiKey[]> =>
    (await invoke<RawKey[]>("list_keys")).map(mapKey),

  createKey: (input: { name: string; quotaLimit: number; expiresAt: string | null }) =>
    invoke<RawCreatedKey>("create_key", { ...input }),

  updateKey: (input: { id: string; name: string; quotaLimit: number; expiresAt: string | null }) =>
    invoke("update_key", { ...input }),

  rotateKey: (id: string) => invoke<RawCreatedKey>("rotate_key", { id }),

  deleteKey: (id: string) => invoke("delete_key", { id }),
  toggleKeyStatus: (id: string) => invoke("toggle_key_status", { id }),

  // ---- Requests ----
  listRequests: (input: { keyword?: string; channel?: string; model?: string; risk?: string }) =>
    invoke<RawLog[]>("list_requests", { ...input }).then((rows) => rows.map(mapLog)),
  insertRequest: (input: Record<string, unknown>) => invoke("insert_request", { ...input }),
  getRequestDetail: (id: string) =>
    invoke<RawDetail | null>("get_request_detail", { id }).then((d) => (d ? mapDetail(d) : null)),

  // ---- Security logs ----
  listSecurityLogs: (input?: { requestId?: string; risk?: string; stage?: string }) =>
    invoke<RawSecurityLog[]>("list_security_logs", {
      requestId: input?.requestId ?? null,
      risk: input?.risk ?? null,
      stage: input?.stage ?? null,
    }).then((rows) =>
      rows.map((l) => ({
        id: l.id,
        requestId: l.requestId,
        stage: l.stage as SecurityLog["stage"],
        ruleId: l.ruleId,
        category: l.category,
        risk: l.risk as SecurityLog["risk"],
        snippetMasked: l.snippetMasked,
        action: l.action as SecurityLog["action"],
        createdAt: l.createdAt,
      })),
    ),

  // ---- Security rules ----
  listSecurityRules: () => invoke<RawSecurityRule[]>("list_security_rules").then((rows) =>
    rows.map((r) => ({
      id: r.id,
      category: r.category,
      risk: r.risk as SecurityRule["risk"],
      pattern: r.pattern,
      matchType: r.matchType as SecurityRule["matchType"],
      enabled: r.enabled,
      action: r.action as SecurityRule["action"],
      createdAt: r.createdAt,
    })),
  ),
  addSecurityRule: (input: { category: string; risk: string; pattern: string; action: string; matchType: string }) =>
    invoke("add_security_rule", { ...input }),
  updateSecurityRule: (input: { id: string; category: string; risk: string; pattern: string; action: string; matchType: string }) =>
    invoke("update_security_rule", { ...input }),
  toggleSecurityRule: (id: string) => invoke("toggle_security_rule", { id }),
  setSecurityRulesEnabled: (ids: string[], enabled: boolean) =>
    invoke("set_security_rules_enabled", { ids, enabled }),
  deleteSecurityRule: (id: string) => invoke("delete_security_rule", { id }),

  // ---- Audit (auth events / admin audits) ----
  listAuthEvents: (limit?: number) => invoke<RawAuthEvent[]>("list_auth_events", { limit: limit ?? 100 }).then((rows) =>
    rows.map((e) => ({
      id: e.id,
      keyId: e.keyId,
      success: e.success,
      reason: e.reason,
      createdAt: e.createdAt,
    })),
  ),
  listAdminAudits: (limit?: number) =>
    invoke<RawAdminAudit[]>("list_admin_audits", { limit: limit ?? 100 }).then((rows) =>
      rows.map((a) => ({
        id: a.id,
        action: a.action,
        targetType: a.targetType,
        targetId: a.targetId,
        detail: a.detail,
        createdAt: a.createdAt,
      })),
    ),
  securityStats: (days: number) => invoke<SecurityStats>("security_stats", { days }),

  // ---- Settings ----
  listSettings: () => invoke<{ key: string; value: string }[]>("list_settings"),
  getSetting: (key: string) => invoke<string | null>("get_setting", { key }),
  setSetting: (key: string, value: string) => invoke("set_setting", { key, value }),
  settingsFileInfo: () =>
    invoke<{ path: string; exists: boolean }>("settings_file_info"),
  reloadSettingsFile: () => invoke<number>("reload_settings_file"),

  // ---- Services（扩展服务状态管理）----
  // 后端返回字段已是 camelCase，与 ServiceInfo 同构，无需映射
  listServices: () => invoke<ServiceInfo[]>("list_services"),
  setServiceEnabled: (id: string, enabled: boolean) =>
    invoke("set_service_enabled", { id, enabled }),
  restartService: (id: string) => invoke("restart_service", { id }),

  // ---- Daily stats ----
  listDailyStats: (limit: number) =>
    invoke<RawDailyStat[]>("list_daily_stats", { limit }),
  todayStats: () => invoke<RawDailyStat>("today_stats"),

  // ---- Knowledge base（检索与 RAG 问答）----
  // 后端 DTO 已统一 camelCase 输出，与 types 同构，无需映射
  listBases: () => invoke<KnowledgeBase[]>("kb_list_bases"),
  createBase: (input: { name: string; description?: string; chunkSize?: number; chunkOverlap?: number }) =>
    invoke<KnowledgeBase>("kb_create_base", { ...input }),
  updateBase: (input: {
    id: string;
    name?: string;
    description?: string;
    chunkSize?: number;
    chunkOverlap?: number;
    status?: KbStatus;
  }) => invoke<KnowledgeBase>("kb_update_base", { ...input }),
  deleteBase: (id: string) => invoke("kb_delete_base", { id }),

  listDocuments: (kbId: string) => invoke<KbDocument[]>("kb_list_documents", { kbId }),
  importText: (input: { kbId: string; title: string; format: string; text: string }) =>
    invoke<KbDocument>("kb_import_text", { ...input }),
  importFile: (input: { kbId: string; path: string }) =>
    invoke<KbDocument>("kb_import_file", { ...input }),
  deleteDocument: (documentId: string) => invoke("kb_delete_document", { documentId }),
  listChunks: (documentId: string) => invoke<KbChunk[]>("kb_list_chunks", { documentId }),

  bindEmbedding: (input: { kbId: string; channelId: string; model: string }) =>
    invoke<KnowledgeBase>("kb_bind_embedding", { ...input }),
  unbindEmbedding: (kbId: string) => invoke<KnowledgeBase>("kb_unbind_embedding", { kbId }),
  buildIndex: (kbId: string) => invoke<string>("kb_build_index", { kbId }),
  listTasks: (kbId: string, limit?: number) =>
    invoke<KbTask[]>("kb_list_tasks", { kbId, limit: limit ?? 20 }),

  search: (input: { kbId: string; query: string; mode?: KbSearchMode; topK?: number }) =>
    invoke<SearchHit[]>("kb_search", { ...input }),

  bindAnswer: (input: { kbId: string; channelId: string; model?: string }) =>
    invoke<KnowledgeBase>("kb_bind_answer", { ...input }),
  unbindAnswer: (kbId: string) => invoke<KnowledgeBase>("kb_unbind_answer", { kbId }),

  createSession: (input: { kbId: string; title?: string }) =>
    invoke<KbSession>("kb_create_session", { ...input }),
  listSessions: (kbId: string) => invoke<KbSession[]>("kb_list_sessions", { kbId }),
  deleteSession: (sessionId: string) => invoke("kb_delete_session", { sessionId }),
  listMessages: (sessionId: string) => invoke<KbMessage[]>("kb_list_messages", { sessionId }),
  chat: (input: { sessionId: string; question: string }) =>
    invoke<ChatReply>("kb_chat", { ...input }),

  eval: (input: {
    kbId: string;
    casesPath: string;
    mode?: KbSearchMode;
    topK?: number;
  }) => invoke<EvalReport>("kb_eval", { ...input }),
};

// ---- MCP 服务管理 ----

export const mcpApi = {
  status: () => invoke<McpStatus>("mcp_status"),
  listTools: () => invoke<McpToolInfo[]>("mcp_list_tools"),
  setToolEnabled: (name: string, enabled: boolean) =>
    invoke<void>("mcp_set_tool_enabled", { name, enabled }),
  saveConfig: (input: { host: string; port: number; readOnly: boolean }) =>
    invoke<boolean>("mcp_save_config", { ...input }),
  rotateToken: () => invoke<string>("mcp_rotate_token"),
  clearToken: () => invoke<void>("mcp_clear_token"),
  listCalls: (limit?: number) => invoke<McpCall[]>("mcp_list_calls", { limit: limit ?? 50 }),
  callTool: (name: string, args: unknown) =>
    invoke<McpCallResult>("mcp_call_tool", { name, args }),
};

// ---- raw 类型（与后端 camelCase 对齐）----
interface RawChannel {
  id: string;
  name: string;
  type: string;
  protocol: string;
  compatReasoningContent: boolean;
  baseUrl: string;
  model: string;
  models: string[];
  status: string;
  apiKeyMasked: string;
  latencyMs: number | null;
  priority: number;
  weight: number;
  pricePer1kTokens: number;
  cachePricePer1kTokens: number;
  reasoningPricePer1kTokens: number;
  modelPrices?: Record<string, ModelPrice>;
  createdAt: string;
}
interface RawKey {
  id: string;
  name: string;
  keyPrefix: string;
  status: string;
  quotaLimit: number;
  usedTokens: number;
  expiresAt: string | null;
  createdAt: string;
}
interface RawCreatedKey {
  id: string;
  name: string;
  key: string;
  keyPrefix: string;
  quotaLimit: number;
  expiresAt: string | null;
  createdAt: string;
}
interface RawLog {
  id: string;
  timestamp: string;
  channelId: string | null;
  channelName: string | null;
  keyId: string | null;
  keyName: string | null;
  model: string;
  protocol: string;
  stream: boolean;
  clientUa: string;
  attempts: number;
  riskLevel: string;
  status: number;
  latencyMs: number;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  cacheHitTokens: number;
  reasoningTokens: number;
  cost: number;
  error: string | null;
}
interface RawDetail {
  requestId: string;
  requestBody: string | null;
  upstreamBody: string | null;
  responseBody: string | null;
  errorBody: string | null;
}
interface RawDailyStat {
  date: string;
  requests: number;
  tokens: number;
  cacheHitTokens: number;
  completionTokens: number;
  cost: number;
  avgLatencyMs: number;
}
interface RawSecurityLog {
  id: string;
  requestId: string;
  stage: string;
  ruleId: string;
  category: string;
  risk: string;
  snippetMasked: string;
  action: string;
  createdAt: string;
}
interface RawSecurityRule {
  id: string;
  category: string;
  risk: string;
  pattern: string;
  matchType: string;
  enabled: boolean;
  action: string;
  createdAt: string;
}
interface RawAuthEvent {
  id: string;
  keyId: string | null;
  success: boolean;
  reason: string | null;
  createdAt: string;
}
interface RawAdminAudit {
  id: string;
  action: string;
  targetType: string;
  targetId: string | null;
  detail: string | null;
  createdAt: string;
}
/** security_stats 聚合结果（元组 = [分组值, 数量]） */
interface SecurityStats {
  byRisk: [string, number][];
  byStage: [string, number][];
  byAction: [string, number][];
  daily: [string, number][];
}
