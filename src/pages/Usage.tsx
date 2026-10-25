import { useEffect, useMemo, useState } from "react";
import { BarChart3 } from "lucide-react";
import { api } from "../api";
import { Card, PageHeader } from "../components/ui";
import { lastNDates } from "../lib/dates";

const ranges = [
  { key: "今日", days: 1 },
  { key: "本周", days: 7 },
  { key: "本月", days: 30 },
] as const;

type RangeKey = (typeof ranges)[number]["key"];

interface DayStat {
  date: string;
  requests: number;
  tokens: number;
  cacheHitTokens: number;
  completionTokens: number;
  cost: number;
}

/** 精确格式化（不用 k/M 缩写） */
const fmt = (n: number) => n.toLocaleString("en-US");

export function Usage() {
  const [range, setRange] = useState<RangeKey>("今日");
  const [daily, setDaily] = useState<DayStat[]>([]);

  const days = ranges.find((r) => r.key === range)!.days;

  useEffect(() => {
    api
      .listDailyStats(days)
      .then((rows) => setDaily(rows.reverse()))
      .catch(() => {});
  }, [days]);

  // 三部分拆分：命中缓存 = 明细列；输出 = completion；未命中 = 剩余输入
  const totals = useMemo(
    () =>
      daily.reduce(
        (acc, d) => ({
          requests: acc.requests + d.requests,
          tokens: acc.tokens + d.tokens,
          cacheHit: acc.cacheHit + d.cacheHitTokens,
          output: acc.output + d.completionTokens,
          cost: acc.cost + d.cost,
        }),
        { requests: 0, tokens: 0, cacheHit: 0, output: 0, cost: 0 },
      ),
    [daily],
  );
  const cacheMiss = Math.max(0, totals.tokens - totals.cacheHit - totals.output);
  // 缓存命中率 = 命中缓存 / 输入总量（命中 + 未命中）
  const inputTokens = totals.cacheHit + cacheMiss;
  const hitRate = inputTokens > 0 ? (totals.cacheHit / inputTokens) * 100 : null;

  // 补齐区间内没有数据的日期（后端只返回有数据的日期），否则只剩一根满宽柱子
  const series = useMemo(() => {
    const byDate = new Map(daily.map((d) => [d.date, d]));
    return lastNDates(days).map(
      (date) =>
        byDate.get(date) ?? {
          date,
          requests: 0,
          tokens: 0,
          cacheHitTokens: 0,
          completionTokens: 0,
          cost: 0,
        },
    );
  }, [daily, days]);

  const max = Math.max(1, ...series.map((d) => d.tokens));

  return (
    <div>
      <PageHeader
        title="用量"
        description="查看请求量与 Token 消耗（精确到个位）"
        actions={
          <div className="flex rounded-lg bg-inset p-1">
            {ranges.map((r) => (
              <button
                key={r.key}
                onClick={() => setRange(r.key)}
                className={`rounded-md px-3 py-1 text-sm font-medium ${
                  range === r.key ? "bg-card text-accent shadow-sm" : "text-muted hover:text-sub"
                }`}
              >
                {r.key}
              </button>
            ))}
          </div>
        }
      />

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <Card>
          <p className="text-xs text-muted">请求总数</p>
          <p className="mt-1 text-2xl font-bold text-main">{fmt(totals.requests)}</p>
        </Card>
        <Card>
          <p className="text-xs text-muted">Token 总消耗</p>
          <p className="mt-1 text-2xl font-bold text-main">{fmt(totals.tokens)}</p>
          <p className="mt-1 text-xs text-faint">
            缓存命中率{" "}
            <span className="font-medium text-emerald-600">
              {hitRate === null ? "—" : `${hitRate.toFixed(1)}%`}
            </span>
          </p>
        </Card>
        <Card>
          <p className="text-xs text-muted">预估成本</p>
          <p className="mt-1 text-2xl font-bold text-main">¥{totals.cost.toFixed(3)}</p>
        </Card>
      </div>

      <Card className="mt-4">
        <h3 className="mb-1 flex items-center gap-2 text-sm font-semibold text-sub">
          <BarChart3 size={15} /> Token 消耗构成（近 {days} 天）
        </h3>
        <div className="mb-3 flex items-center gap-4 text-xs text-muted">
          <span className="flex items-center gap-1.5">
            <span className="h-2.5 w-2.5 rounded-sm bg-emerald-500" /> 命中缓存
          </span>
          <span className="flex items-center gap-1.5">
            <span className="h-2.5 w-2.5 rounded-sm bg-sky-500" /> 未命中缓存
          </span>
          <span className="flex items-center gap-1.5">
            <span className="h-2.5 w-2.5 rounded-sm bg-violet-500" /> 输出
          </span>
        </div>
        <div className="flex h-52 items-end gap-2">
          {daily.length === 0 ? (
            <p className="w-full py-20 text-center text-sm text-faint">暂无数据</p>
          ) : (
            series.map((p, i) => {
              const denom = Math.max(p.tokens, 1);
              const miss = Math.max(0, p.tokens - p.cacheHitTokens - p.completionTokens);
              return (
                <div key={i} className="group relative flex h-full min-w-0 flex-1 flex-col items-center">
                  {/* 悬停浮窗：标明当天各类 token 数量 */}
                  {p.tokens > 0 && (
                    <div className="pointer-events-none absolute -top-1 left-1/2 z-10 hidden -translate-x-1/2 -translate-y-full flex-col gap-0.5 whitespace-nowrap rounded-xl bg-card px-3 py-2 text-xs shadow-pop group-hover:flex">
                      <p className="mb-0.5 font-semibold text-main">
                        {p.date} · 总计 {fmt(p.tokens)}
                      </p>
                      <p className="text-emerald-600">命中缓存 {fmt(p.cacheHitTokens)}</p>
                      <p className="text-sky-600">未命中 {fmt(miss)}</p>
                      <p className="text-violet-600 dark:text-violet-300">输出 {fmt(p.completionTokens)}</p>
                    </div>
                  )}
                  <span className="mb-1 truncate text-[10px] font-medium text-muted" title={fmt(p.tokens)}>
                    {p.tokens > 0 ? fmt(p.tokens) : ""}
                  </span>
                  {/* 柱子区域：h-40 定高 → 绝对定位百分比高度，避免 flex 百分比解析异常导致贴底 */}
                  <div className="relative h-40 w-full">
                    <div
                      className="absolute inset-x-0 bottom-0 flex flex-col justify-end overflow-hidden rounded-t-md"
                      style={{ height: `${(p.tokens / max) * 100}%`, minHeight: p.tokens > 0 ? "8px" : 0 }}
                    >
                      {/* 从上到下：命中缓存 → 未命中 → 输出（flex-col 首个子元素在顶部） */}
                      <div className="bg-emerald-500/80" style={{ height: `${(p.cacheHitTokens / denom) * 100}%` }} />
                      <div className="bg-sky-500/80" style={{ height: `${(miss / denom) * 100}%` }} />
                      <div className="bg-violet-500/80" style={{ height: `${(p.completionTokens / denom) * 100}%` }} />
                    </div>
                  </div>
                  <span className="mt-1 text-[10px] text-faint">{p.date.slice(5)}</span>
                </div>
              );
            })
          )}
        </div>
      </Card>
    </div>
  );
}
