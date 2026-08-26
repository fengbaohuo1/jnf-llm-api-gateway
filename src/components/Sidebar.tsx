import { NavLink } from "react-router-dom";
import {
  LayoutDashboard,
  Gauge,
  Layers,
  KeyRound,
  ScrollText,
  Settings,
} from "lucide-react";

const navItems = [
  { to: "/", label: "仪表盘", icon: LayoutDashboard },
  { to: "/usage", label: "用量", icon: Gauge },
  { to: "/channels", label: "渠道", icon: Layers },
  { to: "/keys", label: "密钥", icon: KeyRound },
  { to: "/logs", label: "日志", icon: ScrollText },
  { to: "/settings", label: "设置", icon: Settings },
];

export function Sidebar() {
  return (
    <aside className="flex w-52 flex-shrink-0 flex-col border-r border-gray-200 bg-white">
      <div className="flex items-center gap-2 px-4 py-4">
        <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-indigo-600">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="white" strokeWidth="2.2">
            <path d="M12 2 2 7l10 5 10-5-10-5Z" />
            <path d="m2 17 10 5 10-5M2 12l10 5 10-5" />
          </svg>
        </div>
        <div>
          <p className="text-sm font-bold text-gray-900">LLM 网关</p>
          <p className="text-xs text-gray-400">API Gateway</p>
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
                    ? "bg-indigo-50 text-indigo-700"
                    : "text-gray-600 hover:bg-gray-100 hover:text-gray-900"
                }`
              }
            >
              <Icon size={17} className="flex-shrink-0" />
              {item.label}
            </NavLink>
          );
        })}
      </nav>

      <div className="border-t border-gray-200 px-4 py-3">
        <p className="text-xs text-gray-400">v0.1.0 本地开发版</p>
      </div>
    </aside>
  );
}
