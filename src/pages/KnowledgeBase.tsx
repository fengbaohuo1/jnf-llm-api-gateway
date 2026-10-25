import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  CheckCircle2,
  Database,
  FileText,
  Link2,
  Loader,
  MessageSquare,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  RefreshCw,
  Search,
  Send,
  Settings2,
  Sparkles,
  Trash2,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { PageHeader, Card } from "../components/ui";
import { Button, Modal } from "../components/Modal";
import { useAppStore } from "../store/useAppStore";
import { api } from "../api";
import type {
  Channel,
  ChatReply,
  KnowledgeBase,
  KbChunk,
  KbCitation,
  KbDocument,
  KbMessage,
  KbSearchMode,
  KbSession,
  KbTask,
  KbIndexStatus,
  EvalReport,
  SearchHit,
} from "../types";

// ---------------------------------------------------------------------------
// 通用小件
// ---------------------------------------------------------------------------

const inputCls =
  "w-full rounded-lg border border-transparent bg-inset px-3 py-1.5 text-sm text-main transition-colors placeholder:text-faint focus:border-transparent focus:bg-card focus:outline-none focus:ring-2 focus:ring-indigo-500/30";

const errOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** 展示时间：MM-DD HH:MM（后端时间已是北京时间文本） */
function shortTime(iso: string) {
  return iso.length >= 16 ? iso.slice(5, 16) : iso;
}

function fmtBytes(n: number) {
  if (n <= 0) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** 本地即时消息时间戳：与后端一致（北京时间，YYYY-MM-DD HH:MM:SS） */
function nowBeijing() {
  const d = new Date(Date.now() + 8 * 3600 * 1000);
  return d.toISOString().slice(0, 19).replace("T", " ");
}

function ErrBar({ text }: { text: string | null }) {
  if (!text) return null;
  return (
    <div className="mb-3 rounded-lg border border-red-200 bg-red-50 px-4 py-2.5 text-sm text-red-700 dark:border-red-900 dark:bg-red-950 dark:text-red-300">
      {text}
    </div>
  );
}

/** 库级状态徽标 */
function BaseStatusBadge({ status }: { status: KnowledgeBase["status"] }) {
  if (status === "disabled") {
    return (
      <span className="inline-flex items-center rounded-md bg-inset px-2 py-0.5 text-xs font-medium text-muted ">
        已停用
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1 rounded-md bg-green-50 px-2 py-0.5 text-xs font-medium text-green-700 ring-1 ring-green-200 dark:bg-green-950 dark:text-green-300 dark:ring-green-900">
      <span className="h-1.5 w-1.5 rounded-full bg-green-500" />
      启用
    </span>
  );
}

const indexBadge: Record<KbIndexStatus, { label: string; cls: string }> = {
  none: { label: "未建索引", cls: "bg-inset text-muted ring-edge" },
  building: {
    label: "索引构建中",
    cls: "bg-blue-50 text-blue-700 ring-blue-200 dark:bg-blue-950 dark:text-blue-300 dark:ring-blue-900",
  },
  ready: {
    label: "索引就绪",
    cls: "bg-green-50 text-green-700 ring-green-200 dark:bg-green-950 dark:text-green-300 dark:ring-green-900",
  },
  failed: {
    label: "索引失败",
    cls: "bg-red-50 text-red-700 ring-red-200 dark:bg-red-950 dark:text-red-300 dark:ring-red-900",
  },
};

function IndexBadge({ status }: { status: KbIndexStatus }) {
  const s = indexBadge[status];
  return (
    <span className={`inline-flex items-center rounded-md px-2 py-0.5 text-xs font-medium ring-1 ${s.cls}`}>
      {status === "building" && <Loader size={11} className="mr-1 animate-spin" />}
      {s.label}
    </span>
  );
}

function ConfirmBox({
  open,
  title,
  text,
  busy,
  onClose,
  onConfirm,
}: {
  open: boolean;
  title: string;
  text: React.ReactNode;
  busy?: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  return (
    <Modal open={open} title={title} onClose={onClose}>
      <p className="text-sm text-muted">{text}</p>
      <div className="mt-5 flex justify-end gap-2">
        <Button variant="secondary" onClick={onClose} disabled={busy}>
          取消
        </Button>
        <Button variant="danger" onClick={onConfirm} disabled={busy}>
          <Trash2 size={15} className="mr-1 inline" /> 删除
        </Button>
      </div>
    </Modal>
  );
}

/** 分块查看：文档全部分块 + 可选高亮命中块（检索/引用跳转共用） */
function ChunkPreviewModal({
  target,
  focusChunkId,
  onClose,
}: {
  target: { id: string; title: string } | null;
  focusChunkId: number | null;
  onClose: () => void;
}) {
  const [chunks, setChunks] = useState<KbChunk[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!target) return;
    let live = true;
    setChunks(null);
    setError(null);
    api
      .listChunks(target.id)
      .then((c) => {
        if (live) setChunks(c);
      })
      .catch((e) => {
        if (live) setError(errOf(e));
      });
    return () => {
      live = false;
    };
  }, [target]);

  useEffect(() => {
    if (!chunks || !focusChunkId) return;
    const el = document.getElementById(`kb-chunk-${focusChunkId}`);
    el?.scrollIntoView({ block: "center" });
  }, [chunks, focusChunkId]);

  return (
    <Modal
      open={!!target}
      title={target ? `分块预览 · ${target.title}` : ""}
      size="lg"
      onClose={onClose}
    >
      {error && <ErrBar text={error} />}
      {!chunks && !error && (
        <div className="flex items-center gap-2 py-6 text-sm text-faint">
          <Loader size={15} className="animate-spin" /> 加载分块中…
        </div>
      )}
      <div className="space-y-3">
        {chunks?.map((c) => (
          <div
            key={c.id}
            id={`kb-chunk-${c.id}`}
            className={`rounded-lg border p-3 ${
              focusChunkId === c.id
                ? "border-indigo-400 bg-indigo-50 ring-1 ring-indigo-300 dark:bg-indigo-950/40 dark:ring-indigo-800"
                : "border-edge bg-inset/60"
            }`}
          >
            <div className="mb-1.5 flex flex-wrap items-center gap-2 text-xs">
              <span className="rounded bg-accent-soft px-1.5 py-0.5 font-mono text-accent">#{c.seq}</span>
              <span className="font-medium text-sub">{c.heading || "（无章节标题）"}</span>
              <span className="ml-auto text-faint">{c.chars} 字符</span>
            </div>
            <p className="whitespace-pre-wrap break-words text-sm leading-relaxed text-muted">{c.content}</p>
          </div>
        ))}
        {chunks?.length === 0 && !error && (
          <p className="py-6 text-center text-sm text-faint">该文档没有分块（导入可能失败或为空）</p>
        )}
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// 库：新建 / 编辑
// ---------------------------------------------------------------------------

function BaseFormModal({
  open,
  initial,
  onClose,
  onSaved,
}: {
  open: boolean;
  initial: KnowledgeBase | null;
  onClose: () => void;
  onSaved: (b: KnowledgeBase) => void;
}) {
  const editing = !!initial;
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [chunkSize, setChunkSize] = useState(800);
  const [chunkOverlap, setChunkOverlap] = useState(150);
  const [status, setStatus] = useState<"active" | "disabled">("active");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 每次打开时回填表单
  const [lastOpen, setLastOpen] = useState(open);
  if (open !== lastOpen) {
    setLastOpen(open);
    if (open) {
      setName(initial?.name ?? "");
      setDescription(initial?.description ?? "");
      setChunkSize(initial?.chunkSize ?? 800);
      setChunkOverlap(initial?.chunkOverlap ?? 150);
      setStatus(initial?.status ?? "active");
      setError(null);
    }
  }

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const b = initial
        ? await api.updateBase({
            id: initial.id,
            name: name.trim() || undefined,
            description: description.trim() || undefined,
            chunkSize,
            chunkOverlap,
            status,
          })
        : await api.createBase({
            name: name.trim(),
            description: description.trim() || undefined,
            chunkSize,
            chunkOverlap,
          });
      onSaved(b);
      onClose();
    } catch (e) {
      setError(errOf(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal open={open} title={editing ? "编辑知识库" : "新建知识库"} onClose={onClose}>
      <ErrBar text={error} />
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <label className="block">
          <span className="mb-1 block text-sm font-medium text-sub">名称</span>
          <input required value={name} onChange={(e) => setName(e.target.value)} className={inputCls} placeholder="如 面试手记" />
        </label>
        <label className="block">
          <span className="mb-1 block text-sm font-medium text-sub">描述（可选）</span>
          <textarea
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            className={`${inputCls} resize-none`}
            rows={2}
            placeholder="这个知识库装什么？"
          />
        </label>
        <div className="grid grid-cols-2 gap-4">
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-sub">分块大小（字符）</span>
            <input
              required
              type="number"
              min={100}
              max={4000}
              value={chunkSize}
              onChange={(e) => setChunkSize(Number(e.target.value) || 800)}
              className={inputCls}
            />
          </label>
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-sub">分块重叠（字符）</span>
            <input
              required
              type="number"
              min={0}
              value={chunkOverlap}
              onChange={(e) => setChunkOverlap(Number(e.target.value) || 0)}
              className={inputCls}
            />
          </label>
        </div>
        <p className="text-xs text-faint">
          重叠只作用于超长段落的重叠硬窗；编辑后对未导入文档生效，不影响已存在分块。
        </p>
        {editing && (
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-sub">状态</span>
            <select value={status} onChange={(e) => setStatus(e.target.value as "active" | "disabled")} className={inputCls}>
              <option value="active">启用</option>
              <option value="disabled">停用（不可检索与问答）</option>
            </select>
          </label>
        )}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="secondary" onClick={onClose} disabled={busy}>
            取消
          </Button>
          <Button type="submit" disabled={busy}>
            {busy ? "保存中…" : editing ? "保存" : "创建"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// 绑定渠道：embedding（语义检索）与问答（RAG）解耦配置
// ---------------------------------------------------------------------------

function BindModal({
  base,
  channels,
  open,
  onClose,
  onSaved,
}: {
  base: KnowledgeBase | null;
  channels: Channel[];
  open: boolean;
  onClose: () => void;
  onSaved: (b: KnowledgeBase) => void;
}) {
  // embedding 区
  const [embChannel, setEmbChannel] = useState("");
  const [embModel, setEmbModel] = useState("");
  // 问答区
  const [ansChannel, setAnsChannel] = useState("");
  const [ansModel, setAnsModel] = useState("");

  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  useEffect(() => {
    if (!open || !base) return;
    setEmbChannel(base.embeddingChannelId ?? "");
    setEmbModel(base.embeddingModel ?? "");
    setAnsChannel(base.answerChannelId ?? "");
    setAnsModel(base.answerModel ?? "");
    setError(null);
    setSaved(null);
    // 仅在切换打开目标库时重置（保存后的对象替换不清空提示）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, base?.id]);

  const embChannelObj = channels.find((c) => c.id === embChannel);
  const ansChannelObj = channels.find((c) => c.id === ansChannel);
  // 渠道下拉：active + 已绑定的（可能是停用渠道，需可见以便解绑/换绑）
  const listable = channels.filter((c) => c.status === "active" || c.id === embChannel || c.id === ansChannel);

  const runBind = async (kind: "embedding" | "answer") => {
    if (!base) return;
    setBusyKey(kind);
    setError(null);
    setSaved(null);
    try {
      let updated: KnowledgeBase;
      if (kind === "embedding") {
        if (!embChannel || !embModel.trim()) {
          throw new Error("请选择 embedding 渠道并填写模型（如 text-embedding-3-small）");
        }
        updated = await api.bindEmbedding({ kbId: base.id, channelId: embChannel, model: embModel.trim() });
      } else {
        if (!ansChannel) {
          throw new Error("请选择问答渠道");
        }
        updated = await api.bindAnswer({
          kbId: base.id,
          channelId: ansChannel,
          model: ansModel.trim() || undefined,
        });
      }
      onSaved(updated);
      setSaved(kind === "embedding" ? "语义检索绑定已保存" : "问答渠道绑定已保存");
    } catch (e) {
      setError(errOf(e));
    } finally {
      setBusyKey(null);
    }
  };

  const runUnbind = async (kind: "embedding" | "answer") => {
    if (!base) return;
    setBusyKey(kind);
    setError(null);
    setSaved(null);
    try {
      const updated = kind === "embedding" ? await api.unbindEmbedding(base.id) : await api.unbindAnswer(base.id);
      onSaved(updated);
      setEmbChannel("");
      setEmbModel("");
      setAnsChannel("");
      setAnsModel("");
      setSaved(kind === "embedding" ? "已解除语义检索绑定（旧向量已作废）" : "已解除问答绑定");
    } catch (e) {
      setError(errOf(e));
    } finally {
      setBusyKey(null);
    }
  };

  const channelLabel = (c: Channel) => `${c.name}（${c.type}${c.status === "active" ? "" : " · 已停用"}）`;

  return (
    <Modal open={open && !!base} title={`绑定渠道 · ${base?.name ?? ""}`} onClose={onClose}>
      <ErrBar text={error} />
      {saved && (
        <div className="mb-3 flex items-center gap-1.5 rounded-lg border border-green-200 bg-green-50 px-4 py-2 text-sm text-green-700 dark:border-green-900 dark:bg-green-950 dark:text-green-300">
          <CheckCircle2 size={15} /> {saved}
        </div>
      )}
      {channels.length === 0 && (
        <div className="mb-3 rounded-xl bg-inset/70 px-4 py-3 text-sm text-muted">
          还没有任何渠道，请先到「渠道」页添加（如 OpenAI / DeepSeek / Custom）。
        </div>
      )}

      {/* 语义检索 embedding */}
      <div className="mb-5 rounded-xl bg-inset/50 p-3">
        <p className="text-sm font-semibold text-main">
          语义检索（embedding）
          <span className="ml-2 text-xs font-normal text-faint">关键词模式不需要；向量 / 混合检索需要</span>
        </p>
        {base?.embeddingChannelId ? (
          <p className="mt-2 text-xs text-emerald-600 dark:text-emerald-400">
            当前已绑定 · 维度 {base.embeddingDim} · 换绑会清空全部旧向量并需重新构建索引
          </p>
        ) : (
          <p className="mt-2 text-xs text-faint">绑定后导入文档可一键构建向量索引</p>
        )}
        <div className="mt-3 space-y-3">
          <select value={embChannel} onChange={(e) => setEmbChannel(e.target.value)} className={inputCls}>
            <option value="">选择 embedding 渠道…</option>
            {listable.map((c) => (
              <option key={c.id} value={c.id}>
                {channelLabel(c)}
              </option>
            ))}
          </select>
          <div className="flex gap-2">
            <input
              value={embModel}
              onChange={(e) => setEmbModel(e.target.value)}
              className={`${inputCls} min-w-0 flex-1`}
              placeholder={embChannelObj?.model ? `模型（默认 ${embChannelObj.model}）` : "模型，如 text-embedding-3-small"}
            />
            {base?.embeddingChannelId ? (
              <Button variant="secondary" disabled={busyKey !== null} onClick={() => void runUnbind("embedding")}>
                {busyKey === "embedding" ? "处理中…" : "解绑"}
              </Button>
            ) : null}
          </div>
          <div>
            <Button
              disabled={busyKey !== null || !embChannel}
              onClick={() => void runBind("embedding")}
            >
              {busyKey === "embedding" ? (
                <>
                  <Loader size={14} className="mr-1 inline animate-spin" /> 保存中…
                </>
              ) : base?.embeddingChannelId ? (
                "更换绑定"
              ) : (
                "绑定"
              )}
            </Button>
          </div>
        </div>
      </div>

      {/* 问答 RAG */}
      <div className="rounded-xl bg-inset/50 p-3">
        <p className="text-sm font-semibold text-main">
          问答渠道（RAG）
          <span className="ml-2 text-xs font-normal text-faint">生成回答用；不填模型时默认取渠道模型</span>
        </p>
        {base?.answerChannelId && (
          <p className="mt-2 text-xs text-emerald-600 dark:text-emerald-400">当前已绑定，可在「问答」标签直接对话</p>
        )}
        <div className="mt-3 space-y-3">
          <select value={ansChannel} onChange={(e) => setAnsChannel(e.target.value)} className={inputCls}>
            <option value="">选择问答渠道…</option>
            {listable.map((c) => (
              <option key={c.id} value={c.id}>
                {channelLabel(c)}
              </option>
            ))}
          </select>
          <div className="flex gap-2">
            <input
              value={ansModel}
              onChange={(e) => setAnsModel(e.target.value)}
              className={`${inputCls} min-w-0 flex-1`}
              placeholder={ansChannelObj?.model ? `问答模型（默认 ${ansChannelObj.model}）` : "问答模型（可选）"}
            />
            {base?.answerChannelId ? (
              <Button variant="secondary" disabled={busyKey !== null} onClick={() => void runUnbind("answer")}>
                {busyKey === "answer" ? "处理中…" : "解绑"}
              </Button>
            ) : null}
          </div>
          <div>
            <Button disabled={busyKey !== null || !ansChannel} onClick={() => void runBind("answer")}>
              {busyKey === "answer" ? (
                <>
                  <Loader size={14} className="mr-1 inline animate-spin" /> 保存中…
                </>
              ) : base?.answerChannelId ? (
                "更换绑定"
              ) : (
                "绑定"
              )}
            </Button>
          </div>
        </div>
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// 文档 Tab：导入 / 列表 / 索引构建
// ---------------------------------------------------------------------------

function DocsTab({
  base,
  active,
  onBaseRefresh,
  openChunk,
  onOpenBind,
}: {
  base: KnowledgeBase;
  /** 当前 Tab 是否可见（隐藏时暂停轮询） */
  active: boolean;
  onBaseRefresh: () => void;
  openChunk: (target: { id: string; title: string }, focus: number | null) => void;
  onOpenBind: () => void;
}) {
  const [docs, setDocs] = useState<KbDocument[] | null>(null);
  const [tasks, setTasks] = useState<KbTask[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [confirmDel, setConfirmDel] = useState<KbDocument | null>(null);
  const [needsRebuild, setNeedsRebuild] = useState(false);

  const loadDocs = useCallback(async () => {
    try {
      setDocs(await api.listDocuments(base.id));
    } catch (e) {
      setError(errOf(e));
    }
  }, [base.id]);

  const loadTasks = useCallback(async () => {
    try {
      setTasks(await api.listTasks(base.id, 10));
    } catch {
      // 任务读取失败不打断主流程
    }
  }, [base.id]);

  // 文档列表：库切换 / 每次操作后刷新
  useEffect(() => {
    let live = true;
    setDocs(null);
    setError(null);
    setNeedsRebuild(false);
    api
      .listDocuments(base.id)
      .then((d) => {
        if (live) setDocs(d);
      })
      .catch((e) => {
        if (live) setError(errOf(e));
      });
    void loadTasks();
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [base.id, loadTasks]);

  // 构建任务轮询：有 running/pending 时每 1.5s 刷新；结束后回读库状态
  const running = tasks.some((t) => t.status === "pending" || t.status === "running");
  useEffect(() => {
    if (!active || !running) return;
    const timer = setInterval(() => {
      void (async () => {
        const latest = await api.listTasks(base.id, 10).catch(() => null);
        if (!latest) return;
        setTasks(latest);
        const still = latest.some((t) => t.status === "pending" || t.status === "running");
        if (!still) {
          onBaseRefresh();
          setNeedsRebuild(false);
        }
      })();
    }, 1500);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, running, base.id]);

  const startBuild = async () => {
    setError(null);
    try {
      await api.buildIndex(base.id);
      await loadTasks();
    } catch (e) {
      setError(errOf(e));
    }
  };

  const importFiles = async () => {
    setError(null);
    const sel = await open({
      multiple: true,
      filters: [{ name: "文档", extensions: ["md", "markdown", "txt"] }],
    });
    if (!sel) return;
    const picked = Array.isArray(sel) ? sel : [sel];
    const errs: string[] = [];
    for (const p of picked) {
      try {
        await api.importFile({ kbId: base.id, path: p });
      } catch (e) {
        errs.push(errOf(e));
      }
    }
    if (errs.length > 0) setError(`部分文件导入失败：${errs.slice(0, 2).join("；")}`);
    await loadDocs();
    markChanged();
  };

  /** 索引就绪时任何文档增删都提示重建 */
  const markChanged = () => {
    if (base.indexStatus === "ready") setNeedsRebuild(true);
  };

  const deleteDoc = async () => {
    if (!confirmDel) return;
    try {
      await api.deleteDocument(confirmDel.id);
      setDocs((ds) => (ds ? ds.filter((d) => d.id !== confirmDel.id) : ds));
      setConfirmDel(null);
      markChanged();
    } catch (e) {
      setError(errOf(e));
      setConfirmDel(null);
    }
  };

  const lastTask = tasks[0];
  const activeTask = tasks.find((t) => t.status === "pending" || t.status === "running") ?? null;
  const embeddingReady = base.embeddingChannelId !== null;
  const indexReady = base.indexStatus === "ready";

  return (
    <div className="space-y-4">
      {/* 绑定与索引引导 */}
      {!embeddingReady && (
        <div className="rounded-xl bg-inset/60 px-4 py-3 text-sm text-muted">
          尚未绑定语义检索渠道：现在可以<span className="font-medium text-main">导入文档 + 关键词检索</span>；
          要使用向量 / 混合检索与 RAG 问答，请
          <button className="mx-1 font-medium text-indigo-600 hover:underline dark:text-indigo-400" onClick={onOpenBind}>
            绑定渠道
          </button>
          。
        </div>
      )}

      {embeddingReady && (
        <Card>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <p className="text-sm font-semibold text-sub">向量索引</p>
              {indexReady ? (
                <p className="mt-0.5 text-xs text-faint">
                  索引就绪 · 维度 {base.embeddingDim}
                  {needsRebuild && <span className="text-amber-600 dark:text-amber-400"> · 文档有增删，需重新构建以保持同步</span>}
                </p>
              ) : (
                <p className="mt-0.5 text-xs text-faint">
                  {base.indexStatus === "building" || activeTask
                    ? "正在为分块生成向量并构建 HNSW 图…"
                    : base.indexStatus === "failed"
                      ? "上次构建失败，可检查任务详情后重试"
                      : "构建后即可使用向量 / 混合检索与 RAG 问答"}
                </p>
              )}
            </div>
            {activeTask ? (
              <div className="w-56">
                <p className="mb-1 text-xs text-muted">
                  {activeTask.progress}% · {activeTask.doneItems}/{activeTask.totalItems} 块
                </p>
                <div className="h-1.5 overflow-hidden rounded-full bg-inset">
                  <div
                    className="h-full rounded-full bg-indigo-500 transition-all"
                    style={{ width: `${Math.max(activeTask.progress, 2)}%` }}
                  />
                </div>
              </div>
            ) : (
              <Button
                variant="secondary"
                onClick={() => void startBuild()}
                disabled={!indexReady && base.indexStatus === "building"}
              >
                <RefreshCw size={14} className="mr-1 inline" />
                {needsRebuild || base.indexStatus === "failed" ? "重新构建" : indexReady ? "重建索引" : "构建索引"}
              </Button>
            )}
          </div>
          {!activeTask && lastTask?.status === "failed" && lastTask.errorMessage && (
            <p className="mt-2 rounded-md bg-red-50 px-3 py-2 text-xs text-red-700 dark:bg-red-950 dark:text-red-300">
              上次构建失败：{lastTask.errorMessage}
            </p>
          )}
        </Card>
      )}

      {/* 文档工具条 */}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-sm text-muted">
          {docs ? `共 ${docs.length} 篇文档` : "加载中…"}
        </p>
        <div className="flex gap-2">
          <Button variant="secondary" onClick={() => void loadDocs()}>
            <RefreshCw size={14} className="mr-1 inline" /> 刷新
          </Button>
          <Button variant="secondary" onClick={() => void importFiles()}>
            <FileText size={14} className="mr-1 inline" /> 导入文件
          </Button>
          <Button onClick={() => setImportOpen(true)}>
            <Plus size={15} className="mr-1 inline" /> 导入文本
          </Button>
        </div>
      </div>

      <ErrBar text={error} />

      {/* 文档列表 */}
      <Card className="p-0 overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead className="bg-inset/80 text-xs text-muted">
            <tr>
              <th className="px-3 py-2 font-medium">标题</th>
              <th className="px-3 py-2 font-medium">格式</th>
              <th className="px-3 py-2 font-medium">大小</th>
              <th className="px-3 py-2 font-medium">分块</th>
              <th className="px-3 py-2 font-medium">状态</th>
              <th className="px-3 py-2 font-medium">导入时间</th>
              <th className="px-3 py-2 text-right font-medium">操作</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-edge/40">
            {docs?.map((d) => (
              <tr key={d.id} className="hover:bg-hover">
                <td className="max-w-72 px-3 py-2">
                  <p className="truncate font-medium text-main" title={d.title}>
                    {d.title}
                  </p>
                  <p className="text-xs text-faint">{d.charCount.toLocaleString()} 字符</p>
                </td>
                <td className="px-3 py-2">
                  <span className="rounded bg-inset px-1.5 py-0.5 font-mono text-xs text-muted ">{d.format}</span>
                </td>
                <td className="px-3 py-2 text-muted">{fmtBytes(d.fileSize) || "—"}</td>
                <td className="px-3 py-2 text-muted">{d.chunkCount}</td>
                <td className="px-3 py-2">
                  {d.status === "ready" && (
                    <span className="text-xs text-green-600 dark:text-green-400">就绪</span>
                  )}
                  {d.status === "importing" && <span className="text-xs text-amber-600 dark:text-amber-400">导入中…</span>}
                  {d.status === "failed" && (
                    <span className="text-xs text-red-600 dark:text-red-400" title={d.error ?? ""}>
                      导入失败
                    </span>
                  )}
                </td>
                <td className="px-3 py-2 text-muted">{shortTime(d.createdAt)}</td>
                <td className="px-3 py-2">
                  <div className="flex justify-end gap-2">
                    <button
                      className="text-xs font-medium text-indigo-600 hover:underline dark:text-indigo-400"
                      onClick={() => openChunk({ id: d.id, title: d.title }, null)}
                    >
                      分块预览
                    </button>
                    <button
                      className="text-xs font-medium text-red-600 hover:underline dark:text-red-400"
                      onClick={() => setConfirmDel(d)}
                    >
                      删除
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {docs && docs.length === 0 && (
          <div className="flex flex-col items-center gap-2 py-10 text-center">
            <Database size={28} className="text-faint" />
            <p className="text-sm text-faint">还没有文档</p>
            <p className="text-xs text-faint">导入 .md / .txt 文件，或直接粘贴文本；知识库支持多篇文档与章节级溯源</p>
          </div>
        )}
        {!docs && (
          <div className="flex items-center justify-center gap-2 py-10 text-sm text-faint">
            <Loader size={15} className="animate-spin" /> 加载文档中…
          </div>
        )}
      </Card>

      <ConfirmBox
        open={!!confirmDel}
        title="删除文档"
        text={
          <>
            确定删除文档 <span className="font-semibold text-main">{confirmDel?.title}</span> 吗？其全部
            {confirmDel?.chunkCount ?? 0} 个分块与全文索引将一并删除。
          </>
        }
        onClose={() => setConfirmDel(null)}
        onConfirm={() => void deleteDoc()}
      />

      <ImportTextModal
        open={importOpen}
        kbId={base.id}
        onClose={() => setImportOpen(false)}
        onImported={() => {
          void loadDocs();
          markChanged();
        }}
      />
    </div>
  );
}

function ImportTextModal({
  open,
  kbId,
  onClose,
  onImported,
}: {
  open: boolean;
  kbId: string;
  onClose: () => void;
  onImported: () => void;
}) {
  const [title, setTitle] = useState("");
  const [format, setFormat] = useState<"md" | "txt">("txt");
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [lastOpen, setLastOpen] = useState(open);
  if (open !== lastOpen) {
    setLastOpen(open);
    if (open) {
      setTitle("");
      setFormat("txt");
      setText("");
      setError(null);
    }
  }

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.importText({ kbId, title: title.trim(), format, text });
      onImported();
      onClose();
    } catch (e) {
      setError(errOf(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal open={open} title="导入文本" onClose={onClose} size="lg">
      <ErrBar text={error} />
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <div className="grid grid-cols-2 gap-4">
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-sub">标题</span>
            <input required value={title} onChange={(e) => setTitle(e.target.value)} className={inputCls} placeholder="文档标题" />
          </label>
          <label className="block">
            <span className="mb-1 block text-sm font-medium text-sub">格式</span>
            <select value={format} onChange={(e) => setFormat(e.target.value as "md" | "txt")} className={inputCls}>
              <option value="txt">纯文本 txt</option>
              <option value="md">Markdown md（章节级分块）</option>
            </select>
          </label>
        </div>
        <label className="block">
          <span className="mb-1 block text-sm font-medium text-sub">内容</span>
          <textarea
            required
            value={text}
            onChange={(e) => setText(e.target.value)}
            className={`${inputCls} resize-y font-mono`}
            rows={12}
            placeholder={"md 示例：\n# 第一章\n\n……"}
          />
        </label>
        <div className="flex justify-end gap-2">
          <Button type="button" variant="secondary" onClick={onClose} disabled={busy}>
            取消
          </Button>
          <Button type="submit" disabled={busy || !text.trim()}>
            {busy ? "导入中…" : "导入"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// 检索 Tab
// ---------------------------------------------------------------------------

const MODES: { key: KbSearchMode; label: string; desc: string }[] = [
  { key: "keyword", label: "关键词", desc: "FTS5 BM25，快且稳，不需要索引" },
  { key: "vector", label: "向量", desc: "语义相似度，需已构建向量索引" },
  { key: "hybrid", label: "混合", desc: "两路 RRF 融合，召回更全" },
];

function SearchTab({
  base,
  openChunk,
}: {
  base: KnowledgeBase;
  openChunk: (target: { id: string; title: string }, focus: number | null) => void;
}) {
  const canVector = base.embeddingChannelId !== null && base.indexStatus === "ready";
  const [mode, setMode] = useState<KbSearchMode>(canVector ? "hybrid" : "keyword");
  const [query, setQuery] = useState("");
  const [topK, setTopK] = useState(8);
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [ran, setRan] = useState(false);

  // 切换库时重置
  useEffect(() => {
    setQuery("");
    setHits(null);
    setRan(false);
    setError(null);
    setMode(canVector ? "hybrid" : "keyword");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [base.id]);

  const doSearch = async () => {
    const q = query.trim();
    if (!q || searching) return;
    setSearching(true);
    setError(null);
    try {
      const r = await api.search({ kbId: base.id, query: q, mode, topK });
      setHits(r);
      setRan(true);
    } catch (e) {
      setError(errOf(e));
      setHits(null);
      setRan(true);
    } finally {
      setSearching(false);
    }
  };

  const modeDesc = MODES.find((m) => m.key === mode)?.desc ?? "";
  const fmtScore = (h: SearchHit) => {
    if (mode === "vector") return `相似度 ${h.score.toFixed(3)}`;
    if (mode === "keyword") {
      return h.kwScore !== null ? `bm25 ${h.kwScore.toFixed(2)}` : `关键词分 ${h.score.toFixed(2)}`;
    }
    const cos = h.vecScore !== null ? ` · 相似度 ${h.vecScore.toFixed(3)}` : "";
    return `融合分 ${h.score.toFixed(3)}${cos}`;
  };

  return (
    <div className="space-y-4">
      {!canVector && (
        <div className="rounded-xl bg-inset/60 px-4 py-3 text-xs text-muted">
          向量与混合检索需先绑定 embedding 渠道并构建索引（见「文档」页）；当前仅关键词可用。
        </div>
      )}
      <Card>
        <div className="flex flex-wrap gap-2">
          {MODES.map((m) => (
            <button
              key={m.key}
              onClick={() => {
                setMode(m.key);
                setHits(null);
                setRan(false);
              }}
              title={m.desc}
              className={`rounded-md px-3 py-1.5 text-sm font-medium transition-colors ${
                mode === m.key ? "bg-card text-accent shadow-sm" : "text-muted hover:text-sub"
              }`}
            >
              {m.label}
            </button>
          ))}
        </div>
        <p className="mt-2 text-xs text-faint">当前模式：{modeDesc}</p>
        <form
          className="mt-3 flex gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            void doSearch();
          }}
        >
          {/* flex-1 + min-w-0：输入框独占剩余宽度，不被右侧控件挤压 */}
          <input
            type="text"
            autoComplete="off"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void doSearch();
              }
            }}
            className={`${inputCls} min-w-0 flex-1`}
            placeholder="输入要检索的内容，回车搜索"
          />
          <div className="flex shrink-0 items-center gap-2">
            <select
              value={topK}
              onChange={(e) => setTopK(Number(e.target.value))}
              className={`${inputCls} w-24 shrink-0`}
              title="返回条数"
            >
              <option value={5}>5 条</option>
              <option value={8}>8 条</option>
              <option value={15}>15 条</option>
              <option value={30}>30 条</option>
            </select>
            <Button type="submit" disabled={searching || !query.trim()}>
              {searching ? (
                <>
                  <Loader size={14} className="mr-1 inline animate-spin" /> 检索中
                </>
              ) : (
                <>
                  <Search size={14} className="mr-1 inline" /> 搜索
                </>
              )}
            </Button>
          </div>
        </form>
      </Card>

      <ErrBar text={error} />

      <div className="space-y-2">
        {hits?.map((h, i) => (
          <button
            key={h.chunkId}
            onClick={() => openChunk({ id: h.documentId, title: h.title }, h.chunkId)}
            className="block w-full rounded-xl bg-inset/40 p-3 text-left transition-colors hover:bg-inset"
          >
            <div className="flex items-center gap-2">
              <span className="w-5 shrink-0 text-right font-mono text-xs text-faint">{i + 1}</span>
              <p className="truncate text-sm font-semibold text-main">{h.title}</p>
              {h.heading && <p className="truncate text-xs text-faint">· {h.heading}</p>}
              <span className="ml-auto shrink-0 rounded bg-accent-soft px-1.5 py-0.5 text-xs text-accent">
                {fmtScore(h)}
              </span>
            </div>
            <p className="mt-1 line-clamp-2 pl-7 text-sm text-muted">{h.content}</p>
          </button>
        ))}
        {hits && hits.length === 0 && (
          <div className="rounded-xl bg-inset/40 py-10 text-center text-sm text-faint">
            没有命中{query.trim() ? `“${query.trim()}”` : ""}的分块
          </div>
        )}
        {!hits && !error && (
          <div className="rounded-lg border border-dashed border-edge/60 py-10 text-center text-sm text-faint">
            {ran ? "" : "输入查询内容开始检索；点击结果可查看命中的分块原文"}
          </div>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// 问答 Tab
// ---------------------------------------------------------------------------

interface LocalMsg {
  role: "user" | "assistant";
  content: string;
  citations: KbCitation[];
  model: string | null;
  tokensUsed: number;
  createdAt: string;
}

function ChatTab({
  base,
  onOpenBind,
  openChunk,
}: {
  base: KnowledgeBase;
  onOpenBind: () => void;
  openChunk: (target: { id: string; title: string }, focus: number | null) => void;
}) {
  const [sessions, setSessions] = useState<KbSession[] | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [msgs, setMsgs] = useState<LocalMsg[] | null>(null);
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmDelSession, setConfirmDelSession] = useState<KbSession | null>(null);
  const bottomRef = useRef<HTMLDivElement | null>(null);

  const loadSessions = useCallback(async () => {
    try {
      setSessions(await api.listSessions(base.id));
    } catch (e) {
      setError(errOf(e));
    }
  }, [base.id]);

  useEffect(() => {
    setSessions(null);
    setActiveId(null);
    setMsgs(null);
    setInput("");
    setError(null);
    void loadSessions();
  }, [base.id, loadSessions]);

  const active = sessions?.find((s) => s.id === activeId) ?? null;

  const openSession = async (id: string) => {
    setActiveId(id);
    setMsgs(null);
    setError(null);
    try {
      const rows = await api.listMessages(id);
      setMsgs(
        rows.map((m: KbMessage) => ({
          role: m.role,
          content: m.content,
          citations: m.sources,
          model: m.model,
          tokensUsed: m.tokensUsed,
          createdAt: m.createdAt,
        })),
      );
    } catch (e) {
      setError(errOf(e));
    }
  };

  const newSession = async () => {
    try {
      const s = await api.createSession({ kbId: base.id });
      setSessions((prev) => (prev ? [s, ...prev] : [s]));
      await openSession(s.id);
    } catch (e) {
      setError(errOf(e));
    }
  };

  const removeSession = async () => {
    if (!confirmDelSession) return;
    try {
      await api.deleteSession(confirmDelSession.id);
      setSessions((prev) => prev?.filter((s) => s.id !== confirmDelSession.id) ?? null);
      if (activeId === confirmDelSession.id) {
        setActiveId(null);
        setMsgs(null);
      }
      setConfirmDelSession(null);
    } catch (e) {
      setError(errOf(e));
      setConfirmDelSession(null);
    }
  };

  const send = async () => {
    const q = input.trim();
    if (!q || sending || !active) return;
    setSending(true);
    setError(null);
    try {
      const reply: ChatReply = await api.chat({ sessionId: active.id, question: q });
      // 标题可能在首问后由后端自动更新（新对话 → 摘要）
      await loadSessions();
      const userMsg: LocalMsg = {
        role: "user",
        content: q,
        citations: [],
        model: null,
        tokensUsed: 0,
        createdAt: nowBeijing(),
      };
      const asstMsg: LocalMsg = {
        role: "assistant",
        content: reply.answer,
        citations: reply.citations,
        model: reply.model,
        tokensUsed: reply.tokensUsed,
        createdAt: nowBeijing(),
      };
      setMsgs((prev) => [...(prev ?? []), userMsg, asstMsg]);
      setInput("");
    } catch (e) {
      setError(errOf(e));
    } finally {
      setSending(false);
    }
  };

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [msgs, sending]);

  const disabled = sending;

  return (
    <Card className="flex h-[600px] flex-col overflow-hidden p-0">
      {!base.answerChannelId ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 text-center">
          <Sparkles size={30} className="text-faint" />
          <p className="text-sm font-medium text-sub">问答功能尚未就绪</p>
          <p className="max-w-md text-xs text-faint">
            绑定问答渠道（如 OpenAI / DeepSeek / Custom）后即可基于知识库内容进行带引用溯源的对话。
          </p>
          <Button onClick={onOpenBind}>
            <Link2 size={14} className="mr-1 inline" /> 去绑定问答渠道
          </Button>
        </div>
      ) : (
        <div className="flex min-h-0 flex-1">
          {/* 会话列表 */}
          <div className="flex w-56 shrink-0 flex-col border-r border-edge/30">
            <div className="flex items-center justify-between border-b border-edge/40 px-3 py-2">
              <p className="text-xs font-medium text-muted">会话</p>
              <div className="flex gap-1">
                <button
                  title="刷新会话"
                  className="rounded p-1 text-faint hover:bg-hover hover:text-sub"
                  onClick={() => void loadSessions()}
                >
                  <RefreshCw size={13} />
                </button>
                <button
                  title="新建会话"
                  className="rounded p-1 text-faint hover:bg-hover hover:text-sub"
                  onClick={() => void newSession()}
                >
                  <Plus size={14} />
                </button>
              </div>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto p-1.5">
              {!sessions && (
                <div className="flex items-center gap-2 px-2 py-4 text-xs text-faint">
                  <Loader size={13} className="animate-spin" /> 加载中…
                </div>
              )}
              {sessions?.map((s) => (
                <div
                  key={s.id}
                  className={`group flex cursor-pointer items-center gap-1 rounded-md px-2 py-1.5 ${
                    s.id === activeId ? "bg-accent-soft" : "hover:bg-hover"
                  }`}
                  onClick={() => void openSession(s.id)}
                >
                  <MessageSquare size={13} className="shrink-0 text-faint" />
                  <div className="min-w-0 flex-1">
                    <p className={`truncate text-xs ${s.id === activeId ? "font-medium text-accent" : "text-sub"}`}>{s.title}</p>
                    <p className="text-[10px] text-faint">{shortTime(s.updatedAt)}</p>
                  </div>
                  <button
                    title="删除会话"
                    className="hidden shrink-0 rounded p-0.5 text-faint hover:text-red-500 group-hover:block"
                    onClick={(e) => {
                      e.stopPropagation();
                      setConfirmDelSession(s);
                    }}
                  >
                    <Trash2 size={13} />
                  </button>
                </div>
              ))}
              {sessions?.length === 0 && (
                <p className="px-2 py-4 text-xs text-faint">暂无会话，点击右上角 + 新建</p>
              )}
            </div>
          </div>

          {/* 对话区 */}
          <div className="flex min-w-0 flex-1 flex-col">
            <div className="px-3 pt-3">
              <ErrBar text={error} />
            </div>
            {!active ? (
              <div className="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
                <Sparkles size={28} className="text-faint" />
                <p className="text-sm font-medium text-sub">开始一段基于知识库的对话</p>
                <p className="max-w-md text-xs text-faint">
                  回答仅依据检索到的引用材料生成并标注 [n]，点击引用可跳转到原分块核对。
                </p>
                <Button onClick={() => void newSession()}>
                  <MessageSquare size={14} className="mr-1 inline" /> 新建会话
                </Button>
              </div>
            ) : (
              <>
                <div className="flex items-center gap-2 border-b border-edge/40 px-4 py-2.5">
                  <p className="truncate text-sm font-semibold text-main">{active.title}</p>
                  <span className="ml-auto shrink-0 text-xs text-faint">
                    {base.answerModel ?? ""}
                  </span>
                </div>
                <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4">
                  {msgs === null && (
                    <div className="flex items-center gap-2 text-xs text-faint">
                      <Loader size={13} className="animate-spin" /> 加载消息…
                    </div>
                  )}
                  {msgs?.map((m, i) =>
                    m.role === "user" ? (
                      <div key={i} className="flex justify-end">
                        <div className="max-w-[78%] whitespace-pre-wrap break-words rounded-lg bg-indigo-600 px-3 py-2 text-sm text-white">
                          {m.content}
                        </div>
                      </div>
                    ) : (
                      <div key={i} className="flex justify-start">
                        <div className="max-w-[92%] rounded-xl bg-inset/80 px-3 py-2.5">
                          <div className="mb-1 flex items-center gap-1.5 text-xs text-faint">
                            <Sparkles size={12} className="text-accent" />
                            {m.model ?? "模型回答"}
                            <span className="ml-1">· {m.tokensUsed} tokens</span>
                          </div>
                          <p className="whitespace-pre-wrap break-words text-sm leading-relaxed text-main">{m.content}</p>
                          {m.citations.length > 0 && (
                            <div className="mt-2 flex flex-wrap gap-1.5 border-t border-edge/40 pt-2">
                              {m.citations.map((c) => (
                                <button
                                  key={`${c.n}-${c.chunkId}`}
                                  onClick={() => openChunk({ id: c.documentId, title: c.title }, c.chunkId)}
                                  className="rounded-md bg-accent-soft px-2 py-0.5 text-xs text-accent transition-colors hover:bg-accent hover:text-white"
                                  title={`跳转到分块 #${c.chunkId}${c.heading ? `（${c.heading}）` : ""}`}
                                >
                                  [{c.n}] {c.title}
                                  {c.heading ? ` · ${c.heading}` : ""}
                                </button>
                              ))}
                            </div>
                          )}
                        </div>
                      </div>
                    ),
                  )}
                  {sending && (
                    <div className="flex items-center gap-2 text-xs text-faint">
                      <Loader size={13} className="animate-spin" /> 正在检索知识库并生成回答…
                    </div>
                  )}
                  <div ref={bottomRef} />
                </div>
                <div className="border-t border-edge/40 p-3">
                  <div className="flex items-end gap-2">
                    <textarea
                      value={input}
                      onChange={(e) => setInput(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && !e.shiftKey) {
                          e.preventDefault();
                          void send();
                        }
                      }}
                      rows={2}
                      disabled={disabled}
                      className={`${inputCls} min-w-0 flex-1 resize-none`}
                      placeholder="输入问题，Enter 发送（Shift+Enter 换行）"
                    />
                    <div className="shrink-0">
                      <Button onClick={() => void send()} disabled={disabled || !input.trim()}>
                        {sending ? (
                          <Loader size={14} className="animate-spin" />
                        ) : (
                          <Send size={14} />
                        )}
                      </Button>
                    </div>
                  </div>
                </div>
              </>
            )}
          </div>
        </div>
      )}

      <ConfirmBox
        open={!!confirmDelSession}
        title="删除会话"
        text={
          <>
            删除会话 <span className="font-semibold text-main">{confirmDelSession?.title}</span> 将同时删除其中全部问答记录，不可恢复。
          </>
        }
        onClose={() => setConfirmDelSession(null)}
        onConfirm={() => void removeSession()}
      />
    </Card>
  );
}

// ---------------------------------------------------------------------------
// 评测 Tab：跑 kb/evals/*.json 用例集，看 recall@k / MRR / 拒答正确率
// ---------------------------------------------------------------------------

function EvalTab({ base }: { base: KnowledgeBase }) {
  const canVector = base.embeddingChannelId !== null && base.indexStatus === "ready";
  const [mode, setMode] = useState<KbSearchMode>(canVector ? "hybrid" : "keyword");
  const [topK, setTopK] = useState(4);
  const [casesPath, setCasesPath] = useState("");
  const [report, setReport] = useState<EvalReport | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setReport(null);
    setError(null);
    setMode(canVector ? "hybrid" : "keyword");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [base.id]);

  const pickFile = async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "评测用例 JSON", extensions: ["json"] }],
    });
    if (typeof picked === "string") {
      setCasesPath(picked);
      setReport(null);
    }
  };

  const run = async () => {
    if (!casesPath || running) return;
    setRunning(true);
    setError(null);
    try {
      setReport(await api.eval({ kbId: base.id, casesPath, mode, topK }));
    } catch (e) {
      setError(errOf(e));
      setReport(null);
    } finally {
      setRunning(false);
    }
  };

  const pct = (v: number) => `${(v * 100).toFixed(1)}%`;
  const fileName = casesPath.split(/[\\/]/).pop() ?? "";

  return (
    <div className="space-y-4">
      <Card>
        <p className="text-sm text-sub">
          用「问题 → 期望文档」用例集跑问答同一条检索链路（重排 + 阈值 + 邻居扩展），
          输出 recall@{topK} / MRR / 拒答正确率。改检索策略前后跑同一份用例集，用数字判断是否真的变好。
        </p>
        <div className="mt-3 flex flex-wrap gap-2">
          {MODES.map((m) => (
            <button
              key={m.key}
              onClick={() => {
                setMode(m.key);
                setReport(null);
              }}
              disabled={m.key !== "keyword" && !canVector}
              title={m.desc}
              className={`rounded-md px-3 py-1.5 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                mode === m.key ? "bg-card text-accent shadow-sm" : "text-muted hover:text-sub"
              }`}
            >
              {m.label}
            </button>
          ))}
          <label className="ml-auto flex items-center gap-1.5 text-xs text-muted">
            top-k
            <input
              type="number"
              min={1}
              max={20}
              value={topK}
              onChange={(e) => setTopK(Math.max(1, Math.min(20, Number(e.target.value) || 1)))}
              className={`${inputCls} w-20`}
            />
          </label>
        </div>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Button variant="secondary" onClick={() => void pickFile()}>
            <FileText size={14} className="mr-1 inline" /> 选择用例文件
          </Button>
          <span className="text-xs text-faint">示例：仓库 kb/evals/sample.json</span>
          {fileName && (
            <span className="max-w-[420px] truncate text-xs text-muted" title={casesPath}>
              {fileName}
            </span>
          )}
          <Button onClick={() => void run()} disabled={!casesPath || running}>
            {running ? <Loader size={14} className="mr-1 inline animate-spin" /> : null}
            运行评测
          </Button>
          {!canVector && (
            <span className="text-xs text-faint">未构建向量索引，仅关键词模式可跑</span>
          )}
        </div>
        <ErrBar text={error} />
      </Card>

      {report && (
        <>
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
            {[
              { label: `recall@${report.topK}`, value: pct(report.recallAtK), sub: `${report.answerable} 条可答用例` },
              { label: "MRR", value: report.mrr.toFixed(3), sub: "首个期望文档排名倒数均值" },
              { label: "拒答正确率", value: pct(report.refuseAccuracy), sub: "无答案问题检索为空" },
              { label: "模式 / 用例", value: report.mode, sub: `共 ${report.total} 条` },
            ].map((s) => (
              <Card key={s.label}>
                <div className="text-xs text-muted">{s.label}</div>
                <div className="mt-1 text-xl font-semibold text-main">{s.value}</div>
                <div className="mt-0.5 text-xs text-faint">{s.sub}</div>
              </Card>
            ))}
          </div>

          <Card className="p-0">
            <div className="border-b border-edge/40 px-4 py-2.5 text-sm font-medium text-main">
              逐条结果
            </div>
            <div className="divide-y divide-edge/40">
              {report.cases.map((c, i) => (
                <div key={i} className="flex items-start gap-3 px-4 py-2.5">
                  <span
                    className={`mt-0.5 shrink-0 rounded px-1.5 py-0.5 text-xs font-medium ${
                      c.passed
                        ? "bg-emerald-50 text-emerald-700 dark:bg-emerald-950 dark:text-emerald-300"
                        : "bg-red-50 text-red-700 dark:bg-red-950 dark:text-red-300"
                    }`}
                  >
                    {c.passed ? "通过" : "未通过"}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="text-sm text-main">{c.question}</div>
                    <div className="mt-0.5 text-xs text-faint">
                      期望：
                      {c.expectNoAnswer ? "无答案（检索应为空）" : c.expectDocs.join("、") || "—"}
                      {c.matchedRank !== null && ` · 命中第 ${c.matchedRank} 条`}
                    </div>
                    {c.hits.length > 0 && (
                      <div className="mt-1.5 flex flex-wrap gap-1.5">
                        {c.hits.map((h, j) => (
                          <span
                            key={j}
                            className="rounded bg-inset px-1.5 py-0.5 text-xs text-sub"
                            title={`融合分 ${h.score.toFixed(4)}${
                              h.vecScore !== null ? ` · 相似度 ${h.vecScore.toFixed(3)}` : ""
                            }${h.kwScore !== null ? ` · bm25 ${h.kwScore.toFixed(2)}` : ""}`}
                          >
                            {j + 1}. {h.title}
                            {h.vecScore !== null && ` · ${h.vecScore.toFixed(2)}`}
                          </span>
                        ))}
                      </div>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </Card>
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// 主页面：库列表 + Tab 内容
// ---------------------------------------------------------------------------

type TabKey = "docs" | "search" | "chat" | "eval";

const TABS: { key: TabKey; label: string }[] = [
  { key: "docs", label: "文档与索引" },
  { key: "search", label: "检索" },
  { key: "chat", label: "问答" },
  { key: "eval", label: "评测" },
];

export function KnowledgeBasePage() {
  const channels = useAppStore((s) => s.channels);
  const refreshChannels = useAppStore((s) => s.refreshChannels);
  const [bases, setBases] = useState<KnowledgeBase[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [tab, setTab] = useState<TabKey>("docs");
  const [error, setError] = useState<string | null>(null);

  // 库表单 / 绑定 / 删除
  const [formOpen, setFormOpen] = useState(false);
  const [editBase, setEditBase] = useState<KnowledgeBase | null>(null);
  const [bindOpen, setBindOpen] = useState(false);
  const [confirmDelBase, setConfirmDelBase] = useState<KnowledgeBase | null>(null);
  const [busyDelete, setBusyDelete] = useState(false);

  // 分块预览（文档 / 检索命中 / 引用跳转共用）
  const [chunkTarget, setChunkTarget] = useState<{ id: string; title: string } | null>(null);
  const [chunkFocus, setChunkFocus] = useState<number | null>(null);

  // 左侧库列表：可折叠（状态记忆到本地存储；折叠后用下拉选择库）
  const [sideOpen, setSideOpen] = useState<boolean>(() => localStorage.getItem("kb.sideOpen") !== "0");
  const toggleSide = () =>
    setSideOpen((v) => {
      const next = !v;
      localStorage.setItem("kb.sideOpen", next ? "1" : "0");
      return next;
    });

  const loadBases = useCallback(async () => {
    try {
      const bs = await api.listBases();
      setBases(bs);
      setSelectedId((cur) => (cur && bs.some((b) => b.id === cur) ? cur : (bs[0]?.id ?? null)));
      setError(null);
    } catch (e) {
      setError(errOf(e));
    }
  }, []);

  useEffect(() => {
    void loadBases();
    void refreshChannels();
  }, [loadBases, refreshChannels]);

  const selected = useMemo(() => bases?.find((b) => b.id === selectedId) ?? null, [bases, selectedId]);

  const onSavedBase = (b: KnowledgeBase) => {
    setBases((prev) =>
      prev
        ? prev.some((x) => x.id === b.id)
          ? prev.map((x) => (x.id === b.id ? b : x))
          : [b, ...prev] // 新建：插到列表头部并选中
        : [b],
    );
    setSelectedId(b.id);
  };

  const openChunk = (target: { id: string; title: string }, focus: number | null) => {
    setChunkTarget(target);
    setChunkFocus(focus);
  };

  const deleteBase = async () => {
    if (!confirmDelBase) return;
    setBusyDelete(true);
    try {
      await api.deleteBase(confirmDelBase.id);
      setConfirmDelBase(null);
      await loadBases();
    } catch (e) {
      setError(errOf(e));
      setConfirmDelBase(null);
    } finally {
      setBusyDelete(false);
    }
  };

  const openCreate = () => {
    setEditBase(null);
    setFormOpen(true);
  };
  const openEdit = (b: KnowledgeBase) => {
    setEditBase(b);
    setFormOpen(true);
  };

  return (
    <div>
      <PageHeader
        title="知识库"
        description="本地文档检索与 RAG 问答：导入 → 构建索引 → 检索 / 对话"
        actions={
          <>
            <Button variant="secondary" onClick={() => void loadBases()}>
              <RefreshCw size={14} className="mr-1 inline" /> 刷新
            </Button>
            <Button onClick={openCreate}>
              <Plus size={15} className="mr-1 inline" /> 新建知识库
            </Button>
          </>
        }
      />

      <ErrBar text={error} />

      {/* 列表开关：收起后主内容获得整行宽度；折叠态用下拉切换库 */}
      <div className="mb-3 flex items-center gap-2">
        <button
          onClick={toggleSide}
          title={sideOpen ? "收起知识库列表" : "展开知识库列表"}
          className="flex shrink-0 items-center gap-1.5 rounded-md bg-inset px-2.5 py-1.5 text-sm text-sub transition-colors hover:bg-hover"
        >
          {sideOpen ? <PanelLeftClose size={15} /> : <PanelLeftOpen size={15} />}
          <span className="hidden md:inline">{sideOpen ? "收起列表" : "展开列表"}</span>
        </button>
        {!sideOpen && bases && bases.length > 0 && (
          <select
            value={selectedId ?? ""}
            onChange={(e) => setSelectedId(e.target.value || null)}
            className={`${inputCls} max-w-80 shrink-0`}
            title="切换知识库"
          >
            {bases.map((b) => (
              <option key={b.id} value={b.id}>
                {b.name}
                {b.status === "disabled" ? "（已停用）" : ""}
              </option>
            ))}
          </select>
        )}
      </div>

      <div className="flex items-start gap-4">
        {/* 左侧库列表（可折叠） */}
        {sideOpen && (
          <div className="w-60 shrink-0 space-y-2">
          {bases === null && (
            <Card>
              <div className="flex items-center gap-2 py-4 text-sm text-faint">
                <Loader size={15} className="animate-spin" /> 加载知识库…
              </div>
            </Card>
          )}
          {bases?.map((b) => (
            <button
              key={b.id}
              onClick={() => setSelectedId(b.id)}
              className={`block w-full rounded-lg border p-3 text-left transition-colors ${
                b.id === selectedId
                  ? "border-indigo-400 bg-accent-soft ring-1 ring-indigo-300 dark:ring-indigo-800"
                  : "border-edge bg-card hover:bg-hover"
              }`}
            >
              <div className="flex items-center justify-between gap-2">
                <p className="truncate text-sm font-semibold text-main">{b.name}</p>
                <IndexBadge status={b.indexStatus} />
              </div>
              <p className="mt-0.5 truncate text-xs text-faint" title={b.description || undefined}>
                {b.description || "暂无描述"}
              </p>
              <p className="mt-1 text-xs text-muted">
                {b.docCount} 篇文档 · {b.chunkSize} 字/块
                {b.status === "disabled" && " · 已停用"}
              </p>
              {(b.embeddingModel || b.answerModel) && (
                <p className="mt-1 truncate text-[11px] text-accent" title="已绑定渠道">
                  {[b.embeddingModel && `语义 ${b.embeddingModel}`, b.answerModel && `问答 ${b.answerModel}`]
                    .filter(Boolean)
                    .join(" ｜ ")}
                </p>
              )}
            </button>
          ))}
          {bases?.length === 0 && (
            <Card>
              <div className="py-6 text-center">
                <Database size={26} className="mx-auto text-faint" />
                <p className="mt-2 text-sm font-medium text-sub">还没有知识库</p>
                <p className="mt-1 text-xs text-faint">点击右上角「新建知识库」开始</p>
              </div>
            </Card>
          )}
          </div>
        )}

        {/* 右侧内容 */}
        <div className="min-w-0 flex-1">
          {!selected ? (
            <Card>
              <div className="flex flex-col items-center gap-2 py-16 text-center">
                <Database size={34} className="text-faint" />
                <p className="text-sm font-medium text-sub">
                  {bases?.length ? (sideOpen ? "请在左侧选择知识库" : "请从上方展开列表或下拉选择知识库") : "先新建一个知识库"}
                </p>
                <p className="max-w-md text-xs leading-relaxed text-faint">
                  使用流程：新建知识库 → 绑定语义检索渠道（embedding，可选）→ 导入 .md / .txt 文档 →
                  构建向量索引（可选）→ 「检索」页验证召回 → 「问答」页与知识库对话。
                </p>
                {!bases?.length && (
                  <Button onClick={openCreate} className="mt-2">
                    <Plus size={15} className="mr-1 inline" /> 新建知识库
                  </Button>
                )}
              </div>
            </Card>
          ) : (
            <>
              {/* 库头部 */}
              <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <h2 className="truncate text-lg font-bold text-main">{selected.name}</h2>
                    <BaseStatusBadge status={selected.status} />
                    <IndexBadge status={selected.indexStatus} />
                  </div>
                  <p className="mt-0.5 text-xs text-muted">
                    {selected.description || "暂无描述"}
                    {selected.embeddingModel && (
                      <span className="ml-2 text-faint">语义检索：{selected.embeddingModel}（{selected.embeddingDim} 维）</span>
                    )}
                    {selected.answerModel && <span className="ml-2 text-faint">问答：{selected.answerModel}</span>}
                  </p>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <Button variant="secondary" onClick={() => setBindOpen(true)}>
                    <Link2 size={14} className="mr-1 inline" /> 绑定渠道
                  </Button>
                  <Button variant="secondary" onClick={() => openEdit(selected)}>
                    <Settings2 size={14} className="mr-1 inline" /> 设置
                  </Button>
                  <Button variant="danger" onClick={() => setConfirmDelBase(selected)}>
                    <Trash2 size={14} className="mr-1 inline" /> 删除
                  </Button>
                </div>
              </div>

              {/* Tab 切换 */}
              <div className="mb-4 inline-flex rounded-lg bg-inset p-1">
                {TABS.map((t) => (
                  <button
                    key={t.key}
                    onClick={() => setTab(t.key)}
                    className={`rounded-md px-4 py-1.5 text-sm font-medium transition-colors ${
                      tab === t.key ? "bg-accent-soft text-accent" : "text-muted hover:text-main"
                    }`}
                  >
                    {t.label}
                  </button>
                ))}
              </div>

              {/* 三个 Tab 常驻挂载（hidden 隐藏），切换不丢状态 */}
              <div className={tab === "docs" ? "" : "hidden"}>
                <DocsTab
                  base={selected}
                  active={tab === "docs"}
                  onBaseRefresh={() => void loadBases()}
                  openChunk={openChunk}
                  onOpenBind={() => setBindOpen(true)}
                />
              </div>
              <div className={tab === "search" ? "" : "hidden"}>
                <SearchTab base={selected} openChunk={openChunk} />
              </div>
              <div className={tab === "chat" ? "" : "hidden"}>
                <ChatTab base={selected} onOpenBind={() => setBindOpen(true)} openChunk={openChunk} />
              </div>
              <div className={tab === "eval" ? "" : "hidden"}>
                <EvalTab base={selected} />
              </div>
            </>
          )}
        </div>
      </div>

      {/* 弹窗 */}
      <BaseFormModal open={formOpen} initial={editBase} onClose={() => setFormOpen(false)} onSaved={onSavedBase} />
      <BindModal base={selected} channels={channels} open={bindOpen} onClose={() => setBindOpen(false)} onSaved={onSavedBase} />
      <ChunkPreviewModal target={chunkTarget} focusChunkId={chunkFocus} onClose={() => setChunkTarget(null)} />
      <ConfirmBox
        open={!!confirmDelBase}
        title="删除知识库"
        busy={busyDelete}
        text={
          <>
            删除知识库 <span className="font-semibold text-main">{confirmDelBase?.name}</span> 将级联删除全部文档、
            分块、索引与会话记录，不可恢复。
          </>
        }
        onClose={() => setConfirmDelBase(null)}
        onConfirm={() => void deleteBase()}
      />
    </div>
  );
}
