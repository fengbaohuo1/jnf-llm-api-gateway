import { useEffect, useState } from "react";
import { Activity, Coins, Radio, Timer } from "lucide-react";
import { api } from "../api";
import { Card, PageHeader } from "../components/ui";

interface Stat {
  requests: number;
  tokens: number;
  avgLatencyMs: number;
  activeChannels: number;
}

export function Dashboard() {
  const [stat, setStat] = useState<Stat | null>(null);
  const [daily, setDaily] = useState<{ date: string; requests: number; tokens: number; avgLatencyMs: number }[]>([]);
  const [activeChannels, setActiveChannels] = useState(0);

  useEffect(() => {
    api
      .todayStats()
      .then((s) => setStat((prev) => ({ requests: s.requests, tokens: s.tokens, avgLatencyMs: s.avgLatencyMs, activeChannels: prev?.activeChannels ?? 0 })))
      .catch(() => {});
    api.listDailyStats(7).then((rows) => setDaily(rows.reverse())).catch(() => {});
    api.listChannels().then((chs) => {
      const act = chs.filter((c) => c.status === "active").length;
      setActiveChannels(act);
      setStat((prev) => (prev ? { ...prev, activeChannels: act } : prev));
    }).catch(() => {});
  }, []);

  const requests = stat?.requests ?? 0;
  const tokens = stat?.tokens ?? 0;
  const avg = stat?.avgLatencyMs ?? 0;

  const maxReq = Math.max(1, ...daily.map((d) => d.requests));
  const maxTok = Math.max(1, ...daily.map((d) => d.tokens));

  return (
    <div>
      <PageHeader title="仪表盘" description="今日网关运行概览" />

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <StatCard icon={<Activity size={20} className="text-indigo-600" />} color="bg-indigo-50" label="今日请求数" value={requests.toLocaleString()} />
        <StatCard icon={<Coins size={20} className="text-violet-600" />} color="bg-violet-50" label="Token 消耗" value={tokens.toLocaleString()} />
        <StatCard icon={<Radio size={20} className="text-emerald-600" />} color="bg-emerald-50" label="活跃渠道数" value={String(activeChannels)} />
        <StatCard icon={<Timer size={20} className="text-amber-600" />} color="bg-amber-50" label="平均延迟" value={String(Math.round(avg))} unit="ms" />
      </div>

      <div className="mt-6 grid grid-cols-1 gap-4 lg:grid-cols-2">
        <Card>
          <h3 className="mb-4 text-sm font-semibold text-gray-700">近 7 天请求趋势</h3>
          <Chart values={daily.map((d) => d.requests)} max={maxReq} labels={daily.map((d) => d.date.slice(5))} color="bg-indigo-500/80" />
        </Card>
        <Card>
          <h3 className="mb-4 text-sm font-semibold text-gray-700">近 7 天 Token 消耗</h3>
          <Chart values={daily.map((d) => d.tokens)} max={maxTok} labels={daily.map((d) => d.date.slice(5))} color="bg-violet-500/80" />
        </Card>
      </div>
    </div>
  );
}

function StatCard({ icon, label, value, unit, color }: { icon: React.ReactNode; label: string; value: string; unit?: string; color: string }) {
  return (
    <Card>
      <div className={`flex h-11 w-11 items-center justify-center rounded-lg ${color}`}>{icon}</div>
      <p className="mt-4 text-sm text-gray-500">{label}</p>
      <p className="mt-1 text-2xl font-bold text-gray-900">
        {value}
        {unit && <span className="ml-1 text-sm font-normal text-gray-400">{unit}</span>}
      </p>
    </Card>
  );
}

function Chart({ values, max, labels, color }: { values: number[]; max: number; labels: string[]; color: string }) {
  return (
    <div className="flex h-40 items-end gap-2">
      {values.map((v, i) => (
        // 列 h-full（外层 h-40 定高）+ 定高容器 + 绝对定位，避免百分比解析失效导致柱子贴底
        <div key={i} className="flex h-full min-w-0 flex-1 flex-col items-center">
          <div className="relative h-32 w-full">
            <div
              className={`absolute inset-x-0 bottom-0 rounded-t-md ${color}`}
              style={{ height: `${(v / max) * 100}%`, minHeight: "4px" }}
              title={v.toLocaleString()}
            />
          </div>
          <span className="mt-1 text-[10px] text-gray-400">{labels[i]}</span>
        </div>
      ))}
      {values.length === 0 && <p className="w-full py-16 text-center text-sm text-gray-400">暂无数据</p>}
    </div>
  );
}
