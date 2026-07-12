// spoor.answer-trace.v1
// 唯一契约:matcher(后端) → viewer(前端)。一轮问答 = 一个 AnswerTrace。
// JSON Schema 见同目录 answer-trace.schema.json;Python 端见 python/answer_trace.py。

export const SCHEMA_VERSION = 'spoor.answer-trace.v1' as const

export type Verdict = 'supported' | 'partial' | 'unsupported'

/** 一轮问答的完整溯源结果。 */
export interface AnswerTrace {
  schema: typeof SCHEMA_VERSION
  question: string
  /** 答案:普通文字与内联 claim 交替;拼起来即完整答案。 */
  answer: AnswerPart[]
  /** 证据,被 claim 以 id 引用。 */
  evidence: Evidence[]
  /** 本次核验所依据的 spoor 产物(供「定位原文」下钻)。 */
  source: SourceRef
  /** 审计轨迹:谁生成、谁判定、何时、用哪次解析(可复现 + 留痕)。 */
  audit: AuditInfo
}

export type AnswerPart =
  | { type: 'text'; text: string }
  | { type: 'claim'; text: string; verdict: Verdict; evidenceIds: string[] }

export type Evidence = QuoteEvidence | CellEvidence | NoEvidence

interface EvidenceBase {
  id: string
  verdict: Verdict
  page: number | null
  /** 复核说明:为何 partial / 为何 unsupported。 */
  note?: string
}

/** 命中在原始文档里的来源锚点:源页码,born-digital PDF 另附近似坐标框
 *  (PDF 原生用户空间,y 向上,与 /MediaBox 同系;可直接交 PDF.js 画高亮)。 */
export interface EvidenceAnchor {
  /** 命中所在文档在上传语料中的下标(0-based);单文档语料为 0。 */
  doc?: number
  page: number
  bbox?: { x0: number; y0: number; x1: number; y1: number }
}

/** 引文型:后端已从 markdown 切好,前端直接渲染。 */
export interface QuoteEvidence extends EvidenceBase {
  kind: 'quote'
  before: string
  hit: string
  after: string
  /** 命中在 source markdown 的字符区间;离线渲染原文时定位用,可缺省。 */
  span?: { start: number; end: number }
  /** 命中在原始 PDF 里的位置(块级 provenance 命中时才有)。 */
  anchor?: EvidenceAnchor
}

/** 表格型:数字带「表·行·列」,一眼看出口径。 */
export interface CellEvidence extends EvidenceBase {
  kind: 'cell'
  table: string
  row: string
  column: string
  value: string
}

/** 无法核验:原文找不到,给原因 + 真值。 */
export interface NoEvidence extends EvidenceBase {
  kind: 'none'
  verdict: 'unsupported'
  reason: string
  expectedTruth?: string
}

export interface SourceRef {
  documentId: string
  title: string
  pages?: number
}

export interface AuditInfo {
  /** 哪次解析产出 source,如 "spoor@0.8.18"。 */
  parser: string
  /** 生成答案的模型(Cloudflare Workers AI id),如 "@cf/google/gemma-4-26b-a4b-it"。 */
  generator: string
  /** 判定器(钉死版本),如 "rules" / "@cf/qwen/qwen3-30b-a3b-fp8"。 */
  judge: string
  /** ISO 时间戳。 */
  judgedAt: string
}

// --- 原文文档(下钻视图的数据,由 documentId 关联) -------------------------
// 与 AnswerTrace 分开:AnswerTrace 与渲染无关;SourceDocument 是原文的一种可渲染
// 形态,run/cell 上用 refId 指回它支撑的 evidence,供「定位原文」滚动高亮。

export interface SourceDocument {
  documentId: string
  title: string
  pages: SourcePage[]
}

export interface SourcePage {
  number: number
  title?: string
  blocks: SourceBlock[]
}

export type SourceBlock =
  | { type: 'paragraph'; runs: TextRun[] }
  | { type: 'table'; name: string; columns: string[]; rows: TableRow[] }
  | { type: 'note'; text: string }

export interface TextRun {
  text: string
  /** 命中某条 evidence 时,指回它的 id;前端据此滚动 + 闪烁高亮。 */
  refId?: string
  status?: Verdict
}

export interface TableRow {
  label: string
  cells: TableCell[]
}

export interface TableCell {
  value: string
  refId?: string
  status?: Verdict
}

/** 当前依据文档的元信息 + 整篇 markdown(供前端渲染原文、下钻定位)。 */
export interface SourceMeta {
  documentId: string
  title: string
  pages: number
  markdown: string
  /** 原文 token 估算(tiktoken)。 */
  tokens: number
  /** 模型上下文上限(token),用于"是否超限"提示。 */
  contextLimit: number
}

/** api `/api/demo` 的返回:一段对话 + 它依据的原文。 */
export interface DemoPayload {
  source: SourceMeta
  traces: AnswerTrace[]
}

/** api `/api/upload` 的返回:解析结果 + 新的当前语料。 */
export interface UploadResult {
  files: { name: string; ok: boolean; chars?: number; error?: string }[]
  source: { documentId: string; title: string; pages: number }
  markdown: string
  /** 原文 token 估算(tiktoken)。 */
  tokens: number
  /** 模型上下文上限(token),用于"是否超限"提示。 */
  contextLimit: number
  /** 本次上传语料的句柄;边缘后端用 KV 存语料,前端在后续 /ask、/media 带回它。
   *  本地内存态后端(apps/api)不返回此字段,前端缺省即回退到服务端当前语料。 */
  corpusId?: string
}
