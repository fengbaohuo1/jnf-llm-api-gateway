import { useEffect, useMemo, useState } from "react";
import { Pencil, Plus, Trash2, Cable, Search } from "lucide-react";
import { channelTypes } from "../data/mock";
import type { Channel, ChannelStatus, ChannelType } from "../types";
import { useAppStore } from "../store/useAppStore";
import { PageHeader, Card } from "../components/ui";
import { Button, Modal } from "../components/Modal";
import { ChannelTypeBadge, StatusBadge } from "../components/Badge";

type FormState = {
  name: string;
  type: ChannelType;
  baseUrl: string;
  model: string;
  apiKey: string;
  status: ChannelStatus;
  priority: number;
  weight: number;
};

const emptyForm: FormState = { name: "", type: "OpenAI", baseUrl: "", model: "", apiKey: "", status: "active", priority: 0, weight: 1 };

const inputCls =
  "w-full rounded-md border border-gray-300 px-3 py-1.5 text-sm focus:border-indigo-500 focus:outline-none focus:ring-1 focus:ring-indigo-500";

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <span className="mb-1 block text-sm font-medium text-gray-700">{label}</span>
      {children}
      {hint && <span className="mt-0.5 block text-xs text-gray-400">{hint}</span>}
    </label>
  );
}

function ChannelFormModal({
  open,
  initial,
  onClose,
  onSubmit,
  submitting,
}: {
  open: boolean;
  initial: FormState | null;
  onClose: () => void;
  onSubmit: (data: FormState) => Promise<void>;
  submitting: boolean;
}) {
  const [form, setForm] = useState<FormState>(initial ?? emptyForm);

  // Reset form when opening with a fresh target.
  const [lastOpen, setLastOpen] = useState(open);
  if (open !== lastOpen) {
    setLastOpen(open);
    if (open) setForm(initial ?? emptyForm);
  }

  const set = (k: keyof FormState) => (e: React.ChangeEvent<HTMLInputElement | HTMLSelectElement>) =>
    setForm((f) => ({ ...f, [k]: e.target.value }));

  return (
    <Modal open={open} title={initial ? "编辑渠道" : "添加渠道"} onClose={onClose}>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          onSubmit(form);
        }}
      >
        <div className="grid grid-cols-2 gap-4">
          <Field label="渠道名称">
            <input required value={form.name} onChange={set("name")} className={inputCls} placeholder="如 OpenAI Main" />
          </Field>
          <Field label="渠道类型">
            <select value={form.type} onChange={set("type")} className={inputCls}>
              {channelTypes.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <Field label="Base URL" hint="必须以 http:// 或 https:// 开头">
          <input
            required
            value={form.baseUrl}
            onChange={set("baseUrl")}
            className={inputCls}
            placeholder="https://api.openai.com/v1"
            pattern="https?://.+"
            title="请输入合法的 http(s) 地址"
          />
        </Field>
        <div className="grid grid-cols-2 gap-4">
          <Field label="默认模型">
            <input value={form.model} onChange={set("model")} className={inputCls} placeholder="gpt-4o" />
          </Field>
          <Field label="状态">
            <select value={form.status} onChange={set("status")} className={inputCls}>
              <option value="active">启用</option>
              <option value="disabled">停用</option>
              <option value="error">异常</option>
            </select>
          </Field>
        </div>
        <div className="grid grid-cols-2 gap-4">
          <Field label="优先级" hint="故障转移时越大的越先尝试（主渠道 = 最高优先级）">
            <input
              value={form.priority}
              onChange={(e) => setForm((f) => ({ ...f, priority: Number(e.target.value) || 0 }))}
              className={inputCls}
              type="number"
              placeholder="0"
            />
          </Field>
          <Field label="权重" hint="同优先级内越大越可能被先选中">
            <input
              value={form.weight}
              onChange={(e) => setForm((f) => ({ ...f, weight: Number(e.target.value) || 0 }))}
              className={inputCls}
              type="number"
              placeholder="1"
            />
          </Field>
        </div>
        <Field label="API Key" hint={initial ? "留空则保持不变" : undefined}>
          <input
            value={form.apiKey}
            onChange={set("apiKey")}
            className={inputCls}
            placeholder={initial ? "••••••••（留空保持不变）" : "sk-... / AIza..."}
            type="password"
          />
        </Field>
        <div className="flex justify-end gap-2 pt-2">
          <Button variant="secondary" onClick={onClose} disabled={submitting}>
            取消
          </Button>
          <Button type="submit" disabled={submitting}>
            {submitting ? "提交中…" : initial ? "保存" : "创建"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

export function Channels() {
  const { channels, loadingChannels, addChannel, updateChannel, removeChannel, toggleChannelStatus, testChannel, refreshChannels } = useAppStore();
  const [modalOpen, setModalOpen] = useState(false);
  const [editing, setEditing] = useState<Channel | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<Channel | null>(null);
  const [testMsg, setTestMsg] = useState<Record<string, string>>({});
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 筛选
  const [keyword, setKeyword] = useState("");
  const [typeFilter, setTypeFilter] = useState("全部");
  const [statusFilter, setStatusFilter] = useState("全部");

  useEffect(() => {
    refreshChannels();
  }, [refreshChannels]);

  const filtered = useMemo(() => {
    const kw = keyword.trim().toLowerCase();
    return channels.filter((c) => {
      const matchKw =
        !kw ||
        c.name.toLowerCase().includes(kw) ||
        c.baseUrl.toLowerCase().includes(kw) ||
        c.model.toLowerCase().includes(kw);
      const matchType = typeFilter === "全部" || c.type === typeFilter;
      const matchStatus = statusFilter === "全部" || c.status === statusFilter;
      return matchKw && matchType && matchStatus;
    });
  }, [channels, keyword, typeFilter, statusFilter]);

  const openAdd = () => {
    setEditing(null);
    setModalOpen(true);
  };
  const openEdit = (c: Channel) => {
    setEditing(c);
    setModalOpen(true);
  };

  const handleSubmit = async (data: FormState) => {
    setSubmitting(true);
    setError(null);
    try {
      if (editing) {
        await updateChannel({
          id: editing.id,
          name: data.name,
          type: data.type,
          baseUrl: data.baseUrl,
          model: data.model,
          status: data.status,
          apiKey: data.apiKey,
          priority: data.priority,
          weight: data.weight,
        });
      } else {
        await addChannel({
          name: data.name,
          type: data.type,
          baseUrl: data.baseUrl,
          model: data.model,
          apiKey: data.apiKey,
          priority: data.priority,
          weight: data.weight,
        });
      }
      setModalOpen(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  };

  const handleTest = async (id: string) => {
    setTestMsg((m) => ({ ...m, [id]: "测试中…" }));
    try {
      const r = await testChannel(id);
      setTestMsg((m) => ({ ...m, [id]: r.ok ? `✅ ${r.message}` : `❌ ${r.message}` }));
    } catch (e) {
      setTestMsg((m) => ({ ...m, [id]: `❌ ${e instanceof Error ? e.message : String(e)}` }));
    }
  };

  const handleDelete = async () => {
    if (!confirmDelete) return;
    setError(null);
    try {
      await removeChannel(confirmDelete.id);
      setConfirmDelete(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setConfirmDelete(null);
    }
  };

  const handleToggle = async (c: Channel) => {
    setError(null);
    try {
      await toggleChannelStatus(c.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div>
      <PageHeader
        title="渠道"
        description="管理上游 LLM 服务提供方"
        actions={
          <Button onClick={openAdd}>
            <Plus size={16} className="mr-1 inline" /> 添加渠道
          </Button>
        }
      />

      {error && (
        <div className="mb-4 rounded-lg border border-red-200 bg-red-50 px-4 py-2.5 text-sm text-red-700">
          {error}
        </div>
      )}

      <Card className="mb-4">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-4">
          <div className="relative sm:col-span-2">
            <Search size={16} className="absolute left-3 top-1/2 -translate-y-1/2 text-gray-400" />
            <input
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
              className={`${inputCls} pl-9`}
              placeholder="按名称 / Base URL / 模型搜索"
            />
          </div>
          <select value={typeFilter} onChange={(e) => setTypeFilter(e.target.value)} className={inputCls}>
            <option value="全部">全部类型</option>
            {channelTypes.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
          <select value={statusFilter} onChange={(e) => setStatusFilter(e.target.value)} className={inputCls}>
            <option value="全部">全部状态</option>
            <option value="active">启用</option>
            <option value="disabled">停用</option>
            <option value="error">异常</option>
          </select>
        </div>
        <p className="mt-3 text-xs text-gray-400">
          共 {filtered.length} 个渠道{channels.length !== filtered.length && `（已筛选，全部 ${channels.length}）`}
        </p>
      </Card>

      <Card className="p-0 overflow-hidden">
        <table className="w-full text-left text-sm">
          <thead className="border-b border-gray-200 bg-gray-50 text-xs text-gray-500">
            <tr>
              <th className="px-4 py-3 font-medium">名称</th>
              <th className="px-4 py-3 font-medium">类型</th>
              <th className="px-4 py-3 font-medium">模型</th>
              <th className="px-4 py-3 font-medium">优先级</th>
              <th className="px-4 py-3 font-medium">状态</th>
              <th className="px-4 py-3 font-medium">延迟</th>
              <th className="px-4 py-3 font-medium">创建时间</th>
              <th className="px-4 py-3 text-right font-medium">操作</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-100">
            {filtered.map((c) => (
              <tr key={c.id} className="hover:bg-gray-50">
                <td className="px-4 py-3">
                  <p className="font-medium text-gray-900">{c.name}</p>
                  <p className="text-xs text-gray-400">{c.baseUrl}</p>
                </td>
                <td className="px-4 py-3">
                  <ChannelTypeBadge type={c.type} />
                </td>
                <td className="px-4 py-3 text-gray-600">{c.model || "—"}</td>
                <td className="px-4 py-3 text-gray-600">
                  <span className="font-medium text-gray-700">{c.priority}</span>
                  <span className="text-xs text-gray-400"> / {c.weight}</span>
                </td>
                <td className="px-4 py-3">
                  <button onClick={() => handleToggle(c)} title="点击切换状态">
                    <StatusBadge status={c.status} />
                  </button>
                </td>
                <td className="px-4 py-3 text-gray-600">
                  {c.latencyMs ? `${c.latencyMs} ms` : "—"}
                  {testMsg[c.id] && <span className="block text-xs text-indigo-600">{testMsg[c.id]}</span>}
                </td>
                <td className="px-4 py-3 text-gray-500">{c.createdAt}</td>
                <td className="px-4 py-3">
                  <div className="flex justify-end gap-1">
                    <IconBtn title="测试" onClick={() => handleTest(c.id)}>
                      <Cable size={15} />
                    </IconBtn>
                    <IconBtn title="编辑" onClick={() => openEdit(c)}>
                      <Pencil size={15} />
                    </IconBtn>
                    <IconBtn title="删除" danger onClick={() => setConfirmDelete(c)}>
                      <Trash2 size={15} />
                    </IconBtn>
                  </div>
                </td>
              </tr>
            ))}
            {!loadingChannels && filtered.length === 0 && (
              <tr>
                <td colSpan={8} className="px-4 py-10 text-center text-sm text-gray-400">
                  {channels.length === 0 ? "暂无渠道，点击右上角添加" : "没有符合筛选条件的渠道"}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </Card>

      <ChannelFormModal
        open={modalOpen}
        initial={editing ? {
          name: editing.name,
          type: editing.type,
          baseUrl: editing.baseUrl,
          model: editing.model,
          apiKey: "",
          status: editing.status,
          priority: editing.priority,
          weight: editing.weight,
        } : null}
        onClose={() => setModalOpen(false)}
        onSubmit={handleSubmit}
        submitting={submitting}
      />

      <Modal open={!!confirmDelete} title="删除渠道" onClose={() => setConfirmDelete(null)}>
        <p className="text-sm text-gray-600">
          确定要删除渠道 <span className="font-semibold text-gray-900">{confirmDelete?.name}</span> 吗？此操作不可撤销。
        </p>
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setConfirmDelete(null)}>
            取消
          </Button>
          <Button variant="danger" onClick={handleDelete}>
            <Trash2 size={15} className="mr-1 inline" /> 删除
          </Button>
        </div>
      </Modal>
    </div>
  );
}

function IconBtn({ children, onClick, title, danger }: { children: React.ReactNode; onClick: () => void; title: string; danger?: boolean }) {
  return (
    <button
      title={title}
      onClick={onClick}
      className={`rounded-md p-1.5 transition-colors ${
        danger ? "text-gray-400 hover:bg-red-50 hover:text-red-600" : "text-gray-400 hover:bg-indigo-50 hover:text-indigo-600"
      }`}
    >
      {children}
    </button>
  );
}
