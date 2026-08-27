import { useEffect, useMemo, useState } from "react";
import { Shield, Plus, Pencil, Trash2, AlertTriangle, CheckCircle2, Search } from "lucide-react";
import { api } from "../api";
import { PageHeader, Card, Toggle } from "../components/ui";
import { Button, Modal } from "../components/Modal";
import type { AuthEvent, AdminAudit, SecurityLog, SecurityRule } from "../types";

const fmt = (n: number) => n.toLocaleString("en-US");
const inputCls =
  "w-full rounded-md border border-gray-300 px-3 py-1.5 text-sm focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500";

// ---------------- 徽章 ----------------

const riskStyles: Record<string, string> = {
  low: "bg-green-50 text-green-700 ring-green-200",
  medium: "bg-amber-50 text-amber-700 ring-amber-200",
  high: "bg-red-50 text-red-700 ring-red-200",
};
function RiskBadge({ risk }: { risk: string }) {
  const label = { low: "低", medium: "中", high: "高" }[risk] ?? risk;
  return (
    <span className={`inline-flex rounded-md px-2 py-0.5 text-xs font-medium ring-1 ${riskStyles[risk] ?? "bg-gray-50 text-gray-600 ring-gray-200"}`}>
      {label}
    </span>
  );
}

const actionStyles: Record<string, string> = {
  detected: "bg-gray-50 text-gray-600 ring-gray-200",
  masked: "bg-sky-50 text-sky-700 ring-sky-200",
  blocked: "bg-red-50 text-red-700 ring-red-200",
};
const actionLabels: Record<string, string> = {
  detected: "仅记录",
  masked: "已脱敏",
  blocked: "已拦截",
  record: "仅记录",
  mask: "脱敏",
  block: "拦截",
};
function ActionBadge({ action }: { action: string }) {
  return (
    <span className={`inline-flex rounded-md px-2 py-0.5 text-xs font-medium ring-1 ${actionStyles[action] ?? "bg-gray-50 text-gray-600 ring-gray-200"}`}>
      {actionLabels[action] ?? action}
    </span>
  );
}

// ---------------- 事件 Tab ----------------

function EventsTab() {
  const [logs, setLogs] = useState<SecurityLog[]>([]);
  const [stats, setStats] = useState<{ byRisk: [string, number][]; byAction: [string, number][]; daily: [string, number][] } | null>(null);
  const [risk, setRisk] = useState("全部");
  const [stage, setStage] = useState("全部");

  const load = () => {
    api
      .listSecurityLogs({ risk: risk === "全部" ? undefined : risk, stage: stage === "全部" ? undefined : stage })
      .then(setLogs)
      .catch(() => {});
    api.securityStats(7).then(setStats).catch(() => {});
  };
  useEffect(load, [risk, stage]);

  const byRisk = useMemo(() => new Map(stats?.byRisk ?? []), [stats]);
  const byAction = useMemo(() => new Map(stats?.byAction ?? []), [stats]);
  const total = useMemo(() => (stats?.byRisk ?? []).reduce((s, [, c]) => s + c, 0), [stats]);
  const maxDaily = Math.max(1, ...(stats?.daily ?? []).map(([, c]) => c));

  return (
    <div>
      <div className="mb-4 grid grid-cols-1 gap-3 sm:grid-cols-4">
        <Card>
          <p className="text-xs text-gray-500">近 7 天事件</p>
          <p className="mt-1 text-2xl font-bold text-gray-900">{fmt(total)}</p>
        </Card>
        <Card>
          <p className="text-xs text-gray-500">高风险命中</p>
          <p className="mt-1 text-2xl font-bold text-red-600">{fmt(byRisk.get("high") ?? 0)}</p>
        </Card>
        <Card>
          <p className="text-xs text-gray-500">已拦截</p>
          <p className="mt-1 text-2xl font-bold text-red-600">{fmt(byAction.get("blocked") ?? 0)}</p>
        </Card>
        <Card>
          <p className="text-xs text-gray-500">已脱敏</p>
          <p className="mt-1 text-2xl font-bold text-sky-600">{fmt(byAction.get("masked") ?? 0)}</p>
        </Card>
      </div>

      <Card className="mb-4">
        <h3 className="mb-2 flex items-center gap-2 text-sm font-semibold text-gray-700">
          <AlertTriangle size={15} /> 近 7 天安全事件趋势
        </h3>
        <div className="flex h-32 items-end gap-2">
          {(stats?.daily ?? []).map(([date, c], i) => (
            <div key={i} className="flex h-full min-w-0 flex-1 flex-col items-center">
              <div className="relative h-24 w-full">
                <div
                  className="absolute inset-x-0 bottom-0 rounded-t-md bg-red-400/80"
                  style={{ height: `${(c / maxDaily) * 100}%`, minHeight: "4px" }}
                  title={`${date}: ${c} 条`}
                />
              </div>
              <span className="mt-1 text-[10px] text-gray-400">{date.slice(5)}</span>
            </div>
          ))}
          {(stats?.daily ?? []).length === 0 && <p className="w-full py-8 text-center text-sm text-gray-400">暂无数据</p>}
        </div>
      </Card>

      <Card className="mb-4">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          <select value={risk} onChange={(e) => setRisk(e.target.value)} className={inputCls}>
            <option value="全部">全部风险</option>
            <option value="low">低</option>
            <option value="medium">中</option>
            <option value="high">高</option>
          </select>
          <select value={stage} onChange={(e) => setStage(e.target.value)} className={inputCls}>
            <option value="全部">全部阶段</option>
            <option value="request">请求侧</option>
            <option value="response">响应侧</option>
          </select>
          <p className="self-center text-xs text-gray-400">共 {logs.length} 条（按时间倒序，最多 500）</p>
        </div>
      </Card>

      <Card className="p-0 overflow-hidden">
        <table className="w-full text-left text-sm">
          <thead className="border-b border-gray-200 bg-gray-50 text-xs text-gray-500">
            <tr>
              <th className="px-3 py-2 font-medium">时间</th>
              <th className="px-3 py-2 font-medium">阶段</th>
              <th className="px-3 py-2 font-medium">规则</th>
              <th className="px-3 py-2 font-medium">类别</th>
              <th className="px-3 py-2 font-medium">风险</th>
              <th className="px-3 py-2 font-medium">处置</th>
              <th className="px-3 py-2 font-medium">命中片段（脱敏）</th>
              <th className="px-3 py-2 font-medium">请求ID</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-100">
            {logs.map((l) => (
              <tr key={l.id} className="hover:bg-gray-50">
                <td className="px-3 py-2 whitespace-nowrap text-gray-600">{l.createdAt.slice(5, 16)}</td>
                <td className="px-3 py-2 text-gray-600">{l.stage === "request" ? "请求侧" : "响应侧"}</td>
                <td className="px-3 py-2">
                  <code className="font-mono text-xs text-gray-700">{l.ruleId}</code>
                </td>
                <td className="px-3 py-2 text-gray-600">{l.category}</td>
                <td className="px-3 py-2"><RiskBadge risk={l.risk} /></td>
                <td className="px-3 py-2"><ActionBadge action={l.action} /></td>
                <td className="px-3 py-2">
                  <code className="font-mono text-xs text-gray-500">{l.snippetMasked}</code>
                </td>
                <td className="px-3 py-2 font-mono text-xs text-gray-400">{l.requestId.slice(4, 16)}</td>
              </tr>
            ))}
            {logs.length === 0 && (
              <tr>
                <td colSpan={8} className="px-4 py-8 text-center text-sm text-gray-400">
                  暂无安全事件
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </Card>
    </div>
  );
}

// ---------------- 规则 Tab ----------------

function RuleModal({
  open,
  initial,
  onClose,
  onSave,
}: {
  open: boolean;
  initial: SecurityRule | null;
  onClose: () => void;
  onSave: (data: { category: string; risk: string; pattern: string; action: string; matchType: string }) => Promise<void>;
}) {
  const [category, setCategory] = useState("secret");
  const [risk, setRisk] = useState("medium");
  const [action, setAction] = useState("mask");
  const [matchType, setMatchType] = useState("regex");
  const [pattern, setPattern] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [lastOpen, setLastOpen] = useState(open);
  if (open !== lastOpen) {
    setLastOpen(open);
    if (open) {
      setCategory(initial?.category ?? "secret");
      setRisk(initial?.risk ?? "medium");
      setAction(initial?.action ?? "mask");
      setMatchType(initial?.matchType ?? "regex");
      setPattern(initial?.pattern ?? "");
      setError(null);
    }
  }

  return (
    <Modal open={open} title={initial ? "编辑规则" : "新增规则"} onClose={onClose}>
      <form
        className="space-y-4"
        onSubmit={async (e) => {
          e.preventDefault();
          setSubmitting(true);
          setError(null);
          try {
            await onSave({ category, risk, pattern, action, matchType });
          } catch (err) {
            setError(err instanceof Error ? err.message : String(err));
            setSubmitting(false);
            return;
          }
          setSubmitting(false);
        }}
      >
        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-gray-700">类别</span>
            <select value={category} onChange={(e) => setCategory(e.target.value)} className={inputCls}>
              <option value="secret">密钥/凭据</option>
              <option value="pii">个人信息</option>
              <option value="prompt_injection">提示注入</option>
              <option value="encoding">隐写/混淆</option>
              <option value="command">带毒命令</option>
            </select>
          </label>
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-gray-700">匹配方式</span>
            <select value={matchType} onChange={(e) => setMatchType(e.target.value)} className={inputCls}>
              <option value="regex">正则表达式</option>
              <option value="substring">子串匹配（大小写不敏感）</option>
            </select>
          </label>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-gray-700">风险</span>
            <select value={risk} onChange={(e) => setRisk(e.target.value)} className={inputCls}>
              <option value="low">低</option>
              <option value="medium">中</option>
              <option value="high">高</option>
            </select>
          </label>
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-gray-700">处置</span>
            <select value={action} onChange={(e) => setAction(e.target.value)} className={inputCls}>
              <option value="record">仅记录</option>
              <option value="mask">脱敏</option>
              <option value="block">拦截</option>
            </select>
          </label>
        </div>
        <label className="block">
          <span className="mb-1 block text-sm font-medium text-gray-700">
            {matchType === "regex" ? "正则表达式" : "匹配子串"}
          </span>
          <textarea
            required
            value={pattern}
            onChange={(e) => setPattern(e.target.value)}
            className={`${inputCls} font-mono text-xs`}
            rows={3}
            placeholder={matchType === "regex" ? "如 \\b1[3-9]\\d{9}\\b" : "如 忽略以上所有指令（精确出现即命中）"}
          />
          <span className="mt-0.5 block text-xs text-gray-400">
            {matchType === "regex"
              ? "正则引擎（secret/pii/prompt_injection/encoding/command 按类别分流）"
              : "子串引擎（Aho-Corasick 多模式匹配，一次性线性扫描，适合固定短语/命令/密钥块头）"}
          </span>
        </label>
        {error && <p className="text-sm text-red-600">{error}</p>}
        <div className="flex justify-end gap-2 pt-2">
          <Button variant="secondary" onClick={onClose}>取消</Button>
          <Button type="submit" disabled={submitting}>
            {submitting ? "保存中…" : "保存"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function RulesTab() {
  const [rules, setRules] = useState<SecurityRule[]>([]);
  const [loading, setLoading] = useState(true);
  const [modalOpen, setModalOpen] = useState(false);
  const [editing, setEditing] = useState<SecurityRule | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [keyword, setKeyword] = useState("");
  const [catFilter, setCatFilter] = useState("全部");
  const [matchFilter, setMatchFilter] = useState("全部");

  const load = () => {
    api.listSecurityRules().then(setRules).catch(() => {}).finally(() => setLoading(false));
  };
  useEffect(load, []);

  // 本地过滤：按规则ID/匹配内容搜索 + 类别/方式筛选（规则量小，无需后端分页）
  const filtered = useMemo(() => {
    const kw = keyword.trim().toLowerCase();
    return rules.filter((r) => {
      const matchKw =
        !kw || r.id.toLowerCase().includes(kw) || r.pattern.toLowerCase().includes(kw);
      const matchCat = catFilter === "全部" || r.category === catFilter;
      const matchType = matchFilter === "全部" || r.matchType === matchFilter;
      return matchKw && matchCat && matchType;
    });
  }, [rules, keyword, catFilter, matchFilter]);

  const enabledCount = useMemo(() => rules.filter((r) => r.enabled).length, [rules]);

  // 多选与批量启停
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const allSelected = filtered.length > 0 && filtered.every((r) => selected.has(r.id));
  const toggleSelect = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };
  const toggleSelectAll = () => {
    setSelected(allSelected ? new Set() : new Set(filtered.map((r) => r.id)));
  };
  const batchSet = async (enabled: boolean) => {
    if (selected.size === 0) return;
    try {
      await api.setSecurityRulesEnabled(Array.from(selected), enabled);
      setSelected(new Set());
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const save = async (data: { category: string; risk: string; pattern: string; action: string; matchType: string }) => {
    if (editing) {
      await api.updateSecurityRule({ id: editing.id, ...data });
    } else {
      await api.addSecurityRule(data);
    }
    setModalOpen(false);
    load();
  };

  const toggle = async (r: SecurityRule) => {
    try {
      await api.toggleSecurityRule(r.id);
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const remove = async (r: SecurityRule) => {
    try {
      await api.deleteSecurityRule(r.id);
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const catLabels: Record<string, string> = {
    secret: "密钥/凭据",
    pii: "个人信息",
    prompt_injection: "提示注入",
    encoding: "隐写/混淆",
    command: "带毒命令",
  };

  return (
    <div>
      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-2.5 text-sm text-red-700">{error}</div>
      )}
      <Card className="mb-4">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-4">
          <div className="relative sm:col-span-2">
            <Search size={15} className="absolute left-3 top-1/2 -translate-y-1/2 text-gray-400" />
            <input
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
              className={`${inputCls} pl-8`}
              placeholder="按规则ID / 匹配内容搜索"
            />
          </div>
          <select value={catFilter} onChange={(e) => setCatFilter(e.target.value)} className={inputCls}>
            <option value="全部">全部类别</option>
            {Object.entries(catLabels).map(([k, v]) => (
              <option key={k} value={k}>{v}</option>
            ))}
          </select>
          <select value={matchFilter} onChange={(e) => setMatchFilter(e.target.value)} className={inputCls}>
            <option value="全部">全部方式</option>
            <option value="regex">正则</option>
            <option value="substring">子串</option>
          </select>
        </div>
        <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
          <p className="text-xs text-gray-400">
            共 {filtered.length} 条（全部 {rules.length} · 启用 {enabledCount}）· 启停 30 秒内自动生效
          </p>
          <Button
            onClick={() => {
              setEditing(null);
              setModalOpen(true);
            }}
          >
            <Plus size={16} className="mr-1 inline" /> 新增规则
          </Button>
        </div>
        {selected.size > 0 && (
          <div className="mt-3 flex flex-wrap items-center justify-between gap-3 rounded-lg bg-indigo-50 px-3 py-2 ring-1 ring-indigo-100">
            <span className="text-sm font-medium text-indigo-700">已选 {selected.size} 条规则</span>
            <div className="flex gap-2">
              <Button onClick={() => batchSet(true)}>
                <CheckCircle2 size={15} className="mr-1 inline" /> 批量启用
              </Button>
              <Button variant="secondary" onClick={() => batchSet(false)}>
                批量停用
              </Button>
              <Button variant="secondary" onClick={() => setSelected(new Set())}>
                取消选择
              </Button>
            </div>
          </div>
        )}
      </Card>

      <Card className="p-0 overflow-hidden">
        <table className="w-full text-left text-sm">
          <thead className="border-b border-gray-200 bg-gray-50 text-xs text-gray-500">
            <tr>
              <th className="w-8 px-3 py-2">
                <input
                  type="checkbox"
                  checked={allSelected}
                  onChange={toggleSelectAll}
                  className="h-3.5 w-3.5 rounded border-gray-300 text-indigo-600 focus:ring-indigo-500"
                  title="全选当前列表"
                />
              </th>
              <th className="px-3 py-2 font-medium">规则ID</th>
              <th className="px-3 py-2 font-medium">类别</th>
              <th className="px-3 py-2 font-medium">方式</th>
              <th className="px-3 py-2 font-medium">风险</th>
              <th className="px-3 py-2 font-medium">处置</th>
              <th className="px-3 py-2 font-medium">匹配内容</th>
              <th className="px-3 py-2 font-medium">启用</th>
              <th className="px-3 py-2 text-right font-medium">操作</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-100">
            {filtered.map((r) => (
              <tr
                key={r.id}
                className={`hover:bg-gray-50 ${r.enabled ? "" : "opacity-50"} ${selected.has(r.id) ? "bg-indigo-50/50" : ""}`}
              >
                <td className="px-3 py-2">
                  <input
                    type="checkbox"
                    checked={selected.has(r.id)}
                    onChange={() => toggleSelect(r.id)}
                    className="h-3.5 w-3.5 rounded border-gray-300 text-indigo-600 focus:ring-indigo-500"
                  />
                </td>
                <td className="px-3 py-2"><code className="font-mono text-xs text-gray-700">{r.id}</code></td>
                <td className="px-3 py-2 text-gray-600">{catLabels[r.category] ?? r.category}</td>
                <td className="px-3 py-2">
                  <span className={`rounded-md px-2 py-0.5 text-xs font-medium ring-1 ${r.matchType === "substring" ? "bg-violet-50 text-violet-700 ring-violet-200" : "bg-gray-50 text-gray-600 ring-gray-200"}`}>
                    {r.matchType === "substring" ? "子串" : "正则"}
                  </span>
                </td>
                <td className="px-3 py-2"><RiskBadge risk={r.risk} /></td>
                <td className="px-3 py-2"><ActionBadge action={r.action} /></td>
                <td className="px-3 py-2">
                  <code className="block max-w-72 truncate font-mono text-xs text-gray-500" title={r.pattern}>
                    {r.pattern}
                  </code>
                </td>
                <td className="px-3 py-2">
                  <Toggle checked={r.enabled} onChange={() => toggle(r)} />
                </td>
                <td className="px-3 py-2">
                  <div className="flex justify-end gap-1">
                    <button
                      className="rounded-md p-1.5 text-gray-400 hover:bg-indigo-50 hover:text-indigo-600"
                      onClick={() => {
                        setEditing(r);
                        setModalOpen(true);
                      }}
                      title="编辑"
                    >
                      <Pencil size={15} />
                    </button>
                    <button
                      className="rounded-md p-1.5 text-gray-400 hover:bg-red-50 hover:text-red-600"
                      onClick={() => remove(r)}
                      title="删除"
                    >
                      <Trash2 size={15} />
                    </button>
                  </div>
                </td>
              </tr>
            ))}
            {!loading && filtered.length === 0 && (
              <tr>
                <td colSpan={9} className="px-4 py-8 text-center text-sm text-gray-400">
                  {rules.length === 0 ? "暂无规则，点击右上角新增" : "没有符合筛选条件的规则"}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </Card>

      <RuleModal
        open={modalOpen}
        initial={editing}
        onClose={() => setModalOpen(false)}
        onSave={save}
      />
    </div>
  );
}

// ---------------- 审计 Tab ----------------

function AuditTab() {
  const [authEvents, setAuthEvents] = useState<AuthEvent[]>([]);
  const [audits, setAudits] = useState<AdminAudit[]>([]);

  useEffect(() => {
    api.listAuthEvents(50).then(setAuthEvents).catch(() => {});
    api.listAdminAudits(50).then(setAudits).catch(() => {});
  }, []);

  const targetLabels: Record<string, string> = { channel: "渠道", key: "密钥", setting: "设置", security_rule: "安全规则" };
  const actionLabels: Record<string, string> = {
    create: "创建",
    update: "修改",
    delete: "删除",
    toggle: "切换状态",
    rotate: "轮换",
    set: "设置",
  };

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
      <Card className="p-0 overflow-hidden">
        <h3 className="flex items-center gap-2 border-b border-gray-200 bg-gray-50 px-3 py-2 text-sm font-semibold text-gray-700">
          <CheckCircle2 size={15} /> 鉴权事件（最近 50 条）
        </h3>
        <table className="w-full text-left text-sm">
          <tbody className="divide-y divide-gray-100">
            {authEvents.map((e) => (
              <tr key={e.id} className="hover:bg-gray-50">
                <td className="px-3 py-1.5 whitespace-nowrap text-gray-500">{e.createdAt.slice(5, 16)}</td>
                <td className="px-3 py-1.5">
                  {e.success ? (
                    <span className="text-green-600">成功</span>
                  ) : (
                    <span className="text-red-600">失败 · {e.reason ?? "未知"}</span>
                  )}
                </td>
                <td className="px-3 py-1.5 font-mono text-xs text-gray-400">{e.keyId ?? "（匿名）"}</td>
              </tr>
            ))}
            {authEvents.length === 0 && (
              <tr>
                <td colSpan={3} className="px-4 py-8 text-center text-sm text-gray-400">暂无鉴权事件</td>
              </tr>
            )}
          </tbody>
        </table>
      </Card>

      <Card className="p-0 overflow-hidden">
        <h3 className="flex items-center gap-2 border-b border-gray-200 bg-gray-50 px-3 py-2 text-sm font-semibold text-gray-700">
          <Shield size={15} /> 管理操作审计（最近 50 条）
        </h3>
        <table className="w-full text-left text-sm">
          <tbody className="divide-y divide-gray-100">
            {audits.map((a) => (
              <tr key={a.id} className="hover:bg-gray-50">
                <td className="px-3 py-1.5 whitespace-nowrap text-gray-500">{a.createdAt.slice(5, 16)}</td>
                <td className="px-3 py-1.5 text-gray-700">
                  {actionLabels[a.action] ?? a.action}
                  <span className="text-xs text-gray-400"> {targetLabels[a.targetType] ?? a.targetType}</span>
                </td>
                <td className="px-3 py-1.5">
                  <code className="font-mono text-xs text-gray-500">{a.detail ?? a.targetId ?? ""}</code>
                </td>
              </tr>
            ))}
            {audits.length === 0 && (
              <tr>
                <td colSpan={3} className="px-4 py-8 text-center text-sm text-gray-400">暂无管理操作记录</td>
              </tr>
            )}
          </tbody>
        </table>
      </Card>
    </div>
  );
}

// ---------------- 页面 ----------------

export function Security() {
  const [tab, setTab] = useState<"events" | "rules" | "audit">("events");
  const tabs = [
    { key: "events", label: "安全事件" },
    { key: "rules", label: "检测规则" },
    { key: "audit", label: "审计日志" },
  ] as const;

  return (
    <div>
      <PageHeader
        title="安全"
        description="多引擎安全审计：检测（规则/隐写/命令/泄露比对）→ 策略决策 → 审计留痕"
      />
      <div className="mb-4 flex flex-wrap gap-1 border-b border-gray-200">
        {tabs.map((t) => (
          <button
            key={t.key}
            onClick={() => setTab(t.key)}
            className={`-mb-px border-b-2 px-4 py-2 text-sm font-medium transition-colors ${
              tab === t.key ? "border-indigo-600 text-indigo-700" : "border-transparent text-gray-500 hover:text-gray-800"
            }`}
          >
            {t.label}
          </button>
        ))}
      </div>
      {tab === "events" && <EventsTab />}
      {tab === "rules" && <RulesTab />}
      {tab === "audit" && <AuditTab />}
    </div>
  );
}
