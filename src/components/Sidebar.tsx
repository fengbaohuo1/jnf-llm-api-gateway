import { NavLink } from "react-router-dom";
import {
  LayoutDashboard,
  Gauge,
  Layers,
  KeyRound,
  ScrollText,
  Shield,
  Settings,
  Database,
  Plug,
} from "lucide-react";

const navItems = [
  { to: "/", label: "仪表盘", icon: LayoutDashboard },
  { to: "/usage", label: "用量", icon: Gauge },
  { to: "/channels", label: "渠道", icon: Layers },
  { to: "/keys", label: "密钥", icon: KeyRound },
  { to: "/logs", label: "日志", icon: ScrollText },
  { to: "/security", label: "安全", icon: Shield },
  { to: "/kb", label: "知识库", icon: Database },
  { to: "/mcp", label: "MCP 服务", icon: Plug },
  { to: "/settings", label: "设置", icon: Settings },
];

export function Sidebar() {
  return (
    <aside className="flex w-52 flex-shrink-0 flex-col bg-card"> 
      <div className="flex items-center gap-2 px-4 py-4">
        <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-indigo-600 shadow-sm shadow-indigo-600/30">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="white" strokeWidth="1.7" strokeLinecap="round">
            <path d="M4.5 8 10 10.5M4.5 16 10 13.5M14 10.5 19.5 8M14 13.5 19.5 16" />
            <circle cx="12" cy="12" r="3.2" fill="white" stroke="none" />
          </svg>
        </div>
        <div>
          <p className="text-sm font-bold text-main">LLM 网关</p>
          <p className="text-xs text-faint">API Gateway</p>
        </div>
      </div>

      <nav className="flex-1 space-y-0.5 px-2 py-2">
        {navItems.map((item) => {
          const Icon = item.icon;
          return (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.to === "/"}
              className={({ isActive }) =>
                `flex items-center gap-3 rounded-lg px-3 py-2 text-sm font-medium transition-colors ${
                  isActive
                    ? "bg-accent-soft text-accent shadow-sm"
                    : "text-muted hover:bg-hover hover:text-main"
                }`
              }
            >
              <Icon size={17} className="flex-shrink-0" />
              {item.label}
            </NavLink>
          );
        })}
      </nav>

      <div className="px-4 py-3">
        <p className="text-xs text-faint">v1.0.0</p>
      </div>
    </aside>
  );
}
