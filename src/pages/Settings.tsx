import { useQuery } from "@tanstack/react-query";
import { Settings as SettingsIcon } from "lucide-react";

export function Settings() {
  // Watch mode: react-query auto-refetches when browsers regain focus/tab.
  useQuery({
    queryKey: ["settings-touch"],
    queryFn: async () => 1,
    refetchInterval: 15_000,
  });

  return (
    <section className="max-w-xl space-y-4">
      <div className="flex items-center gap-2">
        <SettingsIcon className="h-6 w-6 text-indigo-500" />
        <h2 className="text-xl font-semibold">Settings</h2>
      </div>
      <p className="text-gray-700">
        This is a placeholder page. Add your Tauri plugin wiring here (SQL, store, shell, dialog,
        notifications, clipboard, autostart).
      </p>
    </section>
  );
}
