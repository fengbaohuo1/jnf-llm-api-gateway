import { useEffect, useMemo, useState } from "react";
import { Search } from "lucide-react";
import { useAppStore } from "../store/useAppStore";
import { PageHeader, Card } from "../components/ui";

function statusPill(status: number) {
  const cls =
    status >= 500
      ? "bg-red-50 text-red-700 ring-red-200"
      : status >= 400
        ? "bg-amber-50 text-amber-700 ring-amber-200"
        : "bg-green-50 text-green-700 ring-green-200";
  return <span className={`inline-flex rounded-md px-2 py-0.5 text-xs font-medium ring-1 ${cls}`}>{status}</span>;
}

const inputCls =
  "w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500";

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

  const filtered = logs;

  return (
    <div>
      <PageHeader title="日志" description="查看网关请求日志，支持筛选搜索" />

      <Card className="mb-4">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          <div className="relative">
            <Search size={16} className="absolute left-3 top-1/2 -translate-y-1/2 text-gray-400" />
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
        <p className="mt-3 text-xs text-gray-400">共 {filtered.length} 条记录</p>
      </Card>

      <Card className="p-0 overflow-hidden">
        <table className="w-full text-left text-sm">
          <thead className="border-b border-gray-200 bg-gray-50 text-xs text-gray-500">
            <tr>
              <th className="px-4 py-3 font-medium">请求ID</th>
              <th className="px-4 py-3 font-medium">时间</th>
              <th className="px-4 py-3 font-medium">渠道</th>
              <th className="px-4 py-3 font-medium">模型</th>
              <th className="px-4 py-3 font-medium">密钥</th>
              <th className="px-4 py-3 font-medium">状态</th>
              <th className="px-4 py-3 font-medium">延迟</th>
              <th className="px-4 py-3 font-medium">Token</th>
              <th className="px-4 py-3 font-medium">成本</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-100">
            {filtered.map((l) => (
              <tr key={l.id} className="hover:bg-gray-50">
                <td className="px-4 py-2.5 font-mono text-xs text-gray-500">{l.id}</td>
                <td className="px-4 py-2.5 text-gray-600">{l.timestamp}</td>
                <td className="px-4 py-2.5 text-gray-700">{l.channel}</td>
                <td className="px-4 py-2.5 text-gray-600">{l.model}</td>
                <td className="px-4 py-2.5">
                  <code className="font-mono text-xs text-gray-500">{l.key}</code>
                </td>
                <td className="px-4 py-2.5">{statusPill(l.status)}</td>
                <td className="px-4 py-2.5 text-gray-600">{l.latencyMs} ms</td>
                <td className="px-4 py-2.5 text-gray-600">{l.tokens.toLocaleString()}</td>
                <td className="px-4 py-2.5 text-gray-600">${l.cost.toFixed(3)}</td>
              </tr>
            ))}
            {loadingLogs && (
              <tr>
                <td colSpan={9} className="px-4 py-10 text-center text-sm text-gray-400">
                  加载中…
                </td>
              </tr>
            )}
            {!loadingLogs && filtered.length === 0 && (
              <tr>
                <td colSpan={9} className="px-4 py-10 text-center text-sm text-gray-400">
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
