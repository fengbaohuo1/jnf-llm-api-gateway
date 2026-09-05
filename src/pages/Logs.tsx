import { useEffect, useMemo, useState } from "react";
import { Copy, Search } from "lucide-react";
import { useAppStore } from "../store/useAppStore";
import { PageHeader, Card } from "../components/ui";
import { Modal } from "../components/Modal";
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

function statusPill(status: number) {
  const cls =
    status >= 500
      ? "bg-red-50 text-red-700 ring-red-200 dark:bg-red-950 dark:text-red-300 dark:ring-red-900"
      : status >= 400
        ? "bg-amber-50 text-amber-700 ring-amber-200 dark:bg-amber-950 dark:text-amber-300 dark:ring-amber-900"
        : "bg-green-50 text-green-700 ring-green-200 dark:bg-green-950 dark:text-green-300 dark:ring-green-900";
  return <span className={`inline-flex rounded-md px-2 py-0.5 text-xs font-medium ring-1 ${cls}`}>{status}</span>;
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
  "w-full rounded-md border border-strong px-3 py-1.5 text-sm focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500";

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
    <details className="group rounded-lg border border-edge" open>
      <summary className="flex cursor-pointer select-none items-center justify-between rounded-lg bg-inset px-3 py-2 text-sm font-medium text-sub hover:bg-hover">
        <span>{title}</span>
        <span className="flex items-center gap-2">
          <button
            onClick={(e) => {
              e.preventDefault();
              copy();
            }}
            className="inline-flex items-center gap-1 rounded-md border border-strong px-2 py-0.5 text-xs text-faint hover:bg-hover hover:text-sub"
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

  useEffect(() => {
    let alive = true;
    setDetail(null);
    setFailed(false);
    api
      .getRequestDetail(log.id)
      .then((d) => alive && setDetail(d))
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
  }, [log.id]);

  const hasDetail = detail && (detail.requestBody || detail.upstreamBody || detail.responseBody || detail.errorBody);
  return (
    <Modal open title={`请求详情 · ${log.timestamp.slice(5, 19)}`} onClose={onClose} size="lg">
      <div className="space-y-4">
        {/* 概要信息 */}
        <div className="grid grid-cols-2 gap-x-4 gap-y-2 rounded-lg border border-edge bg-inset px-3 py-3 text-sm sm:grid-cols-3">
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
                <span className="ml-2 text-xs font-normal text-violet-600">输出 {fmt(log.reasoningTokens)}</span>
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
        {failed && <p className="text-sm text-faint">明细加载失败</p>}
        {detail && !hasDetail && (
          <p className="rounded-lg border border-edge px-3 py-3 text-sm text-faint">
            该请求没有内容明细 —— 可能是旧数据，或在设置中关闭了“记录请求与响应内容”。
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
  const [selected, setSelected] = useState<RequestLog | null>(null);

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
      <PageHeader title="日志" description="查看网关请求日志，点击任一行查看完整请求与响应内容" />

      <Card className="mb-4">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
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
        </div>
        <p className="mt-2 text-xs text-faint">
          共 {logs.length} 条记录 · 点击行查看完整请求/响应内容（内容记录可在设置中关闭）
        </p>
      </Card>

      <Card className="p-0 overflow-hidden">
        <table className="w-full text-left text-sm">
          <thead className="border-b border-edge bg-inset text-xs text-muted">
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
          <tbody className="divide-y divide-edge">
            {logs.map((l) => (
              <tr
                key={l.id}
                onClick={() => setSelected(l)}
                className="cursor-pointer hover:bg-hover"
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
                    {l.attempts > 1 && ` · 第${l.attempts}次尝试成功`}
                  </p>
                </td>
                <td className="px-3 py-2">{statusPill(l.status)}</td>
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
                            <span className="text-violet-600">输出 {fmt(l.reasoningTokens)}</span>
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
