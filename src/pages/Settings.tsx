import { useState } from "react";
import { PageHeader, Card } from "../components/ui";
import { Button } from "../components/Modal";

type TabKey = "server" | "general" | "ui" | "retry";

const tabs: { key: TabKey; label: string }[] = [
  { key: "server", label: "服务配置" },
  { key: "general", label: "通用设置" },
  { key: "ui", label: "界面设置" },
  { key: "retry", label: "重试策略" },
];

const inputCls =
  "w-full rounded-md border border-gray-300 px-3 py-1.5 text-sm focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500";

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
  const [port, setPort] = useState("8787");
  const [host, setHost] = useState("0.0.0.0");
  const [basePath, setBasePath] = useState("/v1");
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-gray-700">监听服务</h3>
      <div className="divide-y divide-gray-100">
        <Row label="监听地址" desc="网关服务绑定地址">
          <input value={host} onChange={(e) => setHost(e.target.value)} className={inputCls} />
        </Row>
        <Row label="监听端口" desc="网关 HTTP 服务端口">
          <input value={port} onChange={(e) => setPort(e.target.value)} className={inputCls} type="number" />
        </Row>
        <Row label="基础路径" desc="OpenAI 兼容接口前缀">
          <input value={basePath} onChange={(e) => setBasePath(e.target.value)} className={inputCls} />
        </Row>
      </div>
      <div className="mt-4 flex justify-end">
        <Button>保存服务配置</Button>
      </div>
    </Card>
  );
}

function GeneralTab() {
  const [enableLogging, setEnableLogging] = useState(true);
  const [logRetention, setLogRetention] = useState("30");
  const [defaultModel, setDefaultModel] = useState("gpt-4o");
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-gray-700">日志与默认值</h3>
      <div className="divide-y divide-gray-100">
        <Row label="启用请求日志" desc="记录所有经过网关的请求">
          <Toggle checked={enableLogging} onChange={setEnableLogging} />
        </Row>
        <Row label="日志保留天数">
          <input value={logRetention} onChange={(e) => setLogRetention(e.target.value)} className={inputCls} type="number" />
        </Row>
        <Row label="默认模型">
          <input value={defaultModel} onChange={(e) => setDefaultModel(e.target.value)} className={inputCls} />
        </Row>
      </div>
      <div className="mt-4 flex justify-end">
        <Button>保存通用设置</Button>
      </div>
    </Card>
  );
}

function UiTab() {
  const [theme, setTheme] = useState("system");
  const [compact, setCompact] = useState(false);
  const [denseTable, setDenseTable] = useState(true);
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-gray-700">外观</h3>
      <div className="divide-y divide-gray-100">
        <Row label="主题" desc="亮色 / 暗色 / 跟随系统">
          <select value={theme} onChange={(e) => setTheme(e.target.value)} className={inputCls}>
            <option value="light">亮色</option>
            <option value="dark">暗色</option>
            <option value="system">跟随系统</option>
          </select>
        </Row>
        <Row label="紧凑模式" desc="减少间距以显示更多内容">
          <Toggle checked={compact} onChange={setCompact} />
        </Row>
        <Row label="表格密度" desc="日志/渠道列表使用更密的行高">
          <Toggle checked={denseTable} onChange={setDenseTable} />
        </Row>
      </div>
      <div className="mt-4 flex justify-end">
        <Button>保存界面设置</Button>
      </div>
    </Card>
  );
}

function RetryTab() {
  const [enabled, setEnabled] = useState(true);
  const [maxRetries, setMaxRetries] = useState("3");
  const [backoff, setBackoff] = useState("exponential");
  const [statuses, setStatuses] = useState("500,502,503,504,429");
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-gray-700">重试策略</h3>
      <div className="divide-y divide-gray-100">
        <Row label="启用自动重试" desc="请求失败时自动重试">
          <Toggle checked={enabled} onChange={setEnabled} />
        </Row>
        <Row label="最大重试次数">
          <input value={maxRetries} onChange={(e) => setMaxRetries(e.target.value)} className={inputCls} type="number" />
        </Row>
        <Row label="退避策略">
          <select value={backoff} onChange={(e) => setBackoff(e.target.value)} className={inputCls}>
            <option value="fixed">固定间隔</option>
            <option value="exponential">指数退避</option>
            <option value="jitter">指数 + 抖动</option>
          </select>
        </Row>
        <Row label="触发重试的状态码" desc="逗号分隔">
          <input value={statuses} onChange={(e) => setStatuses(e.target.value)} className={inputCls} />
        </Row>
      </div>
      <div className="mt-4 flex justify-end">
        <Button>保存重试策略</Button>
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
              tab === t.key
                ? "border-indigo-600 text-indigo-700"
                : "border-transparent text-gray-500 hover:text-gray-800"
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
