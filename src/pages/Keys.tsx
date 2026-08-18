import { useEffect, useState } from "react";
import { Plus, Trash2, Copy, KeyRound } from "lucide-react";
import type { ApiKey } from "../types";
import { useAppStore } from "../store/useAppStore";
import { PageHeader, Card } from "../components/ui";
import { Button, Modal } from "../components/Modal";

const inputCls =
  "w-full rounded-md border border-gray-300 px-3 py-1.5 text-sm focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500";

function statusBadge(status: ApiKey["status"]) {
  return status === "active" ? (
    <span className="inline-flex items-center gap-1.5 rounded-md bg-green-50 px-2 py-0.5 text-xs font-medium text-green-700 ring-1 ring-green-200">
      <span className="h-1.5 w-1.5 rounded-full bg-green-500" /> 启用
    </span>
  ) : (
    <span className="inline-flex items-center gap-1.5 rounded-md bg-gray-100 px-2 py-0.5 text-xs font-medium text-gray-500 ring-1 ring-gray-200">
      <span className="h-1.5 w-1.5 rounded-full bg-gray-400" /> 停用
    </span>
  );
}

function CreateKeyModal({
  open,
  onClose,
  onCreate,
}: {
  open: boolean;
  onClose: () => void;
  onCreate: (name: string, quotaLimit: number, expiresAt: string | null) => Promise<string>;
}) {
  const [name, setName] = useState("");
  const [quota, setQuota] = useState<string>("-1");
  const [expires, setExpires] = useState("");
  const [revealedKey, setRevealedKey] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  return (
    <Modal open={open} title={revealedKey ? "密钥已创建" : "创建密钥"} onClose={onClose}>
      {revealedKey ? (
        <div className="space-y-4">
          <p className="text-sm text-gray-600">请复制并妥善保存你的密钥，关闭后无法再次查看完整密钥。</p>
          <div className="flex items-center gap-2 rounded-md border border-dashed border-indigo-300 bg-indigo-50 px-3 py-2.5">
            <code className="flex-1 break-all text-sm font-mono text-indigo-700">{revealedKey}</code>
            <button
              className="rounded-md p-1.5 text-indigo-600 hover:bg-indigo-100"
              onClick={() => navigator.clipboard?.writeText(revealedKey)}
              title="复制"
            >
              <Copy size={16} />
            </button>
          </div>
          <div className="flex justify-end">
            <Button onClick={onClose}>完成</Button>
          </div>
        </div>
      ) : (
        <form
          className="space-y-4"
          onSubmit={async (e) => {
            e.preventDefault();
            setSubmitting(true);
            try {
              const key = await onCreate(name, quota === "-1" ? -1 : Number(quota), expires || null);
              setRevealedKey(key);
            } finally {
              setSubmitting(false);
            }
          }}
        >
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-gray-700">密钥名称</span>
            <input required value={name} onChange={(e) => setName(e.target.value)} className={inputCls} placeholder="如 生产环境" />
          </label>
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-gray-700">配额限制（Token，-1 表示不限）</span>
            <input
              value={quota}
              onChange={(e) => setQuota(e.target.value)}
              className={inputCls}
              type="number"
              min={-1}
            />
          </label>
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-gray-700">过期时间（可选）</span>
            <input value={expires} onChange={(e) => setExpires(e.target.value)} className={inputCls} type="date" />
          </label>
          <div className="flex justify-end gap-2 pt-2">
            <Button variant="secondary" onClick={onClose}>
              取消
            </Button>
            <Button type="submit" disabled={submitting}>
              <KeyRound size={16} className="mr-1 inline" /> {submitting ? "生成中…" : "生成"}
            </Button>
          </div>
        </form>
      )}
    </Modal>
  );
}

export function Keys() {
  const { keys, loadingKeys, createKey, removeKey, toggleKeyStatus, refreshKeys } = useAppStore();
  const [modalOpen, setModalOpen] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<ApiKey | null>(null);

  useEffect(() => {
    refreshKeys();
  }, [refreshKeys]);

  const quotaUsed = (k: ApiKey) =>
    k.quotaLimit === -1 ? null : Math.min(100, Math.round((k.usedTokens / k.quotaLimit) * 100));

  return (
    <div>
      <PageHeader
        title="密钥"
        description="管理网关接入密钥（sk-waliapi-*）"
        actions={
          <Button onClick={() => setModalOpen(true)}>
            <Plus size={16} className="mr-1 inline" /> 创建密钥
          </Button>
        }
      />

      <Card className="p-0 overflow-hidden">
        <table className="w-full text-left text-sm">
          <thead className="border-b border-gray-200 bg-gray-50 text-xs text-gray-500">
            <tr>
              <th className="px-4 py-3 font-medium">名称</th>
              <th className="px-4 py-3 font-medium">密钥</th>
              <th className="px-4 py-3 font-medium">状态</th>
              <th className="px-4 py-3 font-medium">配额</th>
              <th className="px-4 py-3 font-medium">使用量</th>
              <th className="px-4 py-3 font-medium">过期时间</th>
              <th className="px-4 py-3 text-right font-medium">操作</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-100">
            {keys.map((k) => {
              const used = quotaUsed(k);
              return (
                <tr key={k.id} className="hover:bg-gray-50">
                  <td className="px-4 py-3 font-medium text-gray-900">{k.name}</td>
                  <td className="px-4 py-3">
                    <code className="font-mono text-xs text-gray-600">{k.key.slice(0, 13)}••••••••</code>
                  </td>
                  <td className="px-4 py-3">
                    <button onClick={() => toggleKeyStatus(k.id)} title="点击切换状态">
                      {statusBadge(k.status)}
                    </button>
                  </td>
                  <td className="px-4 py-3 text-gray-600">{k.quotaLimit === -1 ? "不限" : k.quotaLimit.toLocaleString()}</td>
                  <td className="px-4 py-3">
                    <div className="flex items-center gap-2">
                      <div className="h-1.5 w-24 overflow-hidden rounded-full bg-gray-100">
                        <div
                          className={`h-full rounded-full ${used !== null && used >= 90 ? "bg-red-500" : "bg-indigo-500"}`}
                          style={{ width: `${used ?? 0}%` }}
                        />
                      </div>
                      <span className="text-xs text-gray-500">
                        {used !== null ? `${used}%` : `${(k.usedTokens / 1000).toFixed(1)}k`}
                      </span>
                    </div>
                  </td>
                  <td className="px-4 py-3 text-gray-500">{k.expiresAt ?? "—"}</td>
                  <td className="px-4 py-3">
                    <div className="flex justify-end">
                      <button
                        className="rounded-md p-1.5 text-gray-400 hover:bg-red-50 hover:text-red-600"
                        onClick={() => setConfirmDelete(k)}
                        title="删除"
                      >
                        <Trash2 size={15} />
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
            {!loadingKeys && keys.length === 0 && (
              <tr>
                <td colSpan={7} className="px-4 py-10 text-center text-sm text-gray-400">
                  暂无密钥，点击右上角创建
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </Card>

      <CreateKeyModal
        open={modalOpen}
        onClose={() => {
          setModalOpen(false);
        }}
        onCreate={(name, quotaLimit, expiresAt) =>
          createKey({ name, quotaLimit, expiresAt }).then((r) => r.key)
        }
      />

      <Modal open={!!confirmDelete} title="删除密钥" onClose={() => setConfirmDelete(null)}>
        <p className="text-sm text-gray-600">
          确定要删除密钥 <span className="font-semibold text-gray-900">{confirmDelete?.name}</span> 吗？使用该密钥的请求将立即被拒绝。
        </p>
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setConfirmDelete(null)}>
            取消
          </Button>
          <Button
            variant="danger"
            onClick={() => {
              if (confirmDelete) removeKey(confirmDelete.id);
              setConfirmDelete(null);
            }}
          >
            删除
          </Button>
        </div>
      </Modal>
    </div>
  );
}
