import { useEffect, useRef, type ReactNode } from "react";

export function Modal({
  open,
  title,
  onClose,
  size = "md",
  children,
}: {
  open: boolean;
  title: string;
  onClose: () => void;
  /** md：常规弹窗；lg：宽屏（请求详情等大内容） */
  size?: "md" | "lg";
  children: ReactNode;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);

  // 键盘可访问：Esc 关闭、打开时聚焦弹窗、Tab 焦点困在弹窗内、关闭时恢复原焦点
  useEffect(() => {
    if (!open) return;
    const prev = document.activeElement as HTMLElement | null;
    const dialog = dialogRef.current;
    const focusables = () =>
      dialog
        ? Array.from(
            dialog.querySelectorAll<HTMLElement>(
              'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
            ),
          ).filter((el) => !el.hasAttribute("disabled"))
        : [];
    (dialog ?? focusables()[0])?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
        return;
      }
      if (e.key !== "Tab" || !dialog) return;
      const els = focusables();
      if (els.length === 0) return;
      const first = els[0];
      const last = els[els.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      prev?.focus?.();
    };
  }, [open, onClose]);

  if (!open) return null;
  const width = size === "lg" ? "max-w-3xl" : "max-w-lg";
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4" role="dialog" aria-modal="true" aria-label={title}>
      <div className="absolute inset-0 bg-black/50 backdrop-blur-[2px]" onClick={onClose} />
      <div
        ref={dialogRef}
        tabIndex={-1}
        className={`relative z-10 flex max-h-[90vh] w-full ${width} flex-col rounded-2xl bg-card p-6 shadow-pop focus:outline-none`}
      >
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-lg font-semibold text-main">{title}</h2>
          <button
            onClick={onClose}
            className="rounded-md p-1 text-faint hover:bg-hover hover:text-sub"
            aria-label="关闭"
            title="关闭（Esc）"
          >
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <path d="M18 6 6 18M6 6l12 12" />
            </svg>
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto">{children}</div>
      </div>
    </div>
  );
}

/** 通用按钮 */
export function Button({
  variant = "primary",
  children,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "primary" | "secondary" | "danger" }) {
  const styles = {
    primary: "bg-indigo-600 text-white shadow-sm shadow-indigo-600/25 hover:bg-indigo-500",
    secondary: "bg-inset text-sub hover:bg-hover",
    danger: "bg-red-600 text-white shadow-sm shadow-red-600/20 hover:bg-red-500",
  }[variant];
  // 默认 type=button：表单里的次要按钮（取消 / 获取模型等）不应意外提交；
  // 需要提交的按钮由调用方显式传 type="submit"。
  return (
    <button
      type="button"
      className={`rounded-md px-3 py-1.5 text-sm font-medium transition-colors disabled:opacity-50 ${styles}`}
      {...props}
    >
      {children}
    </button>
  );
}
