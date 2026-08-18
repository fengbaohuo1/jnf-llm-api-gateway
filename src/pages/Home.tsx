import { useQuery } from "@tanstack/react-query";
import { Zap } from "lucide-react";
import { useAppStore } from "../store/useAppStore";

export function Home() {
  const { counter, increment, reset } = useAppStore();

  // Example: react-query + a public API to show the stack works end-to-end.
  const { data, isFetching } = useQuery({
    queryKey: ["joke"],
    queryFn: async () => {
      const res = await fetch("https://api.chucknorris.io/jokes/random");
      if (!res.ok) throw new Error("Request failed");
      return (await res.json()) as { value: string };
    },
  });

  return (
    <section className="max-w-xl space-y-4">
      <div className="flex items-center gap-2">
        <Zap className="h-6 w-6 text-indigo-500" />
        <h2 className="text-xl font-semibold">Home</h2>
      </div>

      <div className="rounded-lg border border-gray-200 bg-white p-4 shadow-sm">
        <p className="mb-2 font-medium">Zustand counter</p>
        <div className="flex items-center gap-3">
          <button
            onClick={increment}
            className="rounded-md bg-indigo-600 px-3 py-1 text-white hover:bg-indigo-700"
          >
            +1
          </button>
          <span className="text-lg font-semibold">{counter}</span>
          <button
            onClick={reset}
            className="rounded-md border border-gray-300 px-3 py-1 hover:bg-gray-100"
          >
            Reset
          </button>
        </div>
      </div>

      <div className="rounded-lg border border-gray-200 bg-white p-4 shadow-sm">
        <p className="mb-2 font-medium">
          React Query demo {isFetching && <span className="text-gray-400">(fetching…)</span>}
        </p>
        <p className="text-gray-700">{data?.value ?? "Loading a joke…"}</p>
      </div>
    </section>
  );
}
