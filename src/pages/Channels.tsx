import { useState } from "react";
import { Pencil, Plus, Trash2, Cable } from "lucide-react";
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
};

const emptyForm: FormState = { name: "", type: "OpenAI", baseUrl: "", model: "", apiKey: "", status: "active" };

function ChannelFormModal({
  open,
  initial,
  onClose,
  onSubmit,
}: {
  open: boolean;
  initial: FormState | null;
  onClose: () => void;
  onSubmit: (data: FormState) => void;
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
        <Field label="Base URL">
          <input required value={form.baseUrl} onChange={set("baseUrl")} className={inputCls} placeholder="https://api.openai.com/v1" />
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
        <Field label="API Key" hint={initial ? "留空则保持不变" : undefined}>
          <input
            value={form.apiKey}
            onChange={set("apiKey")}
            className={inputCls}
            placeholder={initial ? "••••••••" : "sk-... / AIza..."}
            type="password"
          />
        </Field>
        <div className="flex justify-end gap-2 pt-2">
          <Button variant="secondary" onClick={onClose}>
            取消
          </Button>
          <Button type="submit">{initial ? "保存" : "创建"}</Button>
        </div>
      </form>
    </Modal>
  );
}

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

export function Channels() {
  const { channels, addChannel, updateChannel, removeChannel, toggleChannelStatus, testChannel } = useAppStore();
  const [modalOpen, setModalOpen] = useState(false);
  const [editing, setEditing] = useState<Channel | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<Channel | null>(null);
  const [testMsg, setTestMsg] = useState<Record<string, string>>({});

  const openAdd = () => {
    setEditing(null);
    setModalOpen(true);
  };
  const openEdit = (c: Channel) => {
    setEditing(c);
    setModalOpen(true);
  };

  const handleSubmit = (data: FormState) => {
    if (editing) {
      updateChannel(editing.id, {
        name: data.name,
        type: data.type,
        baseUrl: data.baseUrl,
        model: data.model,
        status: data.status,
      });
    } else {
      addChannel({ name: data.name, type: data.type, baseUrl: data.baseUrl, model: data.model, status: data.status });
    }
    setModalOpen(false);
  };

  const handleTest = (id: string) => {
    setTestMsg((m) => ({ ...m, [id]: "测试中…" }));
    setTimeout(() => {
      testChannel(id);
      setTestMsg((m) => ({ ...m, [id]: "✅ 连接正常" }));
    }, 600);
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

      <Card className="p-0 overflow-hidden">
        <table className="w-full text-left text-sm">
          <thead className="border-b border-gray-200 bg-gray-50 text-xs text-gray-500">
            <tr>
              <th className="px-4 py-3 font-medium">名称</th>
              <th className="px-4 py-3 font-medium">类型</th>
              <th className="px-4 py-3 font-medium">模型</th>
              <th className="px-4 py-3 font-medium">状态</th>
              <th className="px-4 py-3 font-medium">延迟</th>
              <th className="px-4 py-3 font-medium">创建时间</th>
              <th className="px-4 py-3 text-right font-medium">操作</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-100">
            {channels.map((c) => (
              <tr key={c.id} className="hover:bg-gray-50">
                <td className="px-4 py-3">
                  <p className="font-medium text-gray-900">{c.name}</p>
                  <p className="text-xs text-gray-400">{c.baseUrl}</p>
                </td>
                <td className="px-4 py-3">
                  <ChannelTypeBadge type={c.type} />
                </td>
                <td className="px-4 py-3 text-gray-600">{c.model || "—"}</td>
                <td className="px-4 py-3">
                  <button onClick={() => toggleChannelStatus(c.id)} title="点击切换状态">
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
        } : null}
        onClose={() => setModalOpen(false)}
        onSubmit={handleSubmit}
      />

      <Modal open={!!confirmDelete} title="删除渠道" onClose={() => setConfirmDelete(null)}>
        <p className="text-sm text-gray-600">
          确定要删除渠道 <span className="font-semibold text-gray-900">{confirmDelete?.name}</span> 吗？此操作不可撤销。
        </p>
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setConfirmDelete(null)}>
            取消
          </Button>
          <Button
            variant="danger"
            onClick={() => {
              if (confirmDelete) removeChannel(confirmDelete.id);
              setConfirmDelete(null);
            }}
          >
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
