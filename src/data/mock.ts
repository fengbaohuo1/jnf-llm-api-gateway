import type { ChannelProtocol, ChannelType } from "../types";

// 渠道类型的可选枚举（用于创建/编辑渠道表单下拉）。
export const channelTypes: ChannelType[] = ["OpenAI", "DeepSeek", "Claude", "Gemini", "Custom"];

/** 渠道协议选项："" = 自动（按渠道类型推导） */
export const protocolOptions: { value: ChannelProtocol | ""; label: string }[] = [
  { value: "", label: "自动（按渠道类型）" },
  { value: "openai", label: "OpenAI 兼容" },
  { value: "deepseek", label: "DeepSeek（含思考模式兼容）" },
  { value: "anthropic", label: "Anthropic 原生（未实现）" },
  { value: "gemini", label: "Gemini 原生（未实现）" },
];

/** 按渠道类型推导协议（与后端 adapters::effective_protocol 保持一致） */
export function autoProtocol(type: ChannelType): ChannelProtocol {
  switch (type) {
    case "DeepSeek":
      return "deepseek";
    case "Claude":
      return "anthropic";
    case "Gemini":
      return "gemini";
    default:
      return "openai";
  }
}

/**
 * 常见厂商的 Base URL 与常用模型预设（添加渠道时一键填地址 / 点选模型）。
 * 模型列表只是便捷提示，实际可用性以上游 `/models` 与账号权限为准。
 * DeepSeek 不写死模型：填写 API Key 后自动调用上游 `/models` 拉取。
 */
export const providerPresets: Record<ChannelType, { baseUrl: string; models: string[] }> = {
  OpenAI: {
    baseUrl: "https://api.openai.com/v1",
    models: ["gpt-4o", "gpt-4o-mini", "gpt-4.1", "gpt-4.1-mini", "o3-mini"],
  },
  DeepSeek: {
    baseUrl: "https://api.deepseek.com",
    models: [],
  },
  // Claude / Gemini 的原生适配器尚未实现；地址仅作 OpenAI 兼容层提示（类型建议选 Custom）
  Claude: { baseUrl: "https://api.anthropic.com/v1", models: ["claude-sonnet-4-5"] },
  Gemini: {
    baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
    models: ["gemini-2.5-pro", "gemini-2.5-flash"],
  },
  Custom: { baseUrl: "", models: [] },
};
