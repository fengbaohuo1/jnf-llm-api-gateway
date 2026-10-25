import { useEffect, useMemo, useRef, useState } from "react";
import {
  Cable,
  ChevronDown,
  Layers,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Trash2,
  Wallet,
  X,
} from "lucide-react";
import { channelTypes, providerPresets, protocolOptions, autoProtocol } from "../data/mock";
import type {
  Channel,
  ChannelBalance,
  ChannelProtocol,
  ChannelStatus,
  ChannelType,
  ModelPrice,
} from "../types";
import { useAppStore } from "../store/useAppStore";
import { PageHeader, Card, Toggle } from "../components/ui";
import { Button, Modal } from "../components/Modal";
import { StatusBadge } from "../components/Badge";
import { VendorIcon, VENDOR_META, channelProtocol, vendorKeyOf } from "../components/VendorIcon";
import { api } from "../api";

type FormState = {
  name: string;
  type: ChannelType;
  /** 显式协议；"" = 按渠道类型自动推导 */
  protocol: ChannelProtocol | "";
  /** DeepSeek 思考模式兼容：转发时给 tool_calls 轮次补空 reasoning_content */
  compatReasoningContent: boolean;
  baseUrl: string;
  /** 支持的模型列表；空数组 = 匹配任意模型 */
  models: string[];
  apiKey: string;
  status: ChannelStatus;
  priority: number;
  weight: number;
  /** 按模型定价（元/M 输入）；DeepSeek 每个模型价格不同，渠道不再有统一默认价 */
  modelPrices: Record<string, ModelPriceForm>;
};

interface ModelPriceForm {
  inputPerM: string;
  cacheHitPerM: string;
  outputPerM: string;
  /** DeepSeek 空闲时段是否按 5 折计入（同步官方价格时自动开启） */
  offPeak: boolean;
}

const emptyForm: FormState = {
  name: "",
  type: "OpenAI",
  protocol: "",
  compatReasoningContent: true,
  baseUrl: "",
  models: [],
  apiKey: "",
  status: "active",
  priority: 0,
  weight: 1,
  modelPrices: {},
};

/** 每 1k 单价（存储）→ 元/M 展示值，去尾零 */
const perM = (per1k: number) => (per1k > 0 ? parseFloat((per1k * 1000).toFixed(6)) : 0);
/** 元/M 输入 → 每 1k 存储值 */
const per1k = (perM: number) => Math.round((perM / 1000) * 1e9) / 1e9;

const inputCls =
  "w-full rounded-lg border border-transparent bg-inset px-3 py-1.5 text-sm text-main transition-colors placeholder:text-faint focus:border-transparent focus:bg-card focus:outline-none focus:ring-2 focus:ring-indigo-500/30";

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
    <div className="overflow-hidden rounded-xl bg-inset/50">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center justify-between px-3 py-2 text-sm font-medium text-sub transition-colors hover:bg-hover"
      >
        {title}
        <ChevronDown size={16} className={`shrink-0 text-faint transition-transform ${open ? "rotate-180" : ""}`} />
      </button>
      {open && <div className="border-t border-edge/40 p-3">{children}</div>}
    </div>
  );
}

// ---------------- 渠道表单弹窗 ----------------

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
  const [availableModels, setAvailableModels] = useState<string[]>([]);
  const [modelInput, setModelInput] = useState("");
  const [modelErr, setModelErr] = useState<string | null>(null);
  const [loadingModels, setLoadingModels] = useState(false);
  const [syncingPrices, setSyncingPrices] = useState(false);
  const [syncNote, setSyncNote] = useState<string | null>(null);

  // Reset form when opening with a fresh target.
  const [lastOpen, setLastOpen] = useState(open);
  if (open !== lastOpen) {
    setLastOpen(open);
    if (open) {
      setForm(initial ?? emptyForm);
      setAvailableModels([]);
      setModelInput("");
      setModelErr(null);
      setLoadingModels(false);
      setSyncNote(null);
      setSyncingPrices(false);
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

  const addModel = (raw: string) => {
    const m = raw.trim();
    if (!m) return;
    setForm((f) =>
      f.models.includes(m)
        ? f
        : {
            ...f,
            models: [...f.models, m],
            modelPrices: f.modelPrices[m]
              ? f.modelPrices
              : { ...f.modelPrices, [m]: { inputPerM: "", cacheHitPerM: "", outputPerM: "", offPeak: false } },
          },
    );
  };
  const removeModel = (m: string) =>
    setForm((f) => {
      const next = { ...f.modelPrices };
      delete next[m];
      return { ...f, models: f.models.filter((x) => x !== m), modelPrices: next };
    });

  const changeType = (t: ChannelType) => {
    setForm((f) => {
      const baseUrl =
        !f.baseUrl.trim() || defaultUrls.includes(f.baseUrl.trim()) ? providerPresets[t].baseUrl : f.baseUrl;
      // 类型变化时协议回到「自动」（避免 Claude/Gemini 类型残留显式协议导致测试失败）
      return { ...f, type: t, protocol: "", baseUrl };
    });
    setAvailableModels([]);
    setModelErr(null);
  };

  const changeProtocol = (p: ChannelProtocol | "") => {
    setForm((f) => ({ ...f, protocol: p }));
    // 协议变化后旧的模型列表可能来自不同端点，清掉避免误导
    setAvailableModels([]);
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
      setAvailableModels(r.models);
    } catch (e) {
      setModelErr(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingModels(false);
    }
  };

  // 生效协议：显式优先，否则按渠道类型推导（与后端一致）
  const effectiveProtocol = form.protocol || autoProtocol(form.type);
  const isDeepSeek = effectiveProtocol === "deepseek";

  /** 同步 DeepSeek 官方模型价格：官方无价格 API，后端抓文档页（失败回退内置目录） */
  const syncDeepseekPrices = async () => {
    if (form.models.length === 0) {
      setSyncNote("请先添加模型，再同步价格");
      return;
    }
    setSyncingPrices(true);
    setSyncNote(null);
    try {
      const r = await api.fetchDeepseekPrices();
      setForm((f) => {
        const next = { ...f.modelPrices };
        for (const m of f.models) {
          const p = r.models[m];
          if (!p) continue;
          next[m] = {
            inputPerM: p.input > 0 ? String(perM(p.input)) : "",
            cacheHitPerM: p.cacheHit > 0 ? String(perM(p.cacheHit)) : "",
            outputPerM: p.output > 0 ? String(perM(p.output)) : "",
            offPeak: !!p.offPeak,
          };
        }
        return { ...f, modelPrices: next };
      });
      setSyncNote(
        r.source === "official-docs"
          ? `已同步官方价格（${r.fetchedAt}）`
          : "官方文档页暂时不可达，已填入内置官方价格目录（随应用更新）",
      );
    } catch (e) {
      setSyncNote(`同步失败：${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSyncingPrices(false);
    }
  };

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

  // 可选模型候选：上游返回 + 厂商内置常用（去重）
  const suggestions = Array.from(new Set([...availableModels, ...preset.models]));

  const pickChip = (picked: boolean) =>
    `rounded-md px-2 py-0.5 font-mono text-[11px] transition-colors ${
      picked ? "bg-accent-soft text-accent shadow-sm" : "bg-inset text-sub hover:bg-hover"
    }`;

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
          <Field label="渠道协议" hint="决定连通性测试 / 知识库问答用的适配器">
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
        {isDeepSeek && (
          <div className="flex items-center justify-between gap-3 rounded-xl bg-inset/60 px-3 py-2.5">
            <div>
              <p className="text-sm font-medium text-sub">DeepSeek 思考模式兼容</p>
              <p className="text-xs text-faint">
                转发时给带 tool_calls 的 assistant 消息补空 reasoning_content——DeepSeek
                思考模型要求工具调用轮次回传该字段，否则工具执行后的下一轮会 400
              </p>
            </div>
            <Toggle
              checked={form.compatReasoningContent}
              onChange={(v) => setForm((f) => ({ ...f, compatReasoningContent: v }))}
            />
          </div>
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
          label="模型（可多选）"
          hint="留空 = 匹配任意模型（通配）；可回车添加、点选建议，或从上游获取模型列表"
        >
          <div className="flex flex-wrap items-center gap-2">
            <input
              value={modelInput}
              onChange={(e) => setModelInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  addModel(modelInput);
                  setModelInput("");
                }
              }}
              className={`${inputCls} min-w-0 flex-1`}
              placeholder="输入模型 ID，回车添加"
            />
            <Button
              variant="secondary"
              className="shrink-0"
              onClick={() => {
                addModel(modelInput);
                setModelInput("");
              }}
            >
              添加
            </Button>
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
          {form.models.length > 0 ? (
            <div className="mt-2 flex flex-wrap gap-1.5">
              {form.models.map((m) => (
                <span
                  key={m}
                  className="inline-flex items-center gap-1 rounded-lg bg-accent-soft px-2 py-0.5 font-mono text-[11px] text-accent"
                >
                  {m}
                  <button
                    type="button"
                    aria-label={`移除 ${m}`}
                    onClick={() => removeModel(m)}
                    className="text-accent/70 transition-colors hover:text-accent"
                  >
                    <X size={11} />
                  </button>
                </span>
              ))}
            </div>
          ) : (
            <p className="mt-1.5 text-[11px] text-faint">当前为通配：任意模型的请求都会命中该渠道</p>
          )}
        </Field>

        {suggestions.length > 0 && (
          <div>
            <p className="mb-1 text-xs text-faint">可选模型（点击加入 / 再点移除）：</p>
            <div className="flex max-h-28 flex-wrap gap-1.5 overflow-y-auto">
              {suggestions.map((m) => (
                <button
                  key={m}
                  type="button"
                  onClick={() => (form.models.includes(m) ? removeModel(m) : addModel(m))}
                  className={pickChip(form.models.includes(m))}
                >
                  {m}
                </button>
              ))}
            </div>
          </div>
        )}
        {isDeepSeek && availableModels.length === 0 && !loadingModels && !modelErr && (
          <p className="text-xs text-faint">
            DeepSeek 模型列表通过上游接口获取：{channelId ? "打开编辑时自动加载" : "填写 API Key 后自动加载"}
            ，也可点「获取模型」。
          </p>
        )}
        {modelErr && (
          <p className="rounded-md bg-red-50 px-3 py-2 text-xs text-red-700 dark:bg-red-950 dark:text-red-300">
            模型列表获取失败：{modelErr}（可手动填写模型 ID）
          </p>
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
            {/* DeepSeek 等厂商每个模型价格不同，渠道不再有统一默认价，按模型逐个填写 */}
            {form.models.length > 0 ? (
              <div className="space-y-2">
                <div className="flex items-start justify-between gap-2">
                  <div>
                    <p className="text-sm font-medium text-sub">按模型定价</p>
                    <p className="text-xs text-faint">
                      每个模型的输入 / 缓存命中 / 输出单价可不同；留空 = 该模型不计费
                      {isDeepSeek ? "。计时按官方高峰/空闲时段自动折算" : ""}
                    </p>
                  </div>
                  {isDeepSeek && (
                    <Button
                      variant="secondary"
                      className="shrink-0"
                      disabled={syncingPrices}
                      onClick={() => void syncDeepseekPrices()}
                      title="DeepSeek 无价格 API：抓取官方文档价格页，失败回退内置目录"
                    >
                      <RefreshCw size={13} className={`mr-1 inline ${syncingPrices ? "animate-spin" : ""}`} />
                      {syncingPrices ? "同步中" : "同步官方价"}
                    </Button>
                  )}
                </div>
                {syncNote && <p className="text-xs text-faint">{syncNote}</p>}
                {form.models.map((m) => {
                  const p = form.modelPrices[m] ?? {
                    inputPerM: "",
                    cacheHitPerM: "",
                    outputPerM: "",
                    offPeak: false,
                  };
                  const update = (patch: Partial<ModelPriceForm>) =>
                    setForm((f) => ({ ...f, modelPrices: { ...f.modelPrices, [m]: { ...p, ...patch } } }));
                  return (
                    <div key={m} className="rounded-lg bg-inset/60 p-2">
                      <div className="mb-1.5 flex items-center justify-between gap-2">
                        <span className="truncate font-mono text-[11px] text-sub" title={m}>
                          {m}
                        </span>
                        <label className="flex shrink-0 items-center gap-1 text-[11px] text-faint">
                          <input
                            type="checkbox"
                            checked={p.offPeak}
                            onChange={(e) => update({ offPeak: e.target.checked })}
                            className="h-3 w-3 rounded"
                          />
                          空闲时段半价
                        </label>
                      </div>
                      <div className="mb-1 grid grid-cols-3 gap-1.5 text-[10px] text-faint">
                        <span>输入</span>
                        <span>缓存命中</span>
                        <span>输出</span>
                      </div>
                      <div className="grid grid-cols-3 gap-1.5">
                        <input
                          className={inputCls}
                          placeholder="输入"
                          type="text"
                          inputMode="decimal"
                          value={p.inputPerM}
                          onChange={(e) => update({ inputPerM: e.target.value })}
                        />
                        <input
                          className={inputCls}
                          placeholder="缓存命中"
                          type="text"
                          inputMode="decimal"
                          value={p.cacheHitPerM}
                          onChange={(e) => update({ cacheHitPerM: e.target.value })}
                        />
                        <input
                          className={inputCls}
                          placeholder="输出"
                          type="text"
                          inputMode="decimal"
                          value={p.outputPerM}
                          onChange={(e) => update({ outputPerM: e.target.value })}
                        />
                      </div>
                    </div>
                  );
                })}
              </div>
            ) : (
              <div className="rounded-lg bg-inset/60 px-3 py-2 text-xs text-faint">
                该渠道未配置模型，无法按模型计价。请先在上方「模型」中添加模型（DeepSeek 可点「获取模型」拉取），
                再用「同步官方价」或手动填写各模型单价。
              </div>
            )}
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

// ---------------- 渠道卡片 ----------------

type BalanceState = { loading?: boolean; data?: ChannelBalance; error?: string };

function BalanceBlock({
  balance,
  onRefresh,
}: {
  balance?: BalanceState;
  onRefresh: () => void;
}) {
  const data = balance?.data;
  const loading = balance?.loading;
  return (
    <div className="mt-3 rounded-xl bg-gradient-to-br from-inset/90 to-inset/40 p-3">
      <div className="flex items-center justify-between">
        <span className="flex items-center gap-1.5 text-[11px] font-medium text-muted">
          <Wallet size={12} /> 账户余额
        </span>
        <button
          type="button"
          onClick={onRefresh}
          title="刷新余额"
          className="rounded-md p-1 text-faint transition-colors hover:bg-card hover:text-accent"
        >
          <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
        </button>
      </div>
      {loading && !data && <p className="mt-1.5 text-xs text-faint">查询中…</p>}
      {balance?.error && (
        <p className="mt-1 text-xs text-red-500" title={balance.error}>
          {balance.error}
          <button type="button" onClick={onRefresh} className="ml-1 underline">
            重试
          </button>
        </p>
      )}
      {data?.supported && (
        <>
          <div className="mt-1 flex items-baseline gap-1.5">
            <span className="text-[22px] font-semibold leading-none tracking-tight text-main">
              ¥{data.totalBalance?.toFixed(2) ?? "0.00"}
            </span>
            {data.available === false && (
              <span className="rounded bg-red-50 px-1 py-0.5 text-[10px] text-red-600 dark:bg-red-950 dark:text-red-300">
                不可用
              </span>
            )}
          </div>
          <div className="mt-1.5 flex items-center gap-3 text-[10px] text-faint">
            <span>充值 ¥{data.toppedUpBalance?.toFixed(2) ?? "0.00"}</span>
            <span>赠送 ¥{data.grantedBalance?.toFixed(2) ?? "0.00"}</span>
            {data.fetchedAt && <span className="ml-auto">{data.fetchedAt.slice(11, 19)}</span>}
          </div>
        </>
      )}
      {data && !data.supported && <p className="mt-1 text-xs text-faint">该厂商暂不支持余额查询</p>}
    </div>
  );
}

function ChannelCard({
  channel,
  balance,
  testMsg,
  testTitle,
  onTest,
  onEdit,
  onDelete,
  onToggle,
  onRefreshBalance,
}: {
  channel: Channel;
  balance?: BalanceState;
  testMsg?: string;
  testTitle?: string;
  onTest: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onToggle: () => void;
  onRefreshBalance: () => void;
}) {
  const protocol = channelProtocol(channel);
  const vendor = vendorKeyOf(channel.type, channel.protocol);
  const meta = VENDOR_META[vendor];
  const isDeepSeek = protocol === "deepseek";
  const ok = testMsg?.startsWith("✅");

  return (
    <Card className="group relative overflow-hidden p-0 transition-all duration-200 hover:-translate-y-0.5 hover:shadow-pop">
      <div
        className="pointer-events-none absolute inset-x-0 top-0 h-28"
        style={{ background: `linear-gradient(135deg, ${meta.from}33, transparent 68%)` }}
      />
      <div className="relative flex h-full flex-col p-4">
        <div className="flex items-start gap-3">
          <VendorIcon type={channel.type} protocol={channel.protocol} size={44} />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-1.5">
              <h3 className="truncate text-[15px] font-semibold text-main">{channel.name}</h3>
              {channel.compatReasoningContent && isDeepSeek && (
                <span
                  title="DeepSeek 思考模式兼容已开启"
                  className="shrink-0 rounded bg-accent-soft px-1 py-0.5 text-[10px] font-medium text-accent"
                >
                  兼容
                </span>
              )}
            </div>
            <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
              <StatusBadge status={channel.status} />
              <span className="rounded-md bg-inset px-1.5 py-0.5 text-[10px] text-muted">{channel.type}</span>
              {channel.protocol && (
                <span
                  title="显式协议覆盖：连通性测试与知识库问答按此协议调用"
                  className="rounded-md bg-accent-soft px-1.5 py-0.5 text-[10px] text-accent"
                >
                  协议 {channel.protocol}
                </span>
              )}
            </div>
          </div>
          <div className="flex items-center gap-0.5">
            <IconBtn title="测试连通性" onClick={onTest}>
              <Cable size={15} />
            </IconBtn>
            <IconBtn title="编辑" onClick={onEdit}>
              <Pencil size={15} />
            </IconBtn>
            <IconBtn title="删除" danger onClick={onDelete}>
              <Trash2 size={15} />
            </IconBtn>
          </div>
        </div>

        {/* 厂商专属区：DeepSeek 显示余额；其他厂商暂不显示 */}
        {isDeepSeek && <BalanceBlock balance={balance} onRefresh={onRefreshBalance} />}

        <div className="mt-3">
          <p className="mb-1 text-[10px] font-medium uppercase tracking-wide text-faint">
            模型 {channel.models.length > 1 && `· ${channel.models.length}`}
          </p>
          {channel.models.length === 0 ? (
            <p className="text-xs text-faint">通配任意模型</p>
          ) : (
            <div className="flex flex-wrap gap-1">
              {channel.models.slice(0, 4).map((m) => (
                <span
                  key={m}
                  className="rounded-md bg-inset px-1.5 py-0.5 font-mono text-[10px] text-sub"
                  title={m}
                >
                  {m}
                </span>
              ))}
              {channel.models.length > 4 && (
                <span className="px-1 py-0.5 text-[10px] text-faint">+{channel.models.length - 4}</span>
              )}
            </div>
          )}
        </div>

        <div className="mt-3 grid grid-cols-2 gap-x-3 gap-y-2 text-xs">
          <div className="min-w-0">
            <p className="text-[10px] text-faint">Base URL</p>
            <p className="truncate text-muted" title={channel.baseUrl}>
              {channel.baseUrl.replace(/^https?:\/\//, "")}
            </p>
          </div>
          <div>
            <p className="text-[10px] text-faint">单价</p>
            <p className="text-muted">
              {Object.keys(channel.modelPrices ?? {}).length > 0 ? (
                <span
                  title={Object.entries(channel.modelPrices)
                    .map(([m, p]) => `${m}: 输入 ¥${perM(p.input)}/M · 输出 ¥${perM(p.output)}/M`)
                    .join("\n")}
                >
                  按模型
                  <span className="text-[10px] text-faint">（{Object.keys(channel.modelPrices).length}）</span>
                </span>
              ) : channel.pricePer1kTokens > 0 ? (
                <>
                  ¥{perM(channel.pricePer1kTokens)}
                  <span className="text-[10px] text-faint">/M</span>
                </>
              ) : (
                "—"
              )}
            </p>
          </div>
          <div>
            <p className="text-[10px] text-faint">优先级 / 权重</p>
            <p className="text-muted">
              {channel.priority} <span className="text-faint">/ {channel.weight}</span>
            </p>
          </div>
          <div>
            <p className="text-[10px] text-faint">延迟</p>
            <p className="text-muted">{channel.latencyMs ? `${channel.latencyMs} ms` : "—"}</p>
          </div>
        </div>

        {testMsg && (
          <p
            className={`mt-3 truncate text-xs ${ok ? "text-emerald-500" : "text-red-500"}`}
            title={testTitle || undefined}
          >
            {testMsg}
          </p>
        )}

        <div className="mt-auto flex items-center justify-between pt-3">
          <span className="text-[11px] text-faint">点击开关启停渠道</span>
          <Toggle checked={channel.status === "active"} onChange={onToggle} />
        </div>
      </div>
    </Card>
  );
}

// ---------------- 页面 ----------------

export function Channels() {
  const { channels, loadingChannels, addChannel, updateChannel, removeChannel, toggleChannelStatus, testChannel, refreshChannels } = useAppStore();
  const [modalOpen, setModalOpen] = useState(false);
  const [editing, setEditing] = useState<Channel | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<Channel | null>(null);
  // 测试结果摘要（卡片内展示）+ 完整信息（悬停提示）
  const [testMsg, setTestMsg] = useState<Record<string, string>>({});
  const [testTitle, setTestTitle] = useState<Record<string, string>>({});
  // 余额（目前仅 DeepSeek）
  const [balances, setBalances] = useState<Record<string, BalanceState>>({});
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 筛选
  const [keyword, setKeyword] = useState("");
  const [typeFilter, setTypeFilter] = useState("全部");
  const [statusFilter, setStatusFilter] = useState("全部");

  useEffect(() => {
    refreshChannels();
  }, [refreshChannels]);

  const balanceRequested = useRef<Set<string>>(new Set());
  const loadBalance = async (id: string) => {
    balanceRequested.current.add(id);
    setBalances((m) => ({ ...m, [id]: { loading: true } }));
    try {
      const data = await api.fetchChannelBalance(id);
      setBalances((m) => ({ ...m, [id]: { data } }));
    } catch (e) {
      setBalances((m) => ({ ...m, [id]: { error: e instanceof Error ? e.message : String(e) } }));
    }
  };

  // 渠道列表就绪后自动拉取 DeepSeek 余额（每个渠道一次；失败可点刷新重试）
  useEffect(() => {
    for (const c of channels) {
      if (channelProtocol(c) !== "deepseek") continue;
      if (balanceRequested.current.has(c.id)) continue;
      void loadBalance(c.id);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [channels]);

  const filtered = useMemo(() => {
    const kw = keyword.trim().toLowerCase();
    return channels.filter((c) => {
      const matchKw =
        !kw ||
        c.name.toLowerCase().includes(kw) ||
        c.baseUrl.toLowerCase().includes(kw) ||
        c.models.some((m) => m.toLowerCase().includes(kw));
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
    // 按模型定价：只保留与当前模型列表匹配、且至少填了一项的条目；勾选空闲半价时按 5 折写入 offPeak
    const modelPrices: Record<string, ModelPrice> = {};
    for (const m of data.models) {
      const p = data.modelPrices[m];
      if (!p) continue;
      const input = per1k(parseFloat(p.inputPerM) || 0);
      const cacheHit = per1k(parseFloat(p.cacheHitPerM) || 0);
      const output = per1k(parseFloat(p.outputPerM) || 0);
      if (input <= 0 && cacheHit <= 0 && output <= 0) continue;
      const entry: ModelPrice = { input, cacheHit, output, reasoning: 0 };
      if (p.offPeak) {
        entry.offPeak = { input: input / 2, cacheHit: cacheHit / 2, output: output / 2, reasoning: 0 };
      }
      modelPrices[m] = entry;
    }
    const firstPriced = data.models
      .map((m) => modelPrices[m])
      .find((p) => p && (p.input > 0 || p.cacheHit > 0 || p.output > 0));
    const price = {
      // 渠道默认单价保留给「未配置模型的通配渠道 / 未知模型」兜底：取第一个已定价模型
      pricePer1kTokens: firstPriced?.input ?? 0,
      cachePricePer1kTokens: firstPriced?.cacheHit ?? 0,
      reasoningPricePer1kTokens: firstPriced?.reasoning ?? 0,
      modelPrices,
    };
    try {
      if (editing) {
        await updateChannel({
          id: editing.id,
          name: data.name,
          type: data.type,
          protocol: data.protocol,
          compatReasoningContent: data.compatReasoningContent,
          baseUrl: data.baseUrl,
          models: data.models,
          status: data.status,
          apiKey: data.apiKey,
          priority: data.priority,
          weight: data.weight,
          ...price,
        });
        // 编辑后（Key/端点可能变化）重新查询余额
        balanceRequested.current.delete(editing.id);
        setBalances((m) => {
          const next = { ...m };
          delete next[editing.id];
          return next;
        });
      } else {
        await addChannel({
          name: data.name,
          type: data.type,
          protocol: data.protocol,
          compatReasoningContent: data.compatReasoningContent,
          baseUrl: data.baseUrl,
          models: data.models,
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
      balanceRequested.current.delete(confirmDelete.id);
      setBalances((m) => {
        const next = { ...m };
        delete next[confirmDelete.id];
        return next;
      });
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
        description="管理上游 LLM 服务提供方：卡片按厂商展示，DeepSeek 可直接查看账户余额"
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

      {loadingChannels && channels.length === 0 ? (
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {[0, 1, 2].map((i) => (
            <div key={i} className="h-64 animate-pulse rounded-2xl bg-card shadow-card" />
          ))}
        </div>
      ) : filtered.length === 0 ? (
        <Card className="flex flex-col items-center justify-center gap-3 py-14 text-center">
          <div className="flex h-12 w-12 items-center justify-center rounded-2xl bg-accent-soft text-accent">
            <Layers size={22} />
          </div>
          <div>
            <p className="text-sm font-medium text-main">
              {channels.length === 0 ? "还没有渠道" : "没有符合筛选条件的渠道"}
            </p>
            <p className="mt-1 text-xs text-faint">
              {channels.length === 0
                ? "添加第一个上游渠道后，可在卡片上测试连通性、查看余额与模型"
                : "试试调整关键词或筛选条件"}
            </p>
          </div>
          {channels.length === 0 && (
            <Button onClick={openAdd}>
              <Plus size={15} className="mr-1 inline" /> 添加渠道
            </Button>
          )}
        </Card>
      ) : (
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {filtered.map((c) => (
            <ChannelCard
              key={c.id}
              channel={c}
              balance={balances[c.id]}
              testMsg={testMsg[c.id]}
              testTitle={testTitle[c.id]}
              onTest={() => handleTest(c.id)}
              onEdit={() => openEdit(c)}
              onDelete={() => setConfirmDelete(c)}
              onToggle={() => handleToggle(c)}
              onRefreshBalance={() => {
                balanceRequested.current.delete(c.id);
                void loadBalance(c.id);
              }}
            />
          ))}
        </div>
      )}

      <ChannelFormModal
        open={modalOpen}
        channelId={editing?.id}
        error={error}
        initial={editing ? {
          name: editing.name,
          type: editing.type,
          protocol: editing.protocol ?? "",
          compatReasoningContent: editing.compatReasoningContent,
          baseUrl: editing.baseUrl,
          models: editing.models,
          apiKey: "",
          status: editing.status,
          priority: editing.priority,
          weight: editing.weight,
          // 旧渠道没有按模型价格时，用渠道原默认单价预填每个模型，保存后即转为按模型计价
          modelPrices: Object.fromEntries(
            editing.models.map((m) => {
              const p = editing.modelPrices?.[m];
              const input = p ? p.input : editing.pricePer1kTokens;
              const cacheHit = p ? p.cacheHit : editing.cachePricePer1kTokens;
              const output = p ? p.output : editing.pricePer1kTokens;
              return [
                m,
                {
                  inputPerM: input > 0 ? String(perM(input)) : "",
                  cacheHitPerM: cacheHit > 0 ? String(perM(cacheHit)) : "",
                  outputPerM: output > 0 ? String(perM(output)) : "",
                  offPeak: !!p?.offPeak,
                },
              ];
            }),
          ),
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
