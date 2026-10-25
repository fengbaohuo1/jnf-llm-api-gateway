// ---- Shared domain types for the LLM API Gateway UI ----

export type ChannelType = "OpenAI" | "DeepSeek" | "Claude" | "Gemini" | "Custom";

/** 渠道显式协议；空串表示「自动」——按渠道类型推导 */
export type ChannelProtocol = "openai" | "deepseek" | "anthropic" | "gemini";

export type ChannelStatus = "active" | "disabled" | "error";

export interface Channel {
  id: string;
  name: string;
  type: ChannelType;
  /** 显式上游协议；"" = 按渠道类型自动推导 */
  protocol: ChannelProtocol | "";
  baseUrl: string;
  model: string;
  status: ChannelStatus;
  apiKeyMasked: string;
  latencyMs: number | null;
  priority: number; // 故障转移优先级，越大越先
  weight: number; // 同优先级权重，越大越可能先被选中
  // 单价存储单位为"元/1k tokens"（转发层记账用），界面以"元/M tokens"输入与展示
  pricePer1kTokens: number; // 每 1k token 成本（元）
  cachePricePer1kTokens: number; // 每 1k token 缓存命中成本（元），0 = 不区分缓存
  reasoningPricePer1kTokens: number; // 每 1k token 输出（thinking/reasoning）专用成本（元），0 = 不区分；界面以"输出单价"展示
  createdAt: string;
}

export interface ApiKey {
  id: string;
  name: string;
  key: string; // sk-llmapi-...（列表展示时为 key_prefix 开头片段）
  status: "active" | "disabled";
  quotaLimit: number; // tokens allowed (-1 = unlimited)
  usedTokens: number;
  createdAt: string;
  expiresAt: string | null;
}

export interface RequestLog {
  id: string;
  timestamp: string;
  channel: string;
  model: string;
  key: string;
  /** 客户端入口协议：openai / anthropic / responses */
  protocol: string;
  /** 是否流式请求 */
  stream: boolean;
  /** 客户端 User-Agent */
  clientUa: string;
  /** 第几次尝试（1 = 主渠道一次成功） */
  attempts: number;
  /** 请求命中的最高安全风险；none = 无命中（安全扫描关闭时也为 none） */
  riskLevel: "none" | "low" | "medium" | "high";
  status: number;
  latencyMs: number;
  promptTokens: number; // 输入 token 数（缓存命中率的分母）
  tokens: number;
  cacheHitTokens: number; // 输入中命中前缀缓存的 token 数
  reasoningTokens: number; // 输出中用于思考（reasoning）的 token 数
  cost: number; // 成本（元）
}

/** 请求内容明细（按需加载；开启内容记录时有值） */
export interface RequestDetail {
  requestId: string;
  /** 收到的原始请求体（客户端协议原文，脱敏后） */
  requestBody: string | null;
  /** 实际发给上游的请求体（内部 OpenAI 格式） */
  upstreamBody: string | null;
  /** 成功响应体（非流式原文 / 流式重建完整响应） */
  responseBody: string | null;
  /** 失败时的错误响应体 */
  errorBody: string | null;
}

export interface SecurityLog {
  id: string;
  requestId: string;
  stage: "request" | "response";
  ruleId: string;
  category: string;
  risk: "low" | "medium" | "high";
  snippetMasked: string;
  action: "detected" | "masked" | "blocked";
  createdAt: string;
}

export interface SecurityRule {
  id: string;
  category: string;
  risk: "low" | "medium" | "high";
  pattern: string;
  matchType: "regex" | "substring";
  enabled: boolean;
  action: "record" | "mask" | "block";
  createdAt: string;
}

export interface AuthEvent {
  id: string;
  keyId: string | null;
  success: boolean;
  reason: string | null;
  createdAt: string;
}

export interface AdminAudit {
  id: string;
  action: string;
  targetType: string;
  targetId: string | null;
  detail: string | null;
  createdAt: string;
}

/** 扩展服务运行时状态（list_services 命令返回） */
export type ServiceStatus = "running" | "failed" | "disabled";

export interface ServiceInfo {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  status: ServiceStatus;
  error: string | null;
}

// ---- Knowledge base（本地知识库：检索与 RAG 问答）----

export type KbStatus = "active" | "disabled";
/** 向量索引状态机：none（未构建）/ building / ready / failed */
export type KbIndexStatus = "none" | "building" | "ready" | "failed";

export interface KnowledgeBase {
  id: string;
  name: string;
  description: string;
  chunkSize: number;
  chunkOverlap: number;
  embeddingDim: number;
  indexStatus: KbIndexStatus;
  status: KbStatus;
  /** 绑定 embedding 渠道/模型（语义检索用；未启用时为空） */
  embeddingModel: string | null;
  embeddingChannelId: string | null;
  /** 绑定问答渠道/模型（RAG 回答生成用，与 embedding 解耦） */
  answerChannelId: string | null;
  answerModel: string | null;
  createdAt: string;
  updatedAt: string;
  docCount: number;
}

export type KbDocStatus = "importing" | "ready" | "failed";

export interface KbDocument {
  id: string;
  kbId: string;
  title: string;
  format: string; // md / txt
  /** 源文件字节大小（粘贴导入为 0） */
  fileSize: number;
  sourceType: string; // paste / upload / ...
  sourceUrl: string | null;
  sourcePath: string | null;
  docMeta: string;
  fileHash: string;
  charCount: number;
  chunkCount: number;
  status: KbDocStatus;
  error: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface KbChunk {
  id: number;
  documentId: string;
  seq: number;
  /** 章节溯源路径（md 解析：如 "A / B"；txt 为空） */
  heading: string;
  content: string;
  chars: number;
  metadata: string;
}

export type KbTaskStatus = "pending" | "running" | "ready" | "failed";

export interface KbTask {
  id: string;
  kbId: string;
  taskType: string;
  status: KbTaskStatus;
  totalItems: number;
  doneItems: number;
  progress: number;
  errorMessage: string | null;
  createdAt: string;
  completedAt: string | null;
}

/** 统一检索命中（keyword bm25 / vector 相似度 / hybrid RRF 融合） */
export interface SearchHit {
  chunkId: number;
  documentId: string;
  title: string;
  heading: string;
  seq: number;
  content: string;
  /** 当前模式的排序分（keyword 为 bm25 负值 / LIKE 负命中数；vector 余弦；hybrid 融合分） */
  score: number;
  /** 向量路原始余弦相似度（hybrid 且被向量路命中时有值） */
  vecScore: number | null;
  /** 关键词路原始分（bm25 或负命中数；两路皆中取更负者） */
  kwScore: number | null;
}

export type KbSearchMode = "keyword" | "vector" | "hybrid";

// ---- 检索评测 ----

/** 评测用例（kb/evals/*.json 数组元素） */
export interface EvalCase {
  question: string;
  /** 期望命中的文档标题（任一命中即算召回） */
  expectDocs?: string[];
  /** 期望检索为空（无答案问题） */
  expectNoAnswer?: boolean;
}

export interface EvalHit {
  title: string;
  score: number;
  vecScore: number | null;
  kwScore: number | null;
}

export interface EvalCaseResult {
  question: string;
  expectDocs: string[];
  expectNoAnswer: boolean;
  matchedRank: number | null;
  passed: boolean;
  hits: EvalHit[];
}

export interface EvalReport {
  mode: KbSearchMode;
  topK: number;
  total: number;
  answerable: number;
  recallAtK: number;
  mrr: number;
  refuseAccuracy: number;
  cases: EvalCaseResult[];
}

// ---- MCP 服务 ----

/** 工具风险类别 */
export type McpToolKind = "read" | "write" | "dangerous";

export interface McpParamInfo {
  name: string;
  type: string;
  required: boolean;
  description: string;
  enumValues: string[];
}

export interface McpToolInfo {
  name: string;
  description: string;
  kind: McpToolKind;
  kindLabel: string;
  /** 会产生外部费用（问答 / 建索引） */
  costly: boolean;
  /** 是否记入管理审计 */
  audit: boolean;
  required: string[];
  params: McpParamInfo[];
  schema: unknown;
  enabled: boolean;
}

export interface McpStatus {
  enabled: boolean;
  running: boolean;
  host: string;
  configuredPort: number;
  port: number;
  endpoint: string;
  tokenSet: boolean;
  tokenMasked: string | null;
  readOnly: boolean;
  toolTotal: number;
  toolEnabled: number;
  error: string | null;
}

export interface McpCall {
  id: number;
  method: string;
  tool: string | null;
  ok: number;
  error: string | null;
  durationMs: number;
  client: string | null;
  createdAt: string;
}

export interface McpCallResult {
  ok: boolean;
  result: string | null;
  error: string | null;
  durationMs: number;
}

export interface KbSession {
  id: string;
  kbId: string;
  title: string;
  createdAt: string;
  updatedAt: string;
}

/** 引用条目（正文 [n] ↔ 来源一一对应，可跳转到原分块） */
export interface KbCitation {
  n: number;
  chunkId: number;
  documentId: string;
  title: string;
  heading: string;
}

export interface KbMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  sources: KbCitation[];
  model: string | null;
  tokensUsed: number;
  createdAt: string;
}

export interface ChatReply {
  sessionId: string;
  answer: string;
  citations: KbCitation[];
  model: string;
  tokensUsed: number;
  latencyMs: number;
}
