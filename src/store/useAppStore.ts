import { create } from "zustand";
import { api } from "../api";
import type { ApiKey, Channel, RequestLog, ChannelType, ChannelStatus } from "../types";

interface AppState {
  channels: Channel[];
  keys: ApiKey[];
  logs: RequestLog[];
  loadingChannels: boolean;
  loadingKeys: boolean;
  loadingLogs: boolean;

  refreshChannels: () => Promise<void>;
  refreshKeys: () => Promise<void>;
  refreshLogs: (filters?: { keyword?: string; channel?: string; model?: string }) => Promise<void>;

  addChannel: (input: {
    name: string;
    type: ChannelType;
    baseUrl: string;
    model: string;
    apiKey: string;
    priority: number;
    weight: number;
  }) => Promise<void>;
  updateChannel: (input: {
    id: string;
    name: string;
    type: ChannelType;
    baseUrl: string;
    model: string;
    status: ChannelStatus;
    apiKey: string;
    priority: number;
    weight: number;
  }) => Promise<void>;
  removeChannel: (id: string) => Promise<void>;
  toggleChannelStatus: (id: string) => Promise<void>;
  testChannel: (id: string) => Promise<{ ok: boolean; latencyMs: number | null; message: string; model: string }>;

  createKey: (input: { name: string; quotaLimit: number; expiresAt: string | null }) => Promise<{
    id: string;
    key: string;
  }>;
  removeKey: (id: string) => Promise<void>;
  toggleKeyStatus: (id: string) => Promise<void>;
}

async function safe<T>(fn: () => Promise<T>): Promise<T> {
  try {
    return await fn();
  } catch (e) {
    throw new Error(e instanceof Error ? e.message : String(e));
  }
}

export const useAppStore = create<AppState>((set, get) => ({
  channels: [],
  keys: [],
  logs: [],
  loadingChannels: false,
  loadingKeys: false,
  loadingLogs: false,

  refreshChannels: async () => {
    set({ loadingChannels: true });
    try {
      const channels = await safe(api.listChannels);
      set({ channels, loadingChannels: false });
    } catch (e) {
      set({ loadingChannels: false });
      console.error(e);
    }
  },

  refreshKeys: async () => {
    set({ loadingKeys: true });
    try {
      const keys = await safe(api.listKeys);
      set({ keys, loadingKeys: false });
    } catch (e) {
      set({ loadingKeys: false });
      console.error(e);
    }
  },

  refreshLogs: async (filters) => {
    set({ loadingLogs: true });
    try {
      const logs = await safe(() =>
        api.listRequests({
          keyword: filters?.keyword,
          channel: filters?.channel,
          model: filters?.model,
        }),
      );
      set({ logs, loadingLogs: false });
    } catch (e) {
      set({ loadingLogs: false });
      console.error(e);
    }
  },

  addChannel: async (input) => {
    await safe(() => api.createChannel({ ...input, status: "active" }));
    await get().refreshChannels();
  },
  updateChannel: async (input) => {
    await safe(() => api.updateChannel(input));
    await get().refreshChannels();
  },

  removeChannel: async (id) => {
    await safe(() => api.deleteChannel(id));
    await get().refreshChannels();
  },

  toggleChannelStatus: async (id) => {
    await safe(() => api.toggleChannelStatus(id));
    await get().refreshChannels();
  },

  testChannel: async (id) => {
    const result = await safe(() => api.testChannel(id));
    await get().refreshChannels();
    return result;
  },

  createKey: async (input) => {
    const created = await safe(() => api.createKey(input));
    await get().refreshKeys();
    return { id: created.id, key: created.key };
  },

  removeKey: async (id) => {
    await safe(() => api.deleteKey(id));
    await get().refreshKeys();
  },

  toggleKeyStatus: async (id) => {
    await safe(() => api.toggleKeyStatus(id));
    await get().refreshKeys();
  },
}));
