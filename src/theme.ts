// 主题切换：data-theme 属性 + CSS 变量（见 index.css）
// 三档：light / dark / system（跟随系统，监听 prefers-color-scheme 动态切换）

const THEME_KEY = "gateway.theme";

export type ThemePreference = "light" | "dark" | "system";

/** 计算实际主题并写入 <html data-theme>。渲染前同步调用可避免暗色闪烁。 */
export function applyTheme(pref?: ThemePreference | null) {
  const p = pref ?? (localStorage.getItem(THEME_KEY) as ThemePreference | null) ?? "system";
  const dark =
    p === "dark" ||
    (p === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

/** 保存偏好并立即生效（设置页调用） */
export function saveTheme(p: ThemePreference) {
  localStorage.setItem(THEME_KEY, p);
  applyTheme(p);
}

/** 启动时监听系统主题变化：system 模式下动态切换 data-theme */
export function watchTheme() {
  const mq = window.matchMedia("(prefers-color-scheme: dark)");
  const onChange = () => {
    const p = (localStorage.getItem(THEME_KEY) as ThemePreference | null) ?? "system";
    if (p === "system") applyTheme(p);
  };
  mq.addEventListener("change", onChange);
}
