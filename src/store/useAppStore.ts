import { create } from "zustand";
import type { ApiKey, Channel, RequestLog } from "../types";
import { apiKeys as initialKeys, channels as initialChannels, requestLogs } from "../data/mock";

// Zod-less simple id generator for the mock UI.
export function generateId(prefix: string): string {
  return `${prefix}_${Math.random().toString(36).slice(2, 10)}`;
}

interface AppState {
  channels: Channel[];
  keys: ApiKey[];
  logs: RequestLog[];

  addChannel: (channel: Omit<Channel, "id" | "apiKeyMasked" | "latencyMs" | "createdAt">) => void;
  updateChannel: (id: string, patch: Partial<Channel>) => void;
  removeChannel: (id: string) => void;
  toggleChannelStatus: (id: string) => void;
  testChannel: (id: string) => void;

  addKey: (key: Omit<ApiKey, "id" | "createdAt" | "usedTokens">) => void;
  updateKey: (id: string, patch: Partial<ApiKey>) => void;
  removeKey: (id: string) => void;
  toggleKeyStatus: (id: string) => void;
}

export const useAppStore = create<AppState>((set) => ({
  channels: initialChannels,
  keys: initialKeys,
  logs: requestLogs,

  addChannel: (channel) =>
    set((s) => ({
      channels: [
        { ...channel, id: generateId("ch"), apiKeyMasked: "sk-••••new", latencyMs: null, createdAt: new Date().toISOString().slice(0, 10) },
        ...s.channels,
      ],
    })),

  updateChannel: (id, patch) =>
    set((s) => ({
      channels: s.channels.map((c) => (c.id === id ? { ...c, ...patch } : c)),
    })),

  removeChannel: (id) =>
    set((s) => ({ channels: s.channels.filter((c) => c.id !== id) })),

  toggleChannelStatus: (id) =>
    set((s) => ({
      channels: s.channels.map((c) =>
        c.id === id ? { ...c, status: c.status === "active" ? "disabled" : "active" } : c,
      ),
    })),

  testChannel: (id) =>
    set((s) => ({
      channels: s.channels.map((c) =>
        c.id === id ? { ...c, status: "active", latencyMs: Math.round(300 + Math.random() * 600) } : c,
      ),
    })),

  addKey: (key) =>
    set((s) => ({
      keys: [
        { ...key, id: generateId("k"), createdAt: new Date().toISOString().slice(0, 10), usedTokens: 0 },
        ...s.keys,
      ],
    })),

  updateKey: (id, patch) =>
    set((s) => ({ keys: s.keys.map((k) => (k.id === id ? { ...k, ...patch } : k)) })),

  removeKey: (id) => set((s) => ({ keys: s.keys.filter((k) => k.id !== id) })),

  toggleKeyStatus: (id) =>
    set((s) => ({
      keys: s.keys.map((k) => (k.id === id ? { ...k, status: k.status === "active" ? "disabled" : "active" } : k)),
    })),
}));
