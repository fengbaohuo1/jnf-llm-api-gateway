import { invoke } from "@tauri-apps/api/core";
import type {
  ApiKey,
  Channel,
  ChannelStatus,
  ChannelType,
  RequestLog,
  SecurityLog,
  SecurityRule,
} from "../types";

// 后端命令返回的字段是 camelCase；这里映射回前端结构。
const mapChannel = (c: RawChannel): Channel => ({
  id: c.id,
  name: c.name,
  type: c.type as ChannelType,
  baseUrl: c.baseUrl,
  model: c.model,
  status: c.status as ChannelStatus,
  apiKeyMasked: c.apiKeyMasked,
  latencyMs: c.latencyMs ?? null,
  priority: c.priority,
  weight: c.weight,
  pricePer1kTokens: c.pricePer1kTokens,
  cachePricePer1kTokens: c.cachePricePer1kTokens,
  reasoningPricePer1kTokens: c.reasoningPricePer1kTokens,
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
  channel: l.channelName,
  model: l.model,
  key: l.keyName ?? "",
  status: l.status,
  latencyMs: l.latencyMs,
  tokens: l.totalTokens,
  cacheHitTokens: l.cacheHitTokens,
  reasoningTokens: l.reasoningTokens,
  cost: l.cost,
});

// ---- Channels ----
export const api = {
  listChannels: async (): Promise<Channel[]> =>
    (await invoke<RawChannel[]>("list_channels")).map(mapChannel),

  createChannel: async (input: {
    name: string;
    type: string;
    baseUrl: string;
    model: string;
    apiKey: string;
    status: string;
    priority: number;
    weight: number;
    pricePer1kTokens: number;
    cachePricePer1kTokens: number;
    reasoningPricePer1kTokens: number;
  }): Promise<Channel> => {
    const c = await invoke<RawChannel>("create_channel", { ...input });
    return mapChannel(c);
  },

  updateChannel: async (input: {
    id: string;
    name: string;
    type: string;
    baseUrl: string;
    model: string;
    status: string;
    apiKey: string;
    priority: number;
    weight: number;
    pricePer1kTokens: number;
    cachePricePer1kTokens: number;
    reasoningPricePer1kTokens: number;
  }): Promise<void> => invoke("update_channel", { ...input }),

  deleteChannel: (id: string) => invoke("delete_channel", { id }),
  toggleChannelStatus: (id: string) => invoke("toggle_channel_status", { id }),
  testChannel: (id: string) =>
    invoke<{ ok: boolean; latencyMs: number | null; message: string; model: string }>("test_channel", { id }),

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
  listRequests: (input: { keyword?: string; channel?: string; model?: string }) =>
    invoke<RawLog[]>("list_requests", { ...input }).then((rows) => rows.map(mapLog)),
  insertRequest: (input: Record<string, unknown>) => invoke("insert_request", { ...input }),

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

  // ---- Daily stats ----
  listDailyStats: (limit: number) =>
    invoke<RawDailyStat[]>("list_daily_stats", { limit }),
  todayStats: () => invoke<RawDailyStat>("today_stats"),
};

// ---- raw 类型（与后端 camelCase 对齐）----
interface RawChannel {
  id: string;
  name: string;
  type: string;
  baseUrl: string;
  model: string;
  status: string;
  apiKeyMasked: string;
  latencyMs: number | null;
  priority: number;
  weight: number;
  pricePer1kTokens: number;
  cachePricePer1kTokens: number;
  reasoningPricePer1kTokens: number;
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
  channelId: string;
  channelName: string;
  keyId: string | null;
  keyName: string | null;
  model: string;
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
