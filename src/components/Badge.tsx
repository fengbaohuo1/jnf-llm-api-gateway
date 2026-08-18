import type { ChannelStatus, ChannelType } from "../types";

const typeColors: Record<ChannelType, string> = {
  OpenAI: "bg-emerald-50 text-emerald-700 ring-emerald-200",
  DeepSeek: "bg-blue-50 text-blue-700 ring-blue-200",
  Claude: "bg-amber-50 text-amber-700 ring-amber-200",
  Gemini: "bg-sky-50 text-sky-700 ring-sky-200",
  Custom: "bg-gray-100 text-gray-600 ring-gray-200",
};

export function ChannelTypeBadge({ type }: { type: ChannelType }) {
  return (
    <span className={`inline-flex items-center rounded-md px-2 py-0.5 text-xs font-medium ring-1 ${typeColors[type]}`}>
      {type}
    </span>
  );
}

const statusStyles: Record<ChannelStatus, { label: string; cls: string; dot: string }> = {
  active: { label: "启用", cls: "bg-green-50 text-green-700 ring-green-200", dot: "bg-green-500" },
  disabled: { label: "停用", cls: "bg-gray-100 text-gray-500 ring-gray-200", dot: "bg-gray-400" },
  error: { label: "异常", cls: "bg-red-50 text-red-700 ring-red-200", dot: "bg-red-500" },
};

export function StatusBadge({ status }: { status: ChannelStatus }) {
  const s = statusStyles[status];
  return (
    <span className={`inline-flex items-center gap-1.5 rounded-md px-2 py-0.5 text-xs font-medium ring-1 ${s.cls}`}>
      <span className={`h-1.5 w-1.5 rounded-full ${s.dot}`} />
      {s.label}
    </span>
  );
}
