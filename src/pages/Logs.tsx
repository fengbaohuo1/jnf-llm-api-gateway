import { useEffect, useMemo, useState } from "react";
import { Search } from "lucide-react";
import { useAppStore } from "../store/useAppStore";
import { PageHeader, Card } from "../components/ui";

const fmt = (n: number) => n.toLocaleString("en-US");

function statusPill(status: number) {
  const cls =
    status >= 500
      ? "bg-red-50 text-red-700 ring-red-200 dark:bg-red-950 dark:text-red-300 dark:ring-red-900"
      : status >= 400
        ? "bg-amber-50 text-amber-700 ring-amber-200 dark:bg-amber-950 dark:text-amber-300 dark:ring-amber-900"
        : "bg-green-50 text-green-700 ring-green-200 dark:bg-green-950 dark:text-green-300 dark:ring-green-900";
  return <span className={`inline-flex rounded-md px-2 py-0.5 text-xs font-medium ring-1 ${cls}`}>{status}</span>;
}

const inputCls =
  "w-full rounded-md border border-strong px-3 py-1.5 text-sm focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500";

export function Logs() {
  const { logs, loadingLogs, refreshLogs } = useAppStore();
  const [keyword, setKeyword] = useState("");
  const [channel, setChannel] = useState("全部");
  const [model, setModel] = useState("全部");

  // 防抖关键词，避免每次键入都触发查询
  useEffect(() => {
    const handle = setTimeout(() => {
      refreshLogs({ keyword: keyword || undefined, channel, model });
    }, 300);
    return () => clearTimeout(handle);
  }, [keyword, channel, model, refreshLogs]);

  // 首次挂载
  useEffect(() => {
    refreshLogs({});
  }, [refreshLogs]);

  const channels = useMemo(() => ["全部", ...Array.from(new Set(logs.map((l) => l.channel)))], [logs]);
  const models = useMemo(() => ["全部", ...Array.from(new Set(logs.map((l) => l.model)))], [logs]);

  return (
    <div>
      <PageHeader title="日志" description="查看网关请求日志，支持筛选搜索" />

      <Card className="mb-4">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          <div className="relative">
            <Search size={16} className="absolute left-3 top-1/2 -translate-y-1/2 text-faint" />
            <input
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
              className={`${inputCls} pl-9`}
              placeholder="按关键词搜索（请求ID/密钥/渠道）"
            />
          </div>
          <select value={channel} onChange={(e) => setChannel(e.target.value)} className={inputCls}>
            {channels.map((c) => (
              <option key={c} value={c}>
                {c === "全部" ? "全部渠道" : c}
              </option>
            ))}
          </select>
          <select value={model} onChange={(e) => setModel(e.target.value)} className={inputCls}>
            {models.map((m) => (
              <option key={m} value={m}>
                {m === "全部" ? "全部模型" : m}
              </option>
            ))}
          </select>
        </div>
        <p className="mt-2 text-xs text-faint">共 {logs.length} 条记录</p>
      </Card>

      <Card className="p-0 overflow-hidden">
        <table className="w-full text-left text-sm">
          <thead className="border-b border-edge bg-inset text-xs text-muted">
            <tr>
              <th className="px-3 py-2 font-medium">时间</th>
              <th className="px-3 py-2 font-medium">渠道 / 密钥</th>
              <th className="px-3 py-2 font-medium">模型</th>
              <th className="px-3 py-2 font-medium">状态</th>
              <th className="px-3 py-2 font-medium">延迟</th>
              <th className="px-3 py-2 font-medium">Token（命中 / 输出）</th>
              <th className="px-3 py-2 font-medium">成本</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-edge">
            {logs.map((l) => (
              <tr key={l.id} className="hover:bg-hover">
                <td className="px-3 py-2">
                  <p className="font-mono text-[11px] text-faint" title={l.id}>
                    {l.id.slice(4, 16)}
                  </p>
                  <p className="text-sub">{l.timestamp.slice(5, 16)}</p>
                </td>
                <td className="px-3 py-2">
                  <p className="text-sub">{l.channel}</p>
                  {l.key && <p className="max-w-28 truncate text-xs text-faint" title={l.key}>{l.key}</p>}
                </td>
                <td className="px-3 py-2 text-muted">{l.model}</td>
                <td className="px-3 py-2">{statusPill(l.status)}</td>
                <td className="px-3 py-2 text-muted">{l.latencyMs} ms</td>
                <td className="px-3 py-2">
                  <p className="text-sub">{fmt(l.tokens)}</p>
                  {(l.cacheHitTokens > 0 || l.reasoningTokens > 0) && (
                    <p className="text-xs">
                      {l.cacheHitTokens > 0 && (
                        <span className="text-emerald-600">命中 {fmt(l.cacheHitTokens)}</span>
                      )}
                      {l.cacheHitTokens > 0 && l.reasoningTokens > 0 && <span className="text-strong"> · </span>}
                      {l.reasoningTokens > 0 && <span className="text-violet-600">输出 {fmt(l.reasoningTokens)}</span>}
                    </p>
                  )}
                </td>
                <td className="px-3 py-2 text-muted">¥{l.cost.toFixed(3)}</td>
              </tr>
            ))}
            {loadingLogs && (
              <tr>
                <td colSpan={7} className="px-4 py-8 text-center text-sm text-faint">
                  加载中…
                </td>
              </tr>
            )}
            {!loadingLogs && logs.length === 0 && (
              <tr>
                <td colSpan={7} className="px-4 py-8 text-center text-sm text-faint">
                  没有符合条件的日志记录
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </Card>
    </div>
  );
}
