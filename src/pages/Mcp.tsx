import { useCallback, useEffect, useMemo, useState } from "react";
import {
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Copy,
  Loader,
  Play,
  RefreshCw,
  ShieldAlert,
} from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { PageHeader, Card, Toggle } from "../components/ui";
import { Button, Modal } from "../components/Modal";
import { api, mcpApi } from "../api";
import type { McpCall, McpCallResult, McpStatus, McpToolInfo, McpToolKind } from "../types";

const errOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

const inputCls =
  "rounded-md border border-strong bg-card px-3 py-1.5 text-sm text-main focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500";

const KIND_STYLE: Record<McpToolKind, string> = {
  read: "bg-inset text-muted ring-1 ring-edge",
  write: "bg-amber-50 text-amber-700 dark:bg-amber-950 dark:text-amber-300",
  dangerous: "bg-red-50 text-red-700 dark:bg-red-950 dark:text-red-300",
};

function Badge({ children, className = "" }: { children: React.ReactNode; className?: string }) {
  return (
    <span className={`inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium ${className}`}>
      {children}
    </span>
  );
}

/** 由 inputSchema 生成参数模板（调试台与 curl 示例共用） */
function argsTemplate(tool: McpToolInfo): string {
  const obj: Record<string, unknown> = {};
  for (const p of tool.params) {
    if (!p.required) continue;
    if (p.enumValues.length > 0) obj[p.name] = p.enumValues[0];
    else if (p.type === "integer" || p.type === "number") obj[p.name] = 0;
    else if (p.type === "boolean") obj[p.name] = false;
    else obj[p.name] = "";
  }
  return JSON.stringify(obj, null, 2);
}

function curlExample(status: McpStatus, tool: McpToolInfo): string {
  const args = argsTemplate(tool).replace(/\n\s*/g, "");
  const auth = status.tokenSet ? " \\\n  -H 'authorization: Bearer <你的令牌>'" : "";
  return `curl -s ${status.endpoint} \\
  -H 'content-type: application/json'${auth} \\
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"${tool.name}","arguments":${args}}}'`;
}

function clientConfig(status: McpStatus): string {
  const headers = status.tokenSet ? { Authorization: "Bearer <你的令牌>" } : undefined;
  return JSON.stringify(
    {
      mcpServers: {
        "llm-api-gateway": {
          type: "http",
          url: status.endpoint,
          ...(headers ? { headers } : {}),
        },
      },
    },
    null,
    2,
  );
}

export function Mcp() {
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [tools, setTools] = useState<McpToolInfo[]>([]);
  const [calls, setCalls] = useState<McpCall[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  // 配置草稿（保存后写 settings；host/port 需重启服务生效）
  const [host, setHost] = useState("127.0.0.1");
  const [port, setPort] = useState(8790);
  const [readOnly, setReadOnly] = useState(false);
  const [restartHint, setRestartHint] = useState(false);
  // 新生成的令牌明文（仅展示一次）
  const [newToken, setNewToken] = useState<string | null>(null);

  // 调试台
  const [playTool, setPlayTool] = useState<McpToolInfo | null>(null);
  const [playArgs, setPlayArgs] = useState("{}");
  const [playResult, setPlayResult] = useState<McpCallResult | null>(null);
  const [playBusy, setPlayBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [st, ts, cs] = await Promise.all([mcpApi.status(), mcpApi.listTools(), mcpApi.listCalls(50)]);
      setStatus(st);
      setTools(ts);
      setCalls(cs);
      setHost(st.host);
      setPort(st.configuredPort || st.port);
      setReadOnly(st.readOnly);
      setError(null);
    } catch (e) {
      setError(errOf(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const copy = async (text: string, key: string) => {
    try {
      await writeText(text);
      setCopied(key);
      setTimeout(() => setCopied((c) => (c === key ? null : c)), 1500);
    } catch {
      setError("复制失败，请手动选中复制");
    }
  };

  const run = async (key: string, act: () => Promise<unknown>) => {
    setBusy(key);
    setError(null);
    try {
      await act();
      await refresh();
    } catch (e) {
      setError(errOf(e));
    } finally {
      setBusy(null);
    }
  };

  const toggleTool = async (t: McpToolInfo, enabled: boolean) => {
    setTools((prev) => prev.map((x) => (x.name === t.name ? { ...x, enabled } : x)));
    try {
      await mcpApi.setToolEnabled(t.name, enabled);
      setStatus((s) => (s ? { ...s, toolEnabled: s.toolEnabled + (enabled ? 1 : -1) } : s));
    } catch (e) {
      setTools((prev) => prev.map((x) => (x.name === t.name ? { ...x, enabled: !enabled } : x)));
      setError(errOf(e));
    }
  };

  const saveConfig = async () => {
    setError(null);
    try {
      const needRestart = await mcpApi.saveConfig({ host, port, readOnly });
      setRestartHint(needRestart);
      await refresh();
    } catch (e) {
      setError(errOf(e));
    }
  };

  const rotateToken = async () => {
    setError(null);
    try {
      const t = await mcpApi.rotateToken();
      setNewToken(t);
      await refresh();
    } catch (e) {
      setError(errOf(e));
    }
  };

  const openPlayground = (t: McpToolInfo) => {
    setPlayTool(t);
    setPlayArgs(argsTemplate(t));
    setPlayResult(null);
  };

  const runPlayground = async () => {
    if (!playTool) return;
    let parsed: unknown;
    try {
      parsed = playArgs.trim() ? JSON.parse(playArgs) : {};
    } catch (e) {
      setPlayResult({ ok: false, result: null, error: `参数不是合法 JSON：${errOf(e)}`, durationMs: 0 });
      return;
    }
    setPlayBusy(true);
    try {
      const r = await mcpApi.callTool(playTool.name, parsed);
      setPlayResult(r);
      setCalls(await mcpApi.listCalls(50));
    } catch (e) {
      setPlayResult({ ok: false, result: null, error: errOf(e), durationMs: 0 });
    } finally {
      setPlayBusy(false);
    }
  };

  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return tools;
    return tools.filter(
      (t) => t.name.toLowerCase().includes(q) || t.description.toLowerCase().includes(q),
    );
  }, [tools, filter]);

  const statusBadge = status?.running
    ? "bg-emerald-50 text-emerald-700 dark:bg-emerald-950 dark:text-emerald-300"
    : status?.enabled
      ? "bg-amber-50 text-amber-700 dark:bg-amber-950 dark:text-amber-300"
      : "bg-inset text-muted ring-1 ring-edge";

  return (
    <div className="space-y-4">
      <PageHeader
        title="MCP 服务"
        description="把知识库能力以 Model Context Protocol 暴露给 Claude Desktop / Cline 等客户端：工具清单、逐工具开关、访问令牌、只读模式与调用记录都在这里配置。"
        actions={
          <Button variant="secondary" onClick={() => void refresh()}>
            <RefreshCw size={14} className="mr-1 inline" /> 刷新
          </Button>
        }
      />

      {error && (
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-2.5 text-sm text-red-700 dark:border-red-900 dark:bg-red-950 dark:text-red-300">
          {error}
        </div>
      )}

      {/* ---- 状态条 ---- */}
      <Card>
        <div className="flex flex-wrap items-center gap-3">
          <span className={`inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium ${statusBadge}`}>
            <span className={`h-1.5 w-1.5 rounded-full ${status?.running ? "bg-emerald-500" : "bg-amber-500"}`} />
            {status?.running ? "运行中" : status?.enabled ? "已启用未运行" : "已停用"}
          </span>
          {status && (
            <>
              <code className="rounded-md bg-inset px-2 py-1 font-mono text-xs text-sub ring-1 ring-edge">
                {status.endpoint}
              </code>
              <button
                onClick={() => void copy(status.endpoint, "endpoint")}
                className="flex items-center gap-1 rounded-md px-1.5 py-1 text-xs text-faint transition-colors hover:bg-hover hover:text-sub"
              >
                {copied === "endpoint" ? <CheckCircle2 size={13} className="text-green-600" /> : <Copy size={13} />}
                {copied === "endpoint" ? "已复制" : "复制"}
              </button>
              <span className="text-xs text-muted">
                工具 {status.toolEnabled}/{status.toolTotal} 启用 · 鉴权 {status.tokenSet ? "已开启" : "未开启"} ·
                {status.readOnly ? " 只读模式" : " 可写"}
              </span>
              {status.host === "0.0.0.0" && (
                <span className="flex items-center gap-1 text-xs text-amber-600 dark:text-amber-400">
                  <ShieldAlert size={13} /> 已开放局域网，建议设置访问令牌
                </span>
              )}
            </>
          )}
          <div className="ml-auto flex items-center gap-3">
            <span className="text-xs text-muted">服务开关</span>
            <Toggle
              checked={!!status?.enabled}
              disabled={busy === "service"}
              onChange={(v) => void run("service", () => api.setServiceEnabled("mcp", v))}
            />
            <Button
              variant="secondary"
              disabled={busy === "restart" || !status?.enabled}
              onClick={() => void run("restart", () => api.restartService("mcp"))}
            >
              {busy === "restart" ? <Loader size={14} className="mr-1 inline animate-spin" /> : null}
              重启
            </Button>
          </div>
        </div>
        {status?.error && (
          <p className="mt-2 text-xs text-red-600 dark:text-red-400">启动失败：{status.error}</p>
        )}
      </Card>

      {/* ---- 运行配置 ---- */}
      <Card>
        <h3 className="mb-3 text-sm font-semibold text-sub">运行配置</h3>
        <div className="grid gap-4 md:grid-cols-3">
          <label className="block">
            <span className="mb-1 block text-xs text-muted">监听范围</span>
            <select className={`${inputCls} w-full`} value={host} onChange={(e) => setHost(e.target.value)}>
              <option value="127.0.0.1">仅本机（127.0.0.1）</option>
              <option value="0.0.0.0">局域网（0.0.0.0）</option>
            </select>
            <span className="mt-1 block text-[11px] text-faint">
              开放局域网时请务必设置令牌；客户端仍填本机地址
            </span>
          </label>
          <label className="block">
            <span className="mb-1 block text-xs text-muted">端口</span>
            <input
              type="number"
              min={0}
              max={65535}
              className={`${inputCls} w-full`}
              value={port}
              onChange={(e) => setPort(Number(e.target.value) || 0)}
            />
            <span className="mt-1 block text-[11px] text-faint">0 = 随机端口（调试用）</span>
          </label>
          <div className="block">
            <span className="mb-1 block text-xs text-muted">只读模式</span>
            <div className="flex items-center gap-2 py-1.5">
              <Toggle checked={readOnly} onChange={setReadOnly} />
              <span className="text-xs text-faint">开启后写入/危险工具一律拒绝</span>
            </div>
          </div>
        </div>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Button onClick={() => void saveConfig()} disabled={busy === "save"}>
            {busy === "save" ? <Loader size={14} className="mr-1 inline animate-spin" /> : null}
            保存配置
          </Button>
          {restartHint && (
            <span className="text-xs text-amber-600 dark:text-amber-400">
              监听地址/端口已改，需点上方「重启」生效
            </span>
          )}
        </div>

        <div className="mt-4 border-t border-edge pt-4">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-xs text-muted">访问令牌</span>
            <code className="rounded bg-inset px-2 py-1 font-mono text-xs text-sub">
              {status?.tokenMasked ?? "（未设置，不校验 Bearer）"}
            </code>
            <Button variant="secondary" onClick={() => void rotateToken()} disabled={busy === "token"}>
              生成新令牌
            </Button>
            {status?.tokenSet && (
              <Button variant="secondary" onClick={() => void run("clear", () => mcpApi.clearToken())}>
                清除
              </Button>
            )}
          </div>
          {newToken && (
            <div className="mt-2 flex flex-wrap items-center gap-2 rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-xs text-amber-800 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-200">
              <span>令牌已生成，仅显示这一次，请立即复制：</span>
              <code className="font-mono">{newToken}</code>
              <button
                onClick={() => void copy(newToken, "token")}
                className="rounded px-1.5 py-0.5 hover:bg-amber-100 dark:hover:bg-amber-900"
              >
                {copied === "token" ? "已复制" : "复制"}
              </button>
              <button onClick={() => setNewToken(null)} className="ml-auto underline">
                我已保存
              </button>
            </div>
          )}
        </div>
      </Card>

      {/* ---- 工具清单 ---- */}
      <Card className="p-0">
        <div className="flex flex-wrap items-center gap-3 border-b border-edge px-4 py-3">
          <h3 className="text-sm font-semibold text-sub">工具清单</h3>
          <span className="text-xs text-faint">{shown.length} / {tools.length} 个</span>
          <input
            className={`${inputCls} ml-auto w-56`}
            placeholder="搜索工具名或描述…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
        <div className="divide-y divide-edge">
          {shown.map((t) => {
            const open = expanded === t.name;
            return (
              <div key={t.name} className="px-4 py-3">
                <div className="flex flex-wrap items-center gap-2">
                  <button
                    onClick={() => setExpanded(open ? null : t.name)}
                    className="flex items-center gap-1.5 text-left"
                  >
                    {open ? <ChevronDown size={14} className="text-faint" /> : <ChevronRight size={14} className="text-faint" />}
                    <code className="font-mono text-sm font-medium text-main">{t.name}</code>
                  </button>
                  <Badge className={KIND_STYLE[t.kind]}>{t.kindLabel}</Badge>
                  {t.costly && <Badge className="bg-indigo-50 text-indigo-700 dark:bg-indigo-950 dark:text-indigo-300">有外部费用</Badge>}
                  {t.audit && <Badge className="bg-inset text-muted ring-1 ring-edge">记审计</Badge>}
                  {!t.enabled && <Badge className="bg-inset text-faint ring-1 ring-edge">已禁用</Badge>}
                  <div className="ml-auto flex items-center gap-2">
                    <Button variant="secondary" onClick={() => openPlayground(t)} disabled={!t.enabled}>
                      <Play size={13} className="mr-1 inline" /> 试一试
                    </Button>
                    <Toggle checked={t.enabled} onChange={(v) => void toggleTool(t, v)} />
                  </div>
                </div>
                <p className="mt-1.5 text-xs text-muted">{t.description}</p>

                {open && (
                  <div className="mt-3 space-y-3 rounded-lg bg-inset/60 p-3">
                    {t.params.length > 0 ? (
                      <div className="overflow-x-auto">
                        <table className="w-full text-left text-xs">
                          <thead className="text-faint">
                            <tr>
                              <th className="py-1 pr-4 font-medium">参数</th>
                              <th className="py-1 pr-4 font-medium">类型</th>
                              <th className="py-1 pr-4 font-medium">必填</th>
                              <th className="py-1 font-medium">说明</th>
                            </tr>
                          </thead>
                          <tbody>
                            {t.params.map((p) => (
                              <tr key={p.name} className="border-t border-edge align-top">
                                <td className="py-1 pr-4 font-mono text-sub">{p.name}</td>
                                <td className="py-1 pr-4 text-muted">
                                  {p.type}
                                  {p.enumValues.length > 0 && (
                                    <span className="text-faint">（{p.enumValues.join(" | ")}）</span>
                                  )}
                                </td>
                                <td className="py-1 pr-4">{p.required ? <span className="text-red-600 dark:text-red-400">是</span> : <span className="text-faint">否</span>}</td>
                                <td className="py-1 text-muted">{p.description || "—"}</td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      </div>
                    ) : (
                      <p className="text-xs text-faint">该工具无需参数。</p>
                    )}
                    <div>
                      <div className="mb-1 flex items-center gap-2 text-xs text-muted">
                        调用示例
                        <button
                          onClick={() => void copy(curlExample(status!, t), `curl-${t.name}`)}
                          className="text-faint hover:text-sub"
                        >
                          {copied === `curl-${t.name}` ? "已复制" : "复制"}
                        </button>
                      </div>
                      <pre className="overflow-x-auto rounded bg-app p-2 font-mono text-[11px] text-sub ring-1 ring-edge">
                        {status ? curlExample(status, t) : ""}
                      </pre>
                    </div>
                  </div>
                )}
              </div>
            );
          })}
          {shown.length === 0 && <p className="px-4 py-6 text-center text-xs text-faint">没有匹配的工具</p>}
        </div>
      </Card>

      {/* ---- 客户端接入 ---- */}
      <Card>
        <div className="mb-2 flex items-center gap-2">
          <h3 className="text-sm font-semibold text-sub">客户端接入</h3>
          <button
            onClick={() => void copy(status ? clientConfig(status) : "", "client")}
            className="text-xs text-faint hover:text-sub"
          >
            {copied === "client" ? "已复制" : "复制配置"}
          </button>
        </div>
        <p className="mb-2 text-xs text-faint">
          Claude Desktop / Cline / Continue 的 MCP 配置片段（HTTP 传输）。令牌未设置时无需 headers。
        </p>
        <pre className="overflow-x-auto rounded bg-inset p-3 font-mono text-[11px] text-sub ring-1 ring-edge">
          {status ? clientConfig(status) : ""}
        </pre>
      </Card>

      {/* ---- 调用记录 ---- */}
      <Card className="p-0">
        <div className="flex items-center gap-3 border-b border-edge px-4 py-3">
          <h3 className="text-sm font-semibold text-sub">调用记录</h3>
          <span className="text-xs text-faint">最近 {calls.length} 条（不记录参数，避免密钥/正文进日志）</span>
          <Button variant="secondary" className="ml-auto" onClick={() => void refresh()}>
            刷新
          </Button>
        </div>
        <div className="divide-y divide-edge">
          {calls.map((c) => (
            <div key={c.id} className="flex flex-wrap items-center gap-2 px-4 py-2 text-xs">
              <span className="w-32 shrink-0 text-faint">{c.createdAt.slice(5, 19)}</span>
              <Badge className={c.ok ? "bg-emerald-50 text-emerald-700 dark:bg-emerald-950 dark:text-emerald-300" : "bg-red-50 text-red-700 dark:bg-red-950 dark:text-red-300"}>
                {c.ok ? "成功" : "失败"}
              </Badge>
              <code className="font-mono text-sub">{c.method}</code>
              {c.tool && <code className="font-mono text-muted">{c.tool}</code>}
              <span className="text-faint">{c.durationMs}ms</span>
              {c.client && <span className="truncate text-faint" title={c.client}>{c.client}</span>}
              {c.error && (
                <span className="min-w-0 flex-1 truncate text-red-600 dark:text-red-400" title={c.error}>
                  {c.error}
                </span>
              )}
            </div>
          ))}
          {calls.length === 0 && <p className="px-4 py-6 text-center text-xs text-faint">还没有调用记录</p>}
        </div>
      </Card>

      {/* ---- 调试台 ---- */}
      <Modal
        open={!!playTool}
        title={`调用 ${playTool?.name ?? ""}`}
        onClose={() => setPlayTool(null)}
        size="lg"
      >
        {playTool && (
          <div className="space-y-3">
            <p className="text-xs text-muted">{playTool.description}</p>
            <div>
              <div className="mb-1 text-xs text-muted">参数（JSON）</div>
              <textarea
                className={`${inputCls} h-32 w-full font-mono text-xs`}
                value={playArgs}
                onChange={(e) => setPlayArgs(e.target.value)}
              />
            </div>
            <div className="flex items-center gap-2">
              <Button onClick={() => void runPlayground()} disabled={playBusy}>
                {playBusy ? <Loader size={14} className="mr-1 inline animate-spin" /> : <Play size={14} className="mr-1 inline" />}
                执行
              </Button>
              <span className="text-xs text-faint">
                与外部客户端走同一条执行路径（只读模式、工具开关、审计与调用记录都生效）
              </span>
            </div>
            {playResult && (
              <div>
                <div className="mb-1 flex items-center gap-2 text-xs">
                  {playResult.ok ? (
                    <span className="text-emerald-600 dark:text-emerald-400">成功</span>
                  ) : (
                    <span className="text-red-600 dark:text-red-400">失败</span>
                  )}
                  <span className="text-faint">{playResult.durationMs}ms</span>
                </div>
                <pre className="max-h-80 overflow-auto rounded bg-inset p-2 font-mono text-[11px] text-sub ring-1 ring-edge">
                  {playResult.ok ? playResult.result : playResult.error}
                </pre>
              </div>
            )}
          </div>
        )}
      </Modal>
    </div>
  );
}
