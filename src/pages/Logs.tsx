import { useCallback, useEffect, useMemo, useState } from "react";
import { Copy, RefreshCw, Search } from "lucide-react";
import { useAppStore } from "../store/useAppStore";
import { PageHeader, Card } from "../components/ui";
import { Button, Modal } from "../components/Modal";
import { api } from "../api";
import type { RequestDetail, RequestLog } from "../types";

const fmt = (n: number) => n.toLocaleString("en-US");

/** 缓存命中率（百分比整数）；无输入 token（失败/未解析 usage）时为 null */
function hitRate(l: { promptTokens: number; cacheHitTokens: number }): number | null {
  if (l.promptTokens <= 0) return null;
  return Math.round((l.cacheHitTokens / l.promptTokens) * 100);
}

const PROTO_LABEL: Record<string, { label: string; cls: string }> = {
  openai: { label: "OpenAI", cls: "bg-sky-50 text-sky-700 ring-sky-200 dark:bg-sky-950 dark:text-sky-300 dark:ring-sky-900" },
  anthropic: { label: "Anthropic", cls: "bg-orange-50 text-orange-700 ring-orange-200 dark:bg-orange-950 dark:text-orange-300 dark:ring-orange-900" },
  responses: { label: "Responses", cls: "bg-violet-50 text-violet-700 ring-violet-200 dark:bg-violet-950 dark:text-violet-300 dark:ring-violet-900" },
};

/** 安全风险徽标：none（无命中）/ low / medium / high */
const RISK_LABEL: Record<RequestLog["riskLevel"], { label: string; cls: string }> = {
  none: { label: "无风险", cls: "bg-inset text-muted ring-edge" },
  low: { label: "低风险", cls: "bg-sky-50 text-sky-700 ring-sky-200 dark:bg-sky-950 dark:text-sky-300 dark:ring-sky-900" },
  medium: {
    label: "中风险",
    cls: "bg-amber-50 text-amber-700 ring-amber-200 dark:bg-amber-950 dark:text-amber-300 dark:ring-amber-900",
  },
  high: { label: "高风险", cls: "bg-red-50 text-red-700 ring-red-200 dark:bg-red-950 dark:text-red-300 dark:ring-red-900" },
};

function riskBadge(risk: RequestLog["riskLevel"]) {
  const r = RISK_LABEL[risk] ?? RISK_LABEL.none;
  return (
    <span className={`inline-flex rounded-md px-1.5 py-0.5 text-[10px] font-medium ring-1 ${r.cls}`}>{r.label}</span>
  );
}

function statusPill(status: number) {
  // 后端 requests.status 语义：1 = 成功，0 = 失败（不是 HTTP 状态码）
  const ok = status === 1;
  const cls = ok
    ? "bg-green-50 text-green-700 ring-green-200 dark:bg-green-950 dark:text-green-300 dark:ring-green-900"
    : "bg-red-50 text-red-700 ring-red-200 dark:bg-red-950 dark:text-red-300 dark:ring-red-900";
  return (
    <span className={`inline-flex rounded-md px-2 py-0.5 text-xs font-medium ring-1 ${cls}`}>
      {ok ? "成功" : "失败"}
    </span>
  );
}

function protoBadge(protocol: string) {
  const p = PROTO_LABEL[protocol] ?? {
    label: protocol || "-",
    cls: "bg-slate-100 text-slate-600 ring-slate-200 dark:bg-slate-900 dark:text-slate-400 dark:ring-slate-800",
  };
  return (
    <span className={`inline-flex rounded-md px-1.5 py-0.5 text-[10px] font-medium ring-1 ${p.cls}`}>{p.label}</span>
  );
}

const inputCls =
  "w-full rounded-lg border border-transparent bg-inset px-3 py-1.5 text-sm text-main transition-colors placeholder:text-faint focus:border-transparent focus:bg-card focus:outline-none focus:ring-2 focus:ring-indigo-500/30";

/** JSON 格式化展示：可解析则 pretty，否则原样（截断的长文本/错误体） */
function tryPretty(raw: string | null): string {
  if (!raw) return "";
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

/** 单个内容段：折叠标题 + 复制按钮 + 等宽正文 */
function BodySection({ title, raw }: { title: string; raw: string | null }) {
  const [copied, setCopied] = useState(false);
  if (!raw) return null;
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(raw);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* ignore */
    }
  };
  return (
    <details className="group rounded-xl bg-inset/40" open>
      <summary className="flex cursor-pointer select-none items-center justify-between rounded-lg bg-inset px-3 py-2 text-sm font-medium text-sub hover:bg-hover">
        <span>{title}</span>
        <span className="flex items-center gap-2">
          <button
            onClick={(e) => {
              e.preventDefault();
              copy();
            }}
            className="inline-flex items-center gap-1 rounded-md bg-inset px-2 py-0.5 text-xs text-faint hover:bg-hover hover:text-sub"
          >
            <Copy size={12} /> {copied ? "已复制" : "复制"}
          </button>
          <span className="text-xs text-faint">{raw.length.toLocaleString("en-US")} 字符</span>
        </span>
      </summary>
      <pre className="overflow-x-auto whitespace-pre-wrap break-all px-3 py-2 font-mono text-xs leading-relaxed text-sub">
        {tryPretty(raw)}
      </pre>
    </details>
  );
}

/** 请求详情弹窗：概要 + 请求/转发/响应/错误四段内容 */
function DetailModal({ log, onClose }: { log: RequestLog; onClose: () => void }) {
  const [detail, setDetail] = useState<RequestDetail | null>(null);
  const [failed, setFailed] = useState(false);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let alive = true;
    setDetail(null);
    setFailed(false);
    setLoading(true);
    api
      .getRequestDetail(log.id)
      .then((d) => alive && setDetail(d))
      .catch(() => alive && setFailed(true))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [log.id]);

  const hasDetail = detail && (detail.requestBody || detail.upstreamBody || detail.responseBody || detail.errorBody);
  return (
    <Modal open title={`请求详情 · ${log.timestamp.slice(5, 19)}`} onClose={onClose} size="lg">
      <div className="space-y-4">
        {/* 概要信息 */}
        <div className="grid grid-cols-2 gap-x-4 gap-y-2 rounded-xl bg-inset/70 px-3 py-3 text-sm sm:grid-cols-3">
          <div>
            <p className="text-xs text-faint">模型 / 协议</p>
            <p className="flex items-center gap-2 font-medium text-sub">
              {log.model}
              {protoBadge(log.protocol)}
              {log.stream && (
                <span className="text-xs font-normal text-faint">流式</span>
              )}
            </p>
          </div>
          <div>
            <p className="text-xs text-faint">渠道 / 密钥</p>
            <p className="text-sub">
              {log.channel || "-"}
              {log.key && <span className="ml-1 text-xs text-faint">· {log.key}</span>}
            </p>
          </div>
          <div>
            <p className="text-xs text-faint">状态 / 尝试次数</p>
            <p className="flex items-center gap-2">
              {statusPill(log.status)}
              {log.attempts > 1 && <span className="text-xs text-faint">第 {log.attempts} 次尝试成功</span>}
            </p>
          </div>
          <div>
            <p className="text-xs text-faint">安全风险（请求 + 响应）</p>
            <p className="mt-0.5">{riskBadge(log.riskLevel)}</p>
          </div>
          <div>
            <p className="text-xs text-faint">Token（命中 / 输出 / 总）</p>
            <p className="font-medium text-sub">
              {fmt(log.tokens)}
              {(() => {
                const rate = hitRate(log);
                return rate !== null ? (
                  <span className="ml-2 text-xs font-normal text-emerald-600">
                    命中率 {rate}%
                    {log.cacheHitTokens > 0 && `（${fmt(log.cacheHitTokens)}）`}
                  </span>
                ) : null;
              })()}
              {log.reasoningTokens > 0 && (
                <span className="ml-2 text-xs font-normal text-violet-600 dark:text-violet-300">
                  输出 {fmt(log.reasoningTokens)}
                </span>
              )}
            </p>
          </div>
          <div>
            <p className="text-xs text-faint">延迟 / 成本</p>
            <p className="font-medium text-sub">
              {log.latencyMs} ms <span className="ml-2 text-xs font-normal text-muted">¥{log.cost.toFixed(4)}</span>
            </p>
          </div>
          <div className="col-span-2 sm:col-span-1">
            <p className="text-xs text-faint">客户端 UA</p>
            <p className="truncate text-xs text-muted" title={log.clientUa}>
              {log.clientUa || "-"}
            </p>
          </div>
        </div>

        {/* 内容明细 */}
        {loading && <p className="text-sm text-faint">内容明细加载中…</p>}
        {failed && <p className="text-sm text-faint">明细加载失败</p>}
        {!loading && !failed && !hasDetail && (
          <p className="rounded-xl bg-inset/40 px-3 py-3 text-sm text-faint">
            该请求没有内容明细 —— 可能低于「日志内容存储级别」阈值（无风险请求默认不存内容），
            或设置中关闭了内容记录，也可能是引入内容记录前的旧数据。概要、计费与安全事件不受影响。
          </p>
        )}
        {hasDetail && (
          <div className="space-y-2">
            <BodySection title="① 收到的原始请求（客户端协议原文，脱敏后）" raw={detail?.requestBody ?? null} />
            <BodySection title="② 实际转发给上游的请求（内部 OpenAI 格式）" raw={detail?.upstreamBody ?? null} />
            <BodySection title="③ 上游返回的响应（成功时完整 JSON）" raw={detail?.responseBody ?? null} />
            <BodySection title="④ 错误响应体（失败时上游/网关返回）" raw={detail?.errorBody ?? null} />
          </div>
        )}
        <p className="text-right text-xs text-faint">请求 ID：{log.id}</p>
      </div>
    </Modal>
  );
}

export function Logs() {
  const { logs, loadingLogs, refreshLogs } = useAppStore();
  const [keyword, setKeyword] = useState("");
  const [channel, setChannel] = useState("全部");
  const [model, setModel] = useState("全部");
  const [risk, setRisk] = useState("全部");
  const [selected, setSelected] = useState<RequestLog | null>(null);
  // 筛选下拉的可选值：跨多次查询累积（否则选中某渠道后其它渠道选项会消失）
  const [channelOptions, setChannelOptions] = useState<string[]>([]);
  const [modelOptions, setModelOptions] = useState<string[]>([]);
  const [lastUpdated, setLastUpdated] = useState<Date | null>(null);

  const filters = useMemo(
    () => ({ keyword: keyword || undefined, channel, model, risk }),
    [keyword, channel, model, risk],
  );

  const load = useCallback(
    async (f: typeof filters) => {
      await refreshLogs(f);
      setLastUpdated(new Date());
    },
    [refreshLogs],
  );

  // 防抖关键词；筛选变化或手动刷新时复用同一入口
  useEffect(() => {
    const handle = setTimeout(() => {
      void load(filters);
    }, 300);
    return () => clearTimeout(handle);
  }, [filters, load]);

  // 累积渠道 / 模型选项（只增不减，保证当前选中项始终在列表里）
  useEffect(() => {
    if (logs.length === 0) return;
    setChannelOptions((prev) =>
      Array.from(new Set([...prev, ...logs.map((l) => l.channel).filter(Boolean)])).sort(),
    );
    setModelOptions((prev) => Array.from(new Set([...prev, ...logs.map((l) => l.model).filter(Boolean)])).sort());
  }, [logs]);

  const channels = useMemo(
    () => ["全部", ...new Set([...(channel !== "全部" ? [channel] : []), ...channelOptions])],
    [channel, channelOptions],
  );
  const models = useMemo(() => ["全部", ...new Set([...(model !== "全部" ? [model] : []), ...modelOptions])], [model, modelOptions]);

  return (
    <div>
      <PageHeader
        title="日志"
        description="查看网关请求日志，点击任一行查看完整请求与响应内容"
        actions={
          <div className="flex items-center gap-2">
            {lastUpdated && (
              <span className="text-xs text-faint">更新于 {lastUpdated.toLocaleTimeString("zh-CN", { hour12: false })}</span>
            )}
            <Button variant="secondary" onClick={() => void load(filters)} disabled={loadingLogs}>
              <RefreshCw size={14} className={`mr-1 inline ${loadingLogs ? "animate-spin" : ""}`} />
              刷新
            </Button>
          </div>
        }
      />

      <Card className="mb-4">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-4">
          <div className="relative">
            <Search size={16} className="absolute left-3 top-1/2 -translate-y-1/2 text-faint" />
            <input
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
              className={`${inputCls} pl-9`}
              placeholder="按关键词搜索（ID/密钥/渠道/模型/请求与响应内容）"
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
          <select value={risk} onChange={(e) => setRisk(e.target.value)} className={inputCls}>
            <option value="全部">全部风险</option>
            <option value="none">无风险（未命中）</option>
            <option value="low">低风险</option>
            <option value="medium">中风险</option>
            <option value="high">高风险</option>
          </select>
        </div>
        <p className="mt-2 text-xs text-faint">
          共 {logs.length} 条记录（最多显示最近 500 条） · 点击行查看完整请求/响应内容（内容是否保存由设置中的「日志内容存储级别」决定）
        </p>
      </Card>

      <Card className="p-0 overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead className="bg-inset/80 text-xs text-muted">
            <tr>
              <th className="px-3 py-2 font-medium">时间</th>
              <th className="px-3 py-2 font-medium">渠道 / 密钥</th>
              <th className="px-3 py-2 font-medium">模型 / 协议</th>
              <th className="px-3 py-2 font-medium">状态</th>
              <th className="px-3 py-2 font-medium">延迟</th>
              <th className="px-3 py-2 font-medium">Token（命中 / 输出）</th>
              <th className="px-3 py-2 font-medium">成本</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-edge/40">
            {logs.map((l) => (
              <tr
                key={l.id}
                onClick={() => setSelected(l)}
                tabIndex={0}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    setSelected(l);
                  }
                }}
                className="cursor-pointer hover:bg-hover focus:outline-none focus-visible:ring-1 focus-visible:ring-indigo-500"
                title="点击查看完整内容"
              >
                <td className="px-3 py-2">
                  <p className="font-mono text-[11px] text-faint" title={l.id}>
                    {l.id.slice(4, 16)}
                  </p>
                  <p className="text-sub">{l.timestamp.slice(5, 16)}</p>
                </td>
                <td className="px-3 py-2">
                  <p className="text-sub">{l.channel || "-"}</p>
                  {l.key && <p className="max-w-28 truncate text-xs text-faint" title={l.key}>{l.key}</p>}
                </td>
                <td className="px-3 py-2">
                  <p className="flex items-center gap-2">
                    <span className="max-w-36 truncate text-muted" title={l.model}>{l.model}</span>
                    {protoBadge(l.protocol)}
                  </p>
                  <p className="text-xs text-faint">
                    {l.stream ? "流式" : "非流式"}
                    {l.status === 1 && l.attempts > 1 && ` · 第${l.attempts}次尝试成功`}
                  </p>
                </td>
                <td className="px-3 py-2">
                  <div className="flex items-center gap-1.5">
                    {statusPill(l.status)}
                    {l.riskLevel !== "none" && riskBadge(l.riskLevel)}
                  </div>
                </td>
                <td className="px-3 py-2 text-muted">{l.latencyMs} ms</td>
                <td className="px-3 py-2">
                  <p className="text-sub">{fmt(l.tokens)}</p>
                  {(() => {
                    const rate = hitRate(l);
                    return (
                      (rate !== null || l.reasoningTokens > 0) && (
                        <p className="text-xs">
                          {rate !== null && (
                            <span className={rate > 0 ? "text-emerald-600" : "text-faint"}>
                              命中率 {rate}%
                              {l.cacheHitTokens > 0 && `（命中 ${fmt(l.cacheHitTokens)}）`}
                            </span>
                          )}
                          {rate !== null && l.reasoningTokens > 0 && <span className="text-strong"> · </span>}
                          {l.reasoningTokens > 0 && (
                            <span className="text-violet-600 dark:text-violet-300">输出 {fmt(l.reasoningTokens)}</span>
                          )}
                        </p>
                      )
                    );
                  })()}
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

      {selected && <DetailModal log={selected} onClose={() => setSelected(null)} />}
    </div>
  );
}
