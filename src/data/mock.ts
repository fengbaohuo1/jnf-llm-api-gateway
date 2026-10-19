import type { ChannelType } from "../types";

// 渠道类型的可选枚举（用于创建/编辑渠道表单下拉）。
export const channelTypes: ChannelType[] = ["OpenAI", "DeepSeek", "Claude", "Gemini", "Custom"];

/**
 * 常见厂商的 Base URL 与常用模型预设（添加渠道时一键填地址 / 点选模型）。
 * 模型列表只是便捷提示，实际可用性以上游 `/models` 与账号权限为准。
 */
export const providerPresets: Record<ChannelType, { baseUrl: string; models: string[] }> = {
  OpenAI: {
    baseUrl: "https://api.openai.com/v1",
    models: ["gpt-4o", "gpt-4o-mini", "gpt-4.1", "gpt-4.1-mini", "o3-mini"],
  },
  DeepSeek: {
    baseUrl: "https://api.deepseek.com",
    models: ["deepseek-chat", "deepseek-reasoner"],
  },
  // Claude / Gemini 的原生适配器尚未实现；地址仅作 OpenAI 兼容层提示（类型建议选 Custom）
  Claude: { baseUrl: "https://api.anthropic.com/v1", models: ["claude-sonnet-4-5"] },
  Gemini: {
    baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
    models: ["gemini-2.5-pro", "gemini-2.5-flash"],
  },
  Custom: { baseUrl: "", models: [] },
};
