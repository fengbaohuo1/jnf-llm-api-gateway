import { useCallback, useEffect, useState } from "react";
import { CheckCircle2, Copy } from "lucide-react";
import { PageHeader, Card, Toggle } from "../components/ui";
import { Button } from "../components/Modal";
import { api } from "../api";
import { saveTheme, type ThemePreference } from "../theme";
import { isEnabled as autoStartEnabled, enable as autoStartEnable, disable as autoStartDisable } from "@tauri-apps/plugin-autostart";
import { openPath } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { ServiceInfo, ServiceStatus } from "../types";

type TabKey = "server" | "proxy" | "general" | "system" | "ui" | "failover" | "services";

const tabs: { key: TabKey; label: string }[] = [
  { key: "server", label: "服务配置" },
  { key: "proxy", label: "代理与安全" },
  { key: "general", label: "通用设置" },
  { key: "system", label: "系统" },
  { key: "ui", label: "界面设置" },
  { key: "failover", label: "故障转移" },
  { key: "services", label: "扩展服务" },
];

const inputCls =
  "w-full rounded-md border border-strong bg-card px-3 py-1.5 text-sm text-main focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500";

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
        <p className="text-sm font-medium text-sub">{label}</p>
        {desc && <p className="text-xs text-faint">{desc}</p>}
      </div>
      <div className="sm:w-72">{children}</div>
    </div>
  );
}

function ServerTab() {
  const host = useSetting("server.host", "127.0.0.1");
  const port = useSetting("server.port", "8787");
  const [saved, setSaved] = useState(false);
  const saveAll = async () => {
    await Promise.all([host.save(), port.save()]);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-sub">监听服务</h3>
      <p className="mb-2 text-xs text-faint">修改监听地址或端口后需重启应用生效</p>
      <div className="divide-y divide-edge">
        <Row label="监听地址" desc="网关服务绑定地址（默认仅本机，对外开放请自行评估风险）">
          <input value={host.value ?? ""} onChange={(e) => host.setValue(e.target.value)} className={inputCls} />
        </Row>
        <Row label="监听端口" desc="网关 HTTP 服务端口（接口固定挂在 /v1 下）">
          <input value={port.value ?? ""} onChange={(e) => port.setValue(e.target.value)} className={inputCls} type="number" />
        </Row>
      </div>
      <div className="mt-4 flex items-center justify-end gap-3">
        {saved && <span className="text-xs text-green-600">已保存</span>}
        <Button onClick={saveAll}>保存服务配置</Button>
      </div>
    </Card>
  );
}

function ProxyTab() {
  const enabled = useSetting("server.enabled", "true");
  const securityEnabled = useSetting("security.enabled", "true");
  const securityMode = useSetting("security.mode", "replace");
  const port = useSetting("server.port", "8787");
  const [saved, setSaved] = useState(false);
  const saveAll = async () => {
    await Promise.all([enabled.save(), securityEnabled.save(), securityMode.save()]);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };
  const endpoint = `http://127.0.0.1:${port.value ?? 8787}/v1/chat/completions`;
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-sub">转发代理</h3>
      <div className="divide-y divide-edge">
        <Row label="启用代理" desc="关闭后代理返回 503，不影响应用其他功能">
          <Toggle checked={enabled.value === "true"} onChange={(v) => enabled.setValue(String(v))} />
        </Row>
        <Row label="服务地址" desc="本地程序接入时配置 Base URL 为以下地址，并使用 sk-llmapi-* 密钥">
          <code className="block rounded-md bg-inset px-3 py-1.5 text-xs text-sub">{endpoint}</code>
        </Row>
      </div>
      <h3 className="mb-2 mt-6 text-sm font-semibold text-sub">安全扫描与脱敏</h3>
      <div className="divide-y divide-edge">
        <Row
          label="启用安全扫描"
          desc="扫描请求与响应中的敏感信息（密钥/密码/手机号/证件号）与提示注入特征"
        >
          <Toggle checked={securityEnabled.value === "true"} onChange={(v) => securityEnabled.setValue(String(v))} />
        </Row>
        <Row
          label="处理模式"
          desc="替换：敏感片段脱敏后照常转发；拦截：检测到高风险内容直接拒绝请求/丢弃响应"
        >
          <select value={securityMode.value ?? "replace"} onChange={(e) => securityMode.setValue(e.target.value)} className={inputCls}>
            <option value="replace">替换（脱敏后转发）</option>
            <option value="block">拦截（高风险直接拒绝）</option>
          </select>
        </Row>
      </div>
      <div className="mt-4 flex items-center justify-end gap-3">
        {saved && <span className="text-xs text-green-600">已保存</span>}
        <Button onClick={saveAll}>保存代理与安全设置</Button>
      </div>
    </Card>
  );
}

function GeneralTab() {
  // 日志内容存储级别：按请求命中的最高安全风险决定是否保存完整内容
  const storeLevel = useSetting("log.store_level", "all");
  const logRetention = useSetting("general.log_retention", "0");
  const [saved, setSaved] = useState(false);
  const saveAll = async () => {
    await Promise.all([storeLevel.save(), logRetention.save()]);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-sub">日志与保留</h3>
      <div className="divide-y divide-edge">
        <Row
          label="日志内容存储级别"
          desc="请求/响应内容按该请求命中的最高安全风险决定是否落库；概要、Token、计费与安全事件始终记录，不受影响"
        >
          <select
            value={storeLevel.value ?? "all"}
            onChange={(e) => storeLevel.setValue(e.target.value)}
            className={inputCls}
          >
            <option value="all">全存（所有请求）</option>
            <option value="low">低风险及以上（需命中规则）</option>
            <option value="medium">中风险及以上</option>
            <option value="high">高风险及以上</option>
            <option value="none">不存内容（仅概要）</option>
          </select>
        </Row>
        <Row
          label="日志保留天数"
          desc="请求日志、内容明细与关联安全事件超过 N 天后自动清理（0 = 永久保留）"
        >
          <input
            value={logRetention.value ?? ""}
            onChange={(e) => logRetention.setValue(e.target.value)}
            className={inputCls}
            type="number"
          />
        </Row>
      </div>
      <p className="mt-3 rounded-lg bg-inset px-3 py-2 text-xs text-faint">
        提示：选择「低风险及以上」后，未命中任何规则的普通请求只保留概要（不再保存请求/响应正文），
        既能大幅缩减日志体积，也能降低敏感内容留存风险；需要排查线上问题时临时切回「全存」即可（即时生效）。
        风险等级由安全扫描器判定——若关闭了「代理与安全」里的安全扫描，将无法判定风险，选阈值级别时不会保存任何内容。
      </p>
      <div className="mt-4 flex items-center justify-end gap-3">
        {saved && <span className="text-xs text-green-600">已保存</span>}
        <Button onClick={saveAll}>保存通用设置</Button>
      </div>
    </Card>
  );
}

function SystemTab() {
  const closeToTray = useSetting("general.close_to_tray", "true");
  const [autoStart, setAutoStart] = useState(false);
  const [autoStartLoaded, setAutoStartLoaded] = useState(false);
  const [fileInfo, setFileInfo] = useState<{ path: string; exists: boolean } | null>(null);
  const [reloadMsg, setReloadMsg] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    autoStartEnabled().then(setAutoStart).catch(() => {}).finally(() => setAutoStartLoaded(true));
  }, []);
  useEffect(() => {
    api.settingsFileInfo().then(setFileInfo).catch(() => {});
  }, []);

  const toggleAutoStart = async (v: boolean) => {
    try {
      // 自启时附加 --hidden（Rust 端 init 时配置）：启动后直接隐藏到托盘
      if (v) {
        await autoStartEnable();
      } else {
        await autoStartDisable();
      }
      setAutoStart(v);
    } catch (e) {
      console.error("切换开机自启失败", e);
    }
  };

  const saveCloseToTray = async () => {
    await closeToTray.save();
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };

  const openConfigFile = async () => {
    if (!fileInfo?.exists) return;
    try {
      // 直接用系统默认程序打开 settings.json（记事本 / 编辑器）
      await openPath(fileInfo.path);
    } catch (e) {
      setReloadMsg(`打开失败：${e}`);
      setTimeout(() => setReloadMsg(null), 3000);
    }
  };

  const reloadFromFile = async () => {
    try {
      const n = await api.reloadSettingsFile();
      setReloadMsg(`已从配置文件导入 ${n} 项设置`);
      // 刷新本页展示
      api.settingsFileInfo().then(setFileInfo).catch(() => {});
    } catch (e) {
      setReloadMsg(`导入失败：${e}`);
    }
    setTimeout(() => setReloadMsg(null), 3000);
  };

  return (
    <div className="space-y-4">
      <Card>
        <h3 className="mb-2 text-sm font-semibold text-sub">启动与窗口</h3>
        <div className="divide-y divide-edge">
          <Row label="开机自启" desc="登录 Windows 后自动运行（自启时隐藏到托盘，代理照常提供服务）">
            <Toggle checked={autoStart} onChange={toggleAutoStart} disabled={!autoStartLoaded} />
          </Row>
          <Row label="关闭到托盘" desc="点击窗口关闭按钮时隐藏到托盘，进程继续运行；彻底退出请用托盘菜单">
            <Toggle checked={closeToTray.value === "true"} onChange={(v) => closeToTray.setValue(String(v))} />
          </Row>
        </div>
        <div className="mt-4 flex items-center justify-end gap-3">
          {saved && <span className="text-xs text-green-600">已保存</span>}
          <Button onClick={saveCloseToTray}>保存窗口设置</Button>
        </div>
      </Card>

      <Card>
        <h3 className="mb-2 text-sm font-semibold text-sub">配置文件（settings.json）</h3>
        <p className="mb-2 text-xs text-faint">
          所有设置以 JSON 键值形式存放在配置文件中（与 Tauri Store 同格式，可手改）。
          应用启动时自动导入；在页面中修改的设置会同步写回文件。
        </p>
        <code className="block rounded-md bg-inset px-3 py-2 text-xs text-sub">{fileInfo?.path ?? "加载中…"}</code>
        <div className="mt-3 flex items-center gap-3">
          <Button variant="secondary" onClick={openConfigFile} disabled={!fileInfo?.exists}>打开配置文件</Button>
          <Button variant="secondary" onClick={reloadFromFile} disabled={!fileInfo?.exists}>从文件重新加载</Button>
          {reloadMsg && <span className="text-xs text-green-600">{reloadMsg}</span>}
        </div>
      </Card>
    </div>
  );
}

function UiTab() {
  const theme = useSetting("ui.theme", "system");
  const [saved, setSaved] = useState(false);
  const saveAll = async () => {
    await theme.save();
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-sub">外观</h3>
      <div className="divide-y divide-edge">
        <Row label="主题" desc="亮色 / 暗色 / 跟随系统（跟随系统时自动响应系统主题变化）">
          <select
            value={theme.value ?? "system"}
            onChange={(e) => {
              theme.setValue(e.target.value);
              // 即时预览：选择后立即切换，保存按钮负责落库
              saveTheme(e.target.value as ThemePreference);
            }}
            className={inputCls}
          >
            <option value="light">亮色</option>
            <option value="dark">暗色</option>
            <option value="system">跟随系统</option>
          </select>
        </Row>
      </div>
      <div className="mt-4 flex items-center justify-end gap-3">
        {saved && <span className="text-xs text-green-600">已保存</span>}
        <Button onClick={saveAll}>保存界面设置</Button>
      </div>
    </Card>
  );
}

function FailoverTab() {
  const enabled = useSetting("failover.enabled", "false");
  const maxAttempts = useSetting("failover.max_attempts", "3");
  const [saved, setSaved] = useState(false);
  const saveAll = async () => {
    await Promise.all([enabled.save(), maxAttempts.save()]);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };
  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-sub">多渠道容灾</h3>
      <div className="divide-y divide-edge">
        <Row
          label="启用故障转移"
          desc="正常请求固定走主渠道（优先级最高）；主渠道失败时按队列顺序尝试备用渠道。关闭则失败即报错"
        >
          <Toggle checked={enabled.value === "true"} onChange={(v) => enabled.setValue(String(v))} />
        </Row>
        <Row label="最大尝试渠道数" desc="包括主渠道在内，最多尝试几个渠道（1 = 不转移）">
          <input
            value={maxAttempts.value ?? ""}
            onChange={(e) => maxAttempts.setValue(e.target.value)}
            className={inputCls}
            type="number"
            min={1}
          />
        </Row>
      </div>
      <div className="mt-4 flex items-center justify-end gap-3">
        {saved && <span className="text-xs text-green-600">已保存</span>}
        <Button onClick={saveAll}>保存故障转移设置</Button>
      </div>
    </Card>
  );
}

// ---- 扩展服务 ----

const serviceBadge: Record<ServiceStatus, string> = {
  running:
    "inline-flex items-center gap-1.5 rounded-md bg-green-50 px-2 py-0.5 text-xs font-medium text-green-700 ring-1 ring-green-200 dark:bg-green-950 dark:text-green-300 dark:ring-green-900",
  failed:
    "inline-flex items-center gap-1.5 rounded-md bg-red-50 px-2 py-0.5 text-xs font-medium text-red-700 ring-1 ring-red-200 dark:bg-red-950 dark:text-red-300 dark:ring-red-900",
  disabled:
    "inline-flex items-center gap-1.5 rounded-md bg-gray-50 px-2 py-0.5 text-xs font-medium text-gray-500 ring-1 ring-gray-200 dark:bg-gray-800 dark:text-gray-400 dark:ring-gray-700",
};
const serviceBadgeLabel: Record<ServiceStatus, string> = {
  running: "运行中",
  failed: "初始化失败",
  disabled: "已停用",
};
const serviceDot: Record<ServiceStatus, string> = {
  running: "bg-green-500",
  failed: "bg-red-500",
  disabled: "bg-gray-400",
};

function ServicesTab() {
  const [services, setServices] = useState<ServiceInfo[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [opError, setOpError] = useState<string | null>(null);
  // MCP 接入端点：读 services.mcp.port（缺省 8790）
  const [mcpPort, setMcpPort] = useState(8790);
  const [copied, setCopied] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setServices(await api.listServices());
      setOpError(null);
    } catch (e) {
      setOpError(String(e));
    } finally {
      setLoaded(true);
    }
  }, []);
  useEffect(() => {
    void refresh();
    api
      .getSetting("services.mcp.port")
      .then((v) => {
        const p = Number(v);
        if (Number.isFinite(p) && p > 0) setMcpPort(p);
      })
      .catch(() => {});
  }, [refresh]);

  const copyEndpoint = async () => {
    try {
      await writeText(`http://127.0.0.1:${mcpPort}/mcp`);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      setOpError("复制失败：请手动复制地址");
    }
  };

  const run = async (s: ServiceInfo, act: () => Promise<unknown>) => {
    setBusyId(s.id);
    setOpError(null);
    try {
      await act();
      await refresh();
    } catch (e) {
      setOpError(String(e));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <Card>
      <h3 className="mb-2 text-sm font-semibold text-sub">扩展服务</h3>
      <p className="mb-2 text-xs text-faint">
        知识库、MCP 等扩展以独立服务注册（services 目录 + 注册表）。开关即时热生效并写入设置（重启应用后保持）；
        初始化失败可点重试恢复，无需重启应用。MCP 服务运行中时，可在 Claude Desktop / Cline 等客户端把下面地址配为
        MCP Server URL。
      </p>
      <div className="divide-y divide-edge">
        {services.map((s) => (
          <div key={s.id} className="flex items-center justify-between gap-4 py-3">
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <p className="text-sm font-medium text-sub">{s.name}</p>
                <span className={serviceBadge[s.status]}>
                  <span className={`h-1.5 w-1.5 rounded-full ${serviceDot[s.status]}`} />
                  {serviceBadgeLabel[s.status]}
                </span>
              </div>
              <p className="mt-0.5 text-xs text-faint">{s.description}</p>
              {s.id === "mcp" && s.status === "running" && (
                <div className="mt-1.5 flex flex-wrap items-center gap-2">
                  <code className="rounded-md bg-inset px-2 py-1 font-mono text-xs text-sub ring-1 ring-edge">
                    http://127.0.0.1:{mcpPort}/mcp
                  </code>
                  <button
                    onClick={() => void copyEndpoint()}
                    className="flex items-center gap-1 rounded-md px-1.5 py-1 text-xs text-faint transition-colors hover:bg-hover hover:text-sub"
                    title="复制 MCP 接入地址"
                  >
                    {copied ? <CheckCircle2 size={13} className="text-green-600" /> : <Copy size={13} />}
                    {copied ? "已复制" : "复制"}
                  </button>
                  <span className="text-[11px] text-faint">
                    监听地址、端口、令牌、只读模式与逐工具开关都在「MCP 服务」页配置
                  </span>
                </div>
              )}
              {s.status === "failed" && s.error && (
                <p className="mt-1 truncate text-xs text-red-600 dark:text-red-400" title={s.error}>
                  失败原因：{s.error}
                </p>
              )}
            </div>
            <div className="flex shrink-0 items-center gap-3">
              {s.status === "failed" && (
                <Button
                  variant="secondary"
                  disabled={busyId === s.id}
                  onClick={() => void run(s, () => api.restartService(s.id))}
                >
                  重试
                </Button>
              )}
              <Toggle
                checked={s.enabled}
                disabled={busyId === s.id}
                onChange={(v) => void run(s, () => api.setServiceEnabled(s.id, v))}
              />
            </div>
          </div>
        ))}
        {!loaded && <p className="py-3 text-xs text-faint">加载中…</p>}
        {loaded && services.length === 0 && <p className="py-3 text-xs text-faint">暂无扩展服务</p>}
      </div>
      {opError && (
        <div className="mt-3 rounded-lg border border-red-200 bg-red-50 px-4 py-2.5 text-sm text-red-700 dark:border-red-900 dark:bg-red-950 dark:text-red-300">
          {opError}
        </div>
      )}
    </Card>
  );
}

export function Settings() {
  const [tab, setTab] = useState<TabKey>("server");
  return (
    <div>
      <PageHeader title="设置" description="配置网关运行参数" />
      <div className="mb-4 flex flex-wrap gap-1 border-b border-edge">
        {tabs.map((t) => (
          <button
            key={t.key}
            onClick={() => setTab(t.key)}
            className={`-mb-px border-b-2 px-4 py-2 text-sm font-medium transition-colors ${
              tab === t.key ? "border-indigo-600 text-accent" : "border-transparent text-muted hover:text-sub"
            }`}
          >
            {t.label}
          </button>
        ))}
      </div>
      {/* 各 Tab 常驻挂载（仅隐藏），避免切换 Tab 时 useSetting 重新读库而丢弃未保存的编辑 */}
      <div className={tab === "server" ? "" : "hidden"}><ServerTab /></div>
      <div className={tab === "proxy" ? "" : "hidden"}><ProxyTab /></div>
      <div className={tab === "general" ? "" : "hidden"}><GeneralTab /></div>
      <div className={tab === "system" ? "" : "hidden"}><SystemTab /></div>
      <div className={tab === "ui" ? "" : "hidden"}><UiTab /></div>
      <div className={tab === "failover" ? "" : "hidden"}><FailoverTab /></div>
      <div className={tab === "services" ? "" : "hidden"}><ServicesTab /></div>
    </div>
  );
}
