import { useEffect, useMemo, useRef, useState } from "react";
import { Pencil, Plus, Trash2, Cable, Search, ChevronDown, RefreshCw } from "lucide-react";
import { channelTypes, providerPresets, protocolOptions, autoProtocol } from "../data/mock";
import type { Channel, ChannelProtocol, ChannelStatus, ChannelType } from "../types";
import { useAppStore } from "../store/useAppStore";
import { PageHeader, Card } from "../components/ui";
import { Button, Modal } from "../components/Modal";
import { ChannelTypeBadge, StatusBadge } from "../components/Badge";
import { api } from "../api";

type FormState = {
  name: string;
  type: ChannelType;
  /** 显式协议；"" = 按渠道类型自动推导 */
  protocol: ChannelProtocol | "";
  baseUrl: string;
  model: string;
  apiKey: string;
  status: ChannelStatus;
  priority: number;
  weight: number;
  // 以下三个价格以"元/M tokens"输入（文本格式，无步进计数器），提交时换算回每 1k 存储
  pricePerM: string;
  cachePricePerM: string;
  reasoningPricePerM: string;
};

const emptyForm: FormState = {
  name: "",
  type: "OpenAI",
  protocol: "",
  baseUrl: "",
  model: "",
  apiKey: "",
  status: "active",
  priority: 0,
  weight: 1,
  pricePerM: "",
  cachePricePerM: "",
  reasoningPricePerM: "",
};

/** 每 1k 单价（存储）→ 元/M 展示值，去尾零 */
const perM = (per1k: number) => (per1k > 0 ? parseFloat((per1k * 1000).toFixed(6)) : 0);
/** 元/M 输入 → 每 1k 存储值 */
const per1k = (perM: number) => Math.round((perM / 1000) * 1e9) / 1e9;

const inputCls =
  "w-full rounded-md border border-strong px-3 py-1.5 text-sm focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500";

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <span className="mb-1 block text-sm font-medium text-sub">{label}</span>
      {children}
      {hint && <span className="mt-0.5 block text-xs text-faint">{hint}</span>}
    </label>
  );
}

/** 手风琴折叠区（默认收起，减少表单字段数量） */
function Accordion({ title, children }: { title: string; children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="overflow-hidden rounded-lg border border-edge">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center justify-between px-3 py-2 text-sm font-medium text-sub transition-colors hover:bg-hover"
      >
        {title}
        <ChevronDown size={16} className={`shrink-0 text-faint transition-transform ${open ? "rotate-180" : ""}`} />
      </button>
      {open && <div className="border-t border-edge p-3">{children}</div>}
    </div>
  );
}

function ChannelFormModal({
  open,
  initial,
  channelId,
  error,
  onClose,
  onSubmit,
  submitting,
}: {
  open: boolean;
  initial: FormState | null;
  /** 编辑时的渠道 id（用于「获取模型」读取已保存的 Key） */
  channelId?: string;
  /** 提交失败的错误信息（在弹窗内展示，避免被遮罩挡住） */
  error?: string | null;
  onClose: () => void;
  onSubmit: (data: FormState) => Promise<void>;
  submitting: boolean;
}) {
  const [form, setForm] = useState<FormState>(initial ?? emptyForm);
  const [models, setModels] = useState<string[]>([]);
  const [modelErr, setModelErr] = useState<string | null>(null);
  const [loadingModels, setLoadingModels] = useState(false);

  // Reset form when opening with a fresh target.
  const [lastOpen, setLastOpen] = useState(open);
  if (open !== lastOpen) {
    setLastOpen(open);
    if (open) {
      setForm(initial ?? emptyForm);
      setModels([]);
      setModelErr(null);
      setLoadingModels(false);
    }
  }

  const set = (k: keyof FormState) => (e: React.ChangeEvent<HTMLInputElement | HTMLSelectElement>) =>
    setForm((f) => ({ ...f, [k]: e.target.value }));

  const preset = providerPresets[form.type] ?? providerPresets.Custom;
  // 所有内置厂商默认地址：切换类型时只有「空地址」或「仍是内置默认」才自动替换，避免覆盖用户手填
  const defaultUrls = useMemo(
    () => Object.values(providerPresets).map((p) => p.baseUrl).filter(Boolean),
    [],
  );

  const changeType = (t: ChannelType) => {
    setForm((f) => {
      const baseUrl =
        !f.baseUrl.trim() || defaultUrls.includes(f.baseUrl.trim()) ? providerPresets[t].baseUrl : f.baseUrl;
      // 类型变化时协议回到「自动」（避免 Claude/Gemini 类型残留显式协议导致测试失败）
      return { ...f, type: t, protocol: "", baseUrl };
    });
    setModels([]);
    setModelErr(null);
  };

  const changeProtocol = (p: ChannelProtocol | "") => {
    setForm((f) => ({ ...f, protocol: p }));
    // 协议变化后旧的模型列表可能来自不同端点，清掉避免误导
    setModels([]);
    setModelErr(null);
  };

  const loadModels = async () => {
    setLoadingModels(true);
    setModelErr(null);
    try {
      const r = await api.fetchChannelModels({
        baseUrl: form.baseUrl.trim() || undefined,
        apiKey: form.apiKey.trim() || undefined,
        channelId: channelId || undefined,
      });
      setModels(r.models);
    } catch (e) {
      setModelErr(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingModels(false);
    }
  };

  // 生效协议：显式优先，否则按渠道类型推导（与后端一致）
  const effectiveProtocol = form.protocol || autoProtocol(form.type);
  const isDeepSeek = effectiveProtocol === "deepseek";
  const isNativeUnimplemented = effectiveProtocol === "anthropic" || effectiveProtocol === "gemini";

  // DeepSeek 模型列表不写死：编辑已有渠道（Key 已保存）时自动拉取；
  // 新建渠道则等 API Key 填到合理长度后自动拉取一次，失败仍可点「获取模型」重试。
  const autoFetchedModels = useRef(false);
  useEffect(() => {
    if (!open) {
      autoFetchedModels.current = false;
      return;
    }
    if (autoFetchedModels.current || !isDeepSeek) return;
    if (!form.baseUrl.trim().startsWith("http")) return;
    const hasKey = !!channelId || form.apiKey.trim().length >= 20;
    if (!hasKey) return;
    autoFetchedModels.current = true;
    void loadModels();
    // 只依赖触发条件；loadModels 每次渲染重建，不放入依赖避免重复请求
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, isDeepSeek, channelId, form.apiKey, form.baseUrl]);

  const chip =
    "rounded-md border border-strong px-2 py-0.5 font-mono text-[11px] text-sub transition-colors hover:bg-accent-soft hover:text-accent";

  return (
    <Modal open={open} title={initial ? "编辑渠道" : "添加渠道"} onClose={onClose}>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          onSubmit(form);
        }}
      >
        {error && (
          <div className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-700 dark:border-red-900 dark:bg-red-950 dark:text-red-300">
            {error}
          </div>
        )}
        <div className="grid grid-cols-2 gap-4">
          <Field label="渠道名称">
            <input required value={form.name} onChange={set("name")} className={inputCls} placeholder="如 OpenAI Main" />
          </Field>
          <Field label="渠道类型">
            <select value={form.type} onChange={(e) => changeType(e.target.value as ChannelType)} className={inputCls}>
              {channelTypes.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <div className="grid grid-cols-2 gap-4">
          <Field label="渠道协议" hint="决定连通性测试 / 知识库问答使用的适配器；代理转发统一走 OpenAI 兼容 /chat/completions">
            <select
              value={form.protocol}
              onChange={(e) => changeProtocol(e.target.value as FormState["protocol"])}
              className={inputCls}
            >
              {protocolOptions.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </Field>
          <Field label="状态">
            <select value={form.status} onChange={set("status")} className={inputCls}>
              <option value="active">启用</option>
              <option value="disabled">停用</option>
              <option value="error">异常</option>
            </select>
          </Field>
        </div>
        {isNativeUnimplemented && (
          <p className="rounded-md bg-amber-50 px-3 py-2 text-xs text-amber-700 dark:bg-amber-950 dark:text-amber-300">
            原生协议适配器（Anthropic / Gemini）尚未实现：连通性测试与知识库问答会失败。可把协议改为「OpenAI
            兼容」，接入厂商的 OpenAI 兼容端点后使用。
          </p>
        )}
        <Field
          label="Base URL"
          hint="必须以 http:// 或 https:// 开头；网关会在此基础上拼接 /chat/completions（OpenAI 基址需含 /v1）"
        >
          <input
            required
            value={form.baseUrl}
            onChange={set("baseUrl")}
            className={inputCls}
            placeholder={preset.baseUrl || "https://api.example.com/v1"}
            pattern="https?://.+"
            title="请输入合法的 http(s) 地址"
          />
        </Field>
        <Field
          label="默认模型"
          hint={
            isDeepSeek
              ? "留空 = 匹配任意模型；DeepSeek 模型由上游 /models 自动获取，也可手动填写"
              : "留空 = 匹配任意模型；建议填上游真实模型 ID"
          }
        >
          <div className="flex gap-2">
            <input
              list="channel-model-options"
              value={form.model}
              onChange={set("model")}
              className={inputCls}
              placeholder={preset.models[0] ?? "如 gpt-4o / deepseek-chat"}
            />
            <Button
              variant="secondary"
              className="shrink-0"
              disabled={loadingModels}
              onClick={loadModels}
              title="用当前 Base URL 与 Key 拉取上游 /models"
            >
              <RefreshCw size={13} className={`mr-1 inline ${loadingModels ? "animate-spin" : ""}`} />
              {loadingModels ? "获取中" : "获取模型"}
            </Button>
          </div>
        </Field>
        {isDeepSeek && models.length === 0 && !loadingModels && (
          <p className="text-xs text-faint">
            DeepSeek 模型列表通过上游接口获取：{channelId ? "打开编辑时自动加载" : "填写 API Key 后自动加载"}
            ，也可点「获取模型」。
          </p>
        )}
        <datalist id="channel-model-options">
          {[...new Set([...models, ...preset.models])].map((m) => (
            <option key={m} value={m} />
          ))}
        </datalist>
        {modelErr && (
          <p className="rounded-md bg-red-50 px-3 py-2 text-xs text-red-700 dark:bg-red-950 dark:text-red-300">
            模型列表获取失败：{modelErr}（可手动填写模型 ID）
          </p>
        )}
        {models.length > 0 && (
          <div>
            <p className="mb-1 text-xs text-faint">上游返回的模型（点击填入）：</p>
            <div className="flex max-h-24 flex-wrap gap-1.5 overflow-y-auto">
              {models.map((m) => (
                <button key={m} type="button" onClick={() => setForm((f) => ({ ...f, model: m }))} className={chip}>
                  {m}
                </button>
              ))}
            </div>
          </div>
        )}
        {preset.models.length > 0 && (
          <div>
            <p className="mb-1 text-xs text-faint">常用模型（点击填入）：</p>
            <div className="flex flex-wrap gap-1.5">
              {preset.models.map((m) => (
                <button key={m} type="button" onClick={() => setForm((f) => ({ ...f, model: m }))} className={chip}>
                  {m}
                </button>
              ))}
            </div>
          </div>
        )}
        <Accordion title="负载均衡配置（优先级 / 权重）">
          <div className="grid grid-cols-2 gap-4">
            <Field label="优先级" hint="故障转移时越大的越先尝试（主渠道 = 最高优先级）">
              <input
                value={form.priority}
                onChange={(e) => setForm((f) => ({ ...f, priority: Number(e.target.value) || 0 }))}
                className={inputCls}
                type="number"
                placeholder="0"
              />
            </Field>
            <Field label="权重" hint="同优先级内越大越可能被先选中">
              <input
                value={form.weight}
                onChange={(e) => setForm((f) => ({ ...f, weight: Number(e.target.value) || 0 }))}
                className={inputCls}
                type="number"
                placeholder="1"
              />
            </Field>
          </div>
        </Accordion>
        <Accordion title="价格配置（元/M tokens）">
          <div className="space-y-4">
            <Field label="单价" hint="每百万 token 成本；记账成本 = tokens ÷ 1,000,000 × 单价">
              <input
                value={form.pricePerM}
                onChange={(e) => setForm((f) => ({ ...f, pricePerM: e.target.value }))}
                className={inputCls}
                type="text"
                inputMode="decimal"
                placeholder="0"
              />
            </Field>
            <Field label="缓存命中单价" hint="留 0 = 不区分缓存，命中部分按正常单价计费">
              <input
                value={form.cachePricePerM}
                onChange={(e) => setForm((f) => ({ ...f, cachePricePerM: e.target.value }))}
                className={inputCls}
                type="text"
                inputMode="decimal"
                placeholder="0"
              />
            </Field>
            <Field label="输出单价" hint="thinking/reasoning token 专用价（DeepSeek-R1 / OpenAI o 系列）；留 0 = 按正常单价">
              <input
                value={form.reasoningPricePerM}
                onChange={(e) => setForm((f) => ({ ...f, reasoningPricePerM: e.target.value }))}
                className={inputCls}
                type="text"
                inputMode="decimal"
                placeholder="0"
              />
            </Field>
          </div>
        </Accordion>
        <Field label="API Key" hint={initial ? "留空则保持不变" : undefined}>
          <input
            value={form.apiKey}
            onChange={set("apiKey")}
            className={inputCls}
            placeholder={initial ? "••••••••（留空保持不变）" : "sk-... / AIza..."}
            type="password"
          />
        </Field>
        <div className="flex justify-end gap-2 pt-2">
          <Button variant="secondary" onClick={onClose} disabled={submitting}>
            取消
          </Button>
          <Button type="submit" disabled={submitting}>
            {submitting ? "提交中…" : initial ? "保存" : "创建"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

export function Channels() {
  const { channels, loadingChannels, addChannel, updateChannel, removeChannel, toggleChannelStatus, testChannel, refreshChannels } = useAppStore();
  const [modalOpen, setModalOpen] = useState(false);
  const [editing, setEditing] = useState<Channel | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<Channel | null>(null);
  // 测试结果摘要（行内展示）+ 完整信息（悬停提示）
  const [testMsg, setTestMsg] = useState<Record<string, string>>({});
  const [testTitle, setTestTitle] = useState<Record<string, string>>({});
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 筛选
  const [keyword, setKeyword] = useState("");
  const [typeFilter, setTypeFilter] = useState("全部");
  const [statusFilter, setStatusFilter] = useState("全部");

  useEffect(() => {
    refreshChannels();
  }, [refreshChannels]);

  const filtered = useMemo(() => {
    const kw = keyword.trim().toLowerCase();
    return channels.filter((c) => {
      const matchKw =
        !kw ||
        c.name.toLowerCase().includes(kw) ||
        c.baseUrl.toLowerCase().includes(kw) ||
        c.model.toLowerCase().includes(kw);
      const matchType = typeFilter === "全部" || c.type === typeFilter;
      const matchStatus = statusFilter === "全部" || c.status === statusFilter;
      return matchKw && matchType && matchStatus;
    });
  }, [channels, keyword, typeFilter, statusFilter]);

  const openAdd = () => {
    setEditing(null);
    setError(null);
    setModalOpen(true);
  };
  const openEdit = (c: Channel) => {
    setEditing(c);
    setError(null);
    setModalOpen(true);
  };

  const handleSubmit = async (data: FormState) => {
    setSubmitting(true);
    setError(null);
    const price = {
      pricePer1kTokens: per1k(parseFloat(data.pricePerM) || 0),
      cachePricePer1kTokens: per1k(parseFloat(data.cachePricePerM) || 0),
      reasoningPricePer1kTokens: per1k(parseFloat(data.reasoningPricePerM) || 0),
    };
    try {
      if (editing) {
        await updateChannel({
          id: editing.id,
          name: data.name,
          type: data.type,
          protocol: data.protocol,
          baseUrl: data.baseUrl,
          model: data.model,
          status: data.status,
          apiKey: data.apiKey,
          priority: data.priority,
          weight: data.weight,
          ...price,
        });
      } else {
        await addChannel({
          name: data.name,
          type: data.type,
          protocol: data.protocol,
          baseUrl: data.baseUrl,
          model: data.model,
          apiKey: data.apiKey,
          status: data.status,
          priority: data.priority,
          weight: data.weight,
          ...price,
        });
      }
      setModalOpen(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  };

  const handleTest = async (id: string) => {
    setTestMsg((m) => ({ ...m, [id]: "测试中…" }));
    setTestTitle((m) => ({ ...m, [id]: "" }));
    try {
      const r = await testChannel(id);
      if (r.ok) {
        // 成功：只展示结果 + 延迟（完整信息放悬停）
        setTestTitle((m) => ({ ...m, [id]: r.message }));
        setTestMsg((m) => ({ ...m, [id]: `✅ 连接正常 · ${r.latencyMs ?? "-"} ms` }));
      } else {
        // 失败：结果 + 错误码（HTTP 状态码 / 网络 / 解析 / 配置）
        setTestTitle((m) => ({ ...m, [id]: r.message }));
        setTestMsg((m) => ({ ...m, [id]: `❌ ${r.code ?? "测试失败"}` }));
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setTestTitle((m) => ({ ...m, [id]: msg }));
      setTestMsg((m) => ({ ...m, [id]: "❌ 测试失败" }));
    }
  };

  const handleDelete = async () => {
    if (!confirmDelete) return;
    setError(null);
    try {
      await removeChannel(confirmDelete.id);
      setConfirmDelete(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setConfirmDelete(null);
    }
  };

  const handleToggle = async (c: Channel) => {
    setError(null);
    try {
      await toggleChannelStatus(c.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div>
      <PageHeader
        title="渠道"
        description="管理上游 LLM 服务提供方"
        actions={
          <Button onClick={openAdd}>
            <Plus size={16} className="mr-1 inline" /> 添加渠道
          </Button>
        }
      />

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-2.5 text-sm text-red-700 dark:border-red-900 dark:bg-red-950 dark:text-red-300">
          {error}
        </div>
      )}

      <Card className="mb-4">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-4">
          <div className="relative sm:col-span-2">
            <Search size={16} className="absolute left-3 top-1/2 -translate-y-1/2 text-faint" />
            <input
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
              className={`${inputCls} pl-9`}
              placeholder="按名称 / Base URL / 模型搜索"
            />
          </div>
          <select value={typeFilter} onChange={(e) => setTypeFilter(e.target.value)} className={inputCls}>
            <option value="全部">全部类型</option>
            {channelTypes.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
          <select value={statusFilter} onChange={(e) => setStatusFilter(e.target.value)} className={inputCls}>
            <option value="全部">全部状态</option>
            <option value="active">启用</option>
            <option value="disabled">停用</option>
            <option value="error">异常</option>
          </select>
        </div>
        <p className="mt-3 text-xs text-faint">
          共 {filtered.length} 个渠道{channels.length !== filtered.length && `（已筛选，全部 ${channels.length}）`}
        </p>
      </Card>

      <Card className="p-0 overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead className="border-b border-edge bg-inset text-xs text-muted">
            <tr>
              <th className="px-3 py-2 font-medium">名称</th>
              <th className="px-3 py-2 font-medium">类型</th>
              <th className="px-3 py-2 font-medium">模型</th>
              <th className="px-3 py-2 font-medium">优先级</th>
              <th className="px-3 py-2 font-medium">单价</th>
              <th className="px-3 py-2 font-medium">状态</th>
              <th className="px-3 py-2 font-medium">延迟</th>
              <th className="px-3 py-2 text-right font-medium">操作</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-edge">
            {filtered.map((c) => (
              <tr key={c.id} className="hover:bg-hover">
                <td className="px-3 py-2">
                  <p className="font-medium text-main">{c.name}</p>
                  <p className="max-w-56 truncate text-xs text-faint" title={c.baseUrl}>
                    {c.baseUrl}
                  </p>
                </td>
                <td className="px-3 py-2">
                  <ChannelTypeBadge type={c.type} />
                  <p className="mt-0.5 text-[10px] text-faint" title="实际使用的上游协议">
                    {c.protocol ? `${c.protocol}（指定）` : `${autoProtocol(c.type)}（自动）`}
                  </p>
                </td>
                <td className="px-3 py-2 text-muted">
                  {c.model ? c.model : <span className="text-xs text-faint">任意模型</span>}
                </td>
                <td className="px-3 py-2 text-muted">
                  <span className="font-medium text-sub">{c.priority}</span>
                  <span className="text-xs text-faint"> / {c.weight}</span>
                </td>
                <td className="px-3 py-2 text-muted">
                  {c.pricePer1kTokens > 0 ? (
                    <span>
                      ¥{perM(c.pricePer1kTokens)}
                      <span className="text-xs text-faint">/M</span>
                      {c.cachePricePer1kTokens > 0 && (
                        <span className="block text-xs text-emerald-500">缓存 ¥{perM(c.cachePricePer1kTokens)}</span>
                      )}
                      {c.reasoningPricePer1kTokens > 0 && (
                        <span className="block text-xs text-violet-500 dark:text-violet-300">输出 ¥{perM(c.reasoningPricePer1kTokens)}</span>
                      )}
                    </span>
                  ) : (
                    <span className="text-strong">—</span>
                  )}
                </td>
                <td className="px-3 py-2">
                  <button onClick={() => handleToggle(c)} title="点击切换状态">
                    <StatusBadge status={c.status} />
                  </button>
                </td>
                <td className="px-3 py-2 text-muted">
                  {c.latencyMs ? `${c.latencyMs} ms` : "—"}
                  {testMsg[c.id] && (
                    <span className="block max-w-48 truncate text-xs text-accent" title={testTitle[c.id] || undefined}>
                      {testMsg[c.id]}
                    </span>
                  )}
                </td>
                <td className="px-3 py-2">
                  <div className="flex justify-end gap-1">
                    <IconBtn title="测试" onClick={() => handleTest(c.id)}>
                      <Cable size={15} />
                    </IconBtn>
                    <IconBtn title="编辑" onClick={() => openEdit(c)}>
                      <Pencil size={15} />
                    </IconBtn>
                    <IconBtn title="删除" danger onClick={() => setConfirmDelete(c)}>
                      <Trash2 size={15} />
                    </IconBtn>
                  </div>
                </td>
              </tr>
            ))}
            {!loadingChannels && filtered.length === 0 && (
              <tr>
                <td colSpan={8} className="px-4 py-10 text-center text-sm text-faint">
                  {channels.length === 0 ? "暂无渠道，点击右上角添加" : "没有符合筛选条件的渠道"}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </Card>

      <ChannelFormModal
        open={modalOpen}
        channelId={editing?.id}
        error={error}
        initial={editing ? {
          name: editing.name,
          type: editing.type,
          protocol: editing.protocol ?? "",
          baseUrl: editing.baseUrl,
          model: editing.model,
          apiKey: "",
          status: editing.status,
          priority: editing.priority,
          weight: editing.weight,
          pricePerM: editing.pricePer1kTokens > 0 ? String(perM(editing.pricePer1kTokens)) : "",
          cachePricePerM: editing.cachePricePer1kTokens > 0 ? String(perM(editing.cachePricePer1kTokens)) : "",
          reasoningPricePerM: editing.reasoningPricePer1kTokens > 0 ? String(perM(editing.reasoningPricePer1kTokens)) : "",
        } : null}
        onClose={() => setModalOpen(false)}
        onSubmit={handleSubmit}
        submitting={submitting}
      />

      <Modal open={!!confirmDelete} title="删除渠道" onClose={() => setConfirmDelete(null)}>
        <p className="text-sm text-muted">
          确定要删除渠道 <span className="font-semibold text-main">{confirmDelete?.name}</span> 吗？此操作不可撤销。
        </p>
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setConfirmDelete(null)}>
            取消
          </Button>
          <Button variant="danger" onClick={handleDelete}>
            <Trash2 size={15} className="mr-1 inline" /> 删除
          </Button>
        </div>
      </Modal>
    </div>
  );
}

function IconBtn({ children, onClick, title, danger }: { children: React.ReactNode; onClick: () => void; title: string; danger?: boolean }) {
  return (
    <button
      title={title}
      onClick={onClick}
      className={`rounded-md p-1.5 transition-colors ${
        danger ? "text-faint hover:bg-red-50 hover:text-red-600 dark:hover:bg-red-950 dark:hover:text-red-400" : "text-faint hover:bg-accent-soft hover:text-accent"
      }`}
    >
      {children}
    </button>
  );
}
