import { useEffect, useState } from "react";
import { PageHeader, Card } from "../components/ui";
import { Button } from "../components/Modal";
import { api } from "../api";

type TabKey = "server" | "general" | "ui" | "retry";

const tabs: { key: TabKey; label: string }[] = [
  { key: "server", label: "服务配置" },
  { key: "general", label: "通用设置" },
  { key: "ui", label: "界面设置" },
  { key: "retry", label: "重试策略" },
];

const inputCls =
  "w-full rounded-md border border-gray-300 px-3 py-1.5 text-sm focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500";

/** 绑定一个 settings KV 字段，自动读取回填并保存。 */
function useSetting(key: string, fallback: string) {
  const [value, setValue] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    api
      .getSetting(key)
      .then((v) => setValue(v ?? fallback))
      .catch(() => setValue(fallback))
      .finally(() => setLoaded(true));
  }, [key]);
  const save = () => api.setSetting(key, value ?? fallback);
  return { value, setValue, save, loaded };
}

function Row({ label, desc, children }: { label: string; desc?: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col justify-between gap-2 py-3 sm:flex-row sm:items-center">
      <div>
        <p className="text-sm font-medium text-gray-800">{label}</p>
        {desc && <p className="text-xs text-gray-400">{desc}</p>}
      </div>
      <div className="sm:w-72">{children}</div>
    </div>
  );
}

function Toggle({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      type="button"
      onClick={() => onChange(!checked)}
      className={`relative h-6 w-11 rounded-full transition-colors ${checked ? "bg-indigo-600" : "bg-gray-300"}`}
    >
      <span
        className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-all ${checked ? "left-[22px]" : "left-0.5"}`}
      />
    </button>
  );
}

function ServerTab() {
  const host = useSetting("server.host", "0.0.0.0");
  const port = useSetting("server.port", "8787");
  const basePath = useSetting("server.base_path", "/v1");
  const [saved, setSaved] = useState(false);
  const saveAll = async () => {
    await Promise.all([host.save(), port.save(), basePath.save()]);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-gray-700">监听服务</h3>
      <div className="divide-y divide-gray-100">
        <Row label="监听地址" desc="网关服务绑定地址">
          <input value={host.value ?? ""} onChange={(e) => host.setValue(e.target.value)} className={inputCls} />
        </Row>
        <Row label="监听端口" desc="网关 HTTP 服务端口">
          <input value={port.value ?? ""} onChange={(e) => port.setValue(e.target.value)} className={inputCls} type="number" />
        </Row>
        <Row label="基础路径" desc="OpenAI 兼容接口前缀">
          <input value={basePath.value ?? ""} onChange={(e) => basePath.setValue(e.target.value)} className={inputCls} />
        </Row>
      </div>
      <div className="mt-4 flex items-center justify-end gap-3">
        {saved && <span className="text-xs text-green-600">已保存</span>}
        <Button onClick={saveAll}>保存服务配置</Button>
      </div>
    </Card>
  );
}

function GeneralTab() {
  const enableLogging = useSetting("general.enable_logging", "true");
  const logRetention = useSetting("general.log_retention", "30");
  const defaultModel = useSetting("general.default_model", "gpt-4o");
  const [saved, setSaved] = useState(false);
  const saveAll = async () => {
    await Promise.all([enableLogging.save(), logRetention.save(), defaultModel.save()]);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-gray-700">日志与默认值</h3>
      <div className="divide-y divide-gray-100">
        <Row label="启用请求日志" desc="记录所有经过网关的请求">
          <Toggle checked={enableLogging.value === "true"} onChange={(v) => enableLogging.setValue(String(v))} />
        </Row>
        <Row label="日志保留天数">
          <input value={logRetention.value ?? ""} onChange={(e) => logRetention.setValue(e.target.value)} className={inputCls} type="number" />
        </Row>
        <Row label="默认模型">
          <input value={defaultModel.value ?? ""} onChange={(e) => defaultModel.setValue(e.target.value)} className={inputCls} />
        </Row>
      </div>
      <div className="mt-4 flex items-center justify-end gap-3">
        {saved && <span className="text-xs text-green-600">已保存</span>}
        <Button onClick={saveAll}>保存通用设置</Button>
      </div>
    </Card>
  );
}

function UiTab() {
  const theme = useSetting("ui.theme", "system");
  const compact = useSetting("ui.compact", "false");
  const denseTable = useSetting("ui.dense_table", "true");
  const [saved, setSaved] = useState(false);
  const saveAll = async () => {
    await Promise.all([theme.save(), compact.save(), denseTable.save()]);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-gray-700">外观</h3>
      <div className="divide-y divide-gray-100">
        <Row label="主题" desc="亮色 / 暗色 / 跟随系统">
          <select value={theme.value ?? "system"} onChange={(e) => theme.setValue(e.target.value)} className={inputCls}>
            <option value="light">亮色</option>
            <option value="dark">暗色</option>
            <option value="system">跟随系统</option>
          </select>
        </Row>
        <Row label="紧凑模式" desc="减少间距以显示更多内容">
          <Toggle checked={compact.value === "true"} onChange={(v) => compact.setValue(String(v))} />
        </Row>
        <Row label="表格密度" desc="日志/渠道列表使用更密的行高">
          <Toggle checked={denseTable.value === "true"} onChange={(v) => denseTable.setValue(String(v))} />
        </Row>
      </div>
      <div className="mt-4 flex items-center justify-end gap-3">
        {saved && <span className="text-xs text-green-600">已保存</span>}
        <Button onClick={saveAll}>保存界面设置</Button>
      </div>
    </Card>
  );
}

function RetryTab() {
  const enabled = useSetting("retry.enabled", "true");
  const maxRetries = useSetting("retry.max_retries", "3");
  const backoff = useSetting("retry.backoff", "exponential");
  const statuses = useSetting("retry.statuses", "500,502,503,504,429");
  const [saved, setSaved] = useState(false);
  const saveAll = async () => {
    await Promise.all([enabled.save(), maxRetries.save(), backoff.save(), statuses.save()]);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-gray-700">重试策略</h3>
      <div className="divide-y divide-gray-100">
        <Row label="启用自动重试" desc="请求失败时自动重试">
          <Toggle checked={enabled.value === "true"} onChange={(v) => enabled.setValue(String(v))} />
        </Row>
        <Row label="最大重试次数">
          <input value={maxRetries.value ?? ""} onChange={(e) => maxRetries.setValue(e.target.value)} className={inputCls} type="number" />
        </Row>
        <Row label="退避策略">
          <select value={backoff.value ?? "exponential"} onChange={(e) => backoff.setValue(e.target.value)} className={inputCls}>
            <option value="fixed">固定间隔</option>
            <option value="exponential">指数退避</option>
            <option value="jitter">指数 + 抖动</option>
          </select>
        </Row>
        <Row label="触发重试的状态码" desc="逗号分隔">
          <input value={statuses.value ?? ""} onChange={(e) => statuses.setValue(e.target.value)} className={inputCls} />
        </Row>
      </div>
      <div className="mt-4 flex items-center justify-end gap-3">
        {saved && <span className="text-xs text-green-600">已保存</span>}
        <Button onClick={saveAll}>保存重试策略</Button>
      </div>
    </Card>
  );
}

export function Settings() {
  const [tab, setTab] = useState<TabKey>("server");
  return (
    <div>
      <PageHeader title="设置" description="配置网关运行参数" />
      <div className="mb-4 flex gap-1 border-b border-gray-200">
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
      {tab === "server" && <ServerTab />}
      {tab === "general" && <GeneralTab />}
      {tab === "ui" && <UiTab />}
      {tab === "retry" && <RetryTab />}
    </div>
  );
}
