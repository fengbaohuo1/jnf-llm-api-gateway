import { useMemo, useState } from "react";
import { BarChart3 } from "lucide-react";
import { requestLogs, usagePoints } from "../data/mock";
import { Card, PageHeader } from "../components/ui";

const ranges = ["今日", "本周", "本月"] as const;

export function Usage() {
  const [range, setRange] = useState<(typeof ranges)[number]>("今日");

  const totals = useMemo(() => {
    return requestLogs.reduce(
      (acc, l) => {
        acc.requests += 1;
        acc.tokens += l.tokens;
        acc.cost += l.cost;
        return acc;
      },
      { requests: 0, tokens: 0, cost: 0 },
    );
  }, [range]);

  const max = Math.max(...usagePoints.map((p) => p.tokens));

  return (
    <div>
      <PageHeader
        title="用量"
        description="查看请求量与 Token 消耗"
        actions={
          <div className="flex rounded-lg border border-gray-200 p-0.5">
            {ranges.map((r) => (
              <button
                key={r}
                onClick={() => setRange(r)}
                className={`rounded-md px-3 py-1 text-sm font-medium ${
                  range === r ? "bg-indigo-600 text-white" : "text-gray-600 hover:bg-gray-100"
                }`}
              >
                {r}
              </button>
            ))}
          </div>
        }
      />

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <Card>
          <p className="text-sm text-gray-500">请求总数</p>
          <p className="mt-1 text-2xl font-bold text-gray-900">{totals.requests.toLocaleString()}</p>
        </Card>
        <Card>
          <p className="text-sm text-gray-500">Token 总消耗</p>
          <p className="mt-1 text-2xl font-bold text-gray-900">{totals.tokens.toLocaleString()}</p>
        </Card>
        <Card>
          <p className="text-sm text-gray-500">预估成本</p>
          <p className="mt-1 text-2xl font-bold text-gray-900">${totals.cost.toFixed(3)}</p>
        </Card>
      </div>

      <Card className="mt-6">
        <h3 className="mb-4 flex items-center gap-2 text-sm font-semibold text-gray-700">
          <BarChart3 size={16} /> Token 消耗（按时段）
        </h3>
        <div className="flex h-56 items-end gap-3">
          {usagePoints.map((p) => (
            <div key={p.label} className="flex flex-1 flex-col items-center gap-1">
              <span className="text-[10px] text-gray-400">{(p.tokens / 1000).toFixed(0)}k</span>
              <div
                className="w-full rounded-t-md bg-violet-500/80 hover:bg-violet-600"
                style={{ height: `${(p.tokens / max) * 100}%`, minHeight: "10px" }}
              />
              <span className="text-[10px] text-gray-400">{p.label}</span>
            </div>
          ))}
        </div>
      </Card>
    </div>
  );
}
