import { Activity, Coins, Radio, Timer } from "lucide-react";
import { dashboardStats, usagePoints } from "../data/mock";
import { Card, PageHeader } from "../components/ui";

function StatCard({
  icon,
  label,
  value,
  unit,
  trend,
  color,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  unit?: string;
  trend?: string;
  color: string;
}) {
  return (
    <Card>
      <div className="flex items-start justify-between">
        <div className={`flex h-11 w-11 items-center justify-center rounded-lg ${color}`}>{icon}</div>
        {trend && (
          <span className="rounded-full bg-green-50 px-2 py-0.5 text-xs font-medium text-green-600">
            ↑ {trend}
          </span>
        )}
      </div>
      <p className="mt-4 text-sm text-gray-500">{label}</p>
      <p className="mt-1 text-2xl font-bold text-gray-900">
        {value}
        {unit && <span className="ml-1 text-sm font-normal text-gray-400">{unit}</span>}
      </p>
    </Card>
  );
}

function UsageChart({ data }: { data: typeof usagePoints }) {
  const max = Math.max(...data.map((p) => p.requests));
  return (
    <Card>
      <h3 className="mb-4 text-sm font-semibold text-gray-700">今日请求趋势</h3>
      <div className="flex h-48 items-end gap-2">
        {data.map((p) => (
          <div key={p.label} className="flex flex-1 flex-col items-center gap-1">
            <div
              className="w-full rounded-t-md bg-indigo-500/80 transition-all hover:bg-indigo-600"
              style={{ height: `${(p.requests / max) * 100}%`, minHeight: "8px" }}
              title={`请求 ${p.requests.toLocaleString()}`}
            />
            <span className="text-[10px] text-gray-400">{p.label}</span>
          </div>
        ))}
      </div>
    </Card>
  );
}

export function Dashboard() {
  const s = dashboardStats;
  return (
    <div>
      <PageHeader title="仪表盘" description="今日网关运行概览" />

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <StatCard
          icon={<Activity size={20} className="text-indigo-600" />}
          color="bg-indigo-50"
          label="今日请求数"
          value={s.todayRequests.toLocaleString()}
          trend={`${s.trend.requests}%`}
        />
        <StatCard
          icon={<Coins size={20} className="text-violet-600" />}
          color="bg-violet-50"
          label="Token 消耗"
          value={s.todayTokens.toLocaleString()}
          trend={`${s.trend.tokens}%`}
        />
        <StatCard
          icon={<Radio size={20} className="text-emerald-600" />}
          color="bg-emerald-50"
          label="活跃渠道数"
          value={String(s.activeChannels)}
        />
        <StatCard
          icon={<Timer size={20} className="text-amber-600" />}
          color="bg-amber-50"
          label="平均延迟"
          value={String(s.avgLatencyMs)}
          unit="ms"
        />
      </div>

      <div className="mt-6">
        <UsageChart data={usagePoints} />
      </div>
    </div>
  );
}
