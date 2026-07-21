// spoor 能力的共享实现：native 工具与 MCP server 都调它，保证两条路结果一致。
// 用 Node 原生 binding @harrisonwang/spoor（同进程、零子进程）。

import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { extractMedia, parseBytes } from "@harrisonwang/spoor";
import { assertObject, assertString, optNum, optStr, pair, safeResolve, strArr } from "./util/validate.js";

/** 单个工具结果给模型的字节上限。超了就截断并提示用 narrowing 收窄——本身就是教学点。 */
const MAX_BODY_BYTES = 96 * 1024;

export interface ReadDocOptions {
  pages?: [number, number];
  sheet?: string;
  rows?: [number, number];
  columns?: string[];
  limit?: number;
  offset?: number;
  provenance?: "page";
}

interface SpoorWarning {
  code: string;
  message: string;
  location?: { kind: string; number: number };
}
interface SpoorResult {
  content:
    | { kind: "document"; value: { markdown: string } }
    | { kind: "tables"; value: { tables: unknown[] } };
  warnings: SpoorWarning[];
  stats: { format: string; output_bytes: number; page_count?: number };
  provenance?: { spans: { output: { start: number; end: number }; source: { kind: string; number: number } }[] };
}

/** 读取文档 → 返回 LLM 可直接消费的文本（含 warnings 与元信息，体现 spoor 的自描述输出）。 */
export async function readDocument(relPath: string, opts: ReadDocOptions = {}): Promise<string> {
  const abs = safeResolve(relPath);
  const buf = await readFile(abs);
  const result = parseBytes(buf, {
    sourceName: relPath,
    pages: opts.pages,
    sheet: opts.sheet,
    rows: opts.rows,
    columns: opts.columns,
    limit: opts.limit,
    offset: opts.offset,
    provenance: opts.provenance,
  }) as SpoorResult;

  return formatResult(relPath, result);
}

function formatResult(relPath: string, result: SpoorResult): string {
  let body: string;
  if (result.content.kind === "document") {
    body = result.content.value.markdown;
  } else {
    body = JSON.stringify(result.content.value.tables, null, 2);
  }

  let truncated = false;
  if (Buffer.byteLength(body, "utf8") > MAX_BODY_BYTES) {
    body = Buffer.from(body, "utf8").subarray(0, MAX_BODY_BYTES).toString("utf8");
    truncated = true;
  }

  const parts: string[] = [body.trimEnd()];

  if (truncated) {
    parts.push(
      "\n> ⚠ 输出过长已截断。用 pages / rows / columns / limit 收窄再读（这正是 spoor 的分页与收窄能力）。",
    );
  }

  if (result.warnings.length > 0) {
    const lines = result.warnings.map((w) => {
      const loc = w.location ? ` @${w.location.kind}${w.location.number}` : "";
      return `- ${w.code}${loc}: ${w.message}`;
    });
    parts.push(`\n⚠ 完整性 warnings（请如实转达用户）：\n${lines.join("\n")}`);
  }

  const s = result.stats;
  const pageInfo = s.page_count != null ? ` · 总页数=${s.page_count}` : "";
  parts.push(`\n〔meta〕来源=${relPath} · 格式=${s.format} · 输出字节=${s.output_bytes}${pageInfo}`);

  if (result.provenance && result.provenance.spans.length > 0) {
    const spans = result.provenance.spans
      .slice(0, 12)
      .map((sp) => `p${sp.source.number}:[${sp.output.start},${sp.output.end})`)
      .join(" ");
    parts.push(`〔provenance〕输出字节区间→源页：${spans}`);
  }

  return parts.join("\n");
}

/** 提取内嵌媒体（spoor:// 占位符）→ 存到 .spoor-media/，供交给 VLM。 */
export async function extractDocumentImage(relPath: string, uri: string): Promise<string> {
  const abs = safeResolve(relPath);
  const buf = await readFile(abs);
  const bytes = extractMedia(buf, uri, { sourceName: relPath });

  const outDir = safeResolve(".spoor-media");
  await mkdir(outDir, { recursive: true });
  const base = uri.replace(/[^a-zA-Z0-9._-]/g, "_").replace(/^_+/, "").slice(-48);
  const name = `${base}${guessExt(bytes)}`;
  await writeFile(path.join(outDir, name), bytes);

  return `已提取内嵌资源 → .spoor-media/${name}（${contentType(bytes)}, ${bytes.length} bytes）。可交给外部 VLM 处理。`;
}

// —— 按魔数判定类型，无需扩展名（与 spoor 各入口一致）——
function contentType(b: Uint8Array): string {
  if (b[0] === 0x89 && b[1] === 0x50 && b[2] === 0x4e && b[3] === 0x47) return "image/png";
  if (b[0] === 0xff && b[1] === 0xd8 && b[2] === 0xff) return "image/jpeg";
  if (b[0] === 0x47 && b[1] === 0x49 && b[2] === 0x46) return "image/gif";
  if (b[0] === 0x52 && b[1] === 0x49 && b[2] === 0x46 && b[8] === 0x57 && b[9] === 0x45 && b[10] === 0x42 && b[11] === 0x50)
    return "image/webp";
  const head = new TextDecoder().decode(b.subarray(0, 64)).trimStart().toLowerCase();
  if (head.startsWith("<?xml") || head.startsWith("<svg")) return "image/svg+xml";
  return "application/octet-stream";
}

function guessExt(b: Uint8Array): string {
  const ct = contentType(b);
  return ({ "image/png": ".png", "image/jpeg": ".jpg", "image/gif": ".gif", "image/webp": ".webp", "image/svg+xml": ".svg" } as Record<string, string>)[ct] ?? ".bin";
}

// —— 两个 spoor 工具的单一真相：schema + 派发。native 工具与 MCP server 都引用它，
//    保证「同一能力、两条接入路」行为完全一致。——
export interface SpoorToolDef {
  name: string;
  description: string;
  inputSchema: { type: "object"; properties: Record<string, unknown>; required?: string[] };
}

export const SPOOR_TOOLS: SpoorToolDef[] = [
  {
    name: "read_document",
    description:
      "读取 PDF/DOCX/XLSX/CSV/PPTX/EPUB/HTML 等文档，返回 LLM 可直接消费的文本（文档→Markdown，表格→JSON），并附完整性 warnings 与元信息。纯文本/代码文件请用 read_file。",
    inputSchema: {
      type: "object",
      properties: {
        path: { type: "string", description: "项目内文档路径，如 data/byd.pdf" },
        pages: { type: "array", items: { type: "number" }, description: "[起,止] 1-based 闭区间，PDF 按页、PPTX 按幻灯片，只解析这些页" },
        sheet: { type: "string", description: "XLSX 工作表名" },
        rows: { type: "array", items: { type: "number" }, description: "[起,止] 1-based 行区间；与 limit/offset 互斥" },
        columns: { type: "array", items: { type: "string" }, description: "只保留这些列名" },
        limit: { type: "number", description: "表格最多返回行数（默认 100）" },
        offset: { type: "number", description: "跳过前 N 行" },
        provenance: { type: "string", enum: ["page"], description: "返回页级出处映射，便于把引用锚回原文" },
      },
      required: ["path"],
    },
  },
  {
    name: "extract_document_image",
    description:
      "提取文档里的内嵌媒体（read_document 结果中出现的 spoor:// 占位符），存到 .spoor-media/ 供交给 VLM。",
    inputSchema: {
      type: "object",
      properties: {
        path: { type: "string", description: "项目内文档路径" },
        uri: { type: "string", description: "read_document 结果里的 spoor:// 占位符" },
      },
      required: ["path", "uri"],
    },
  },
];

/** 按名字派发一次 spoor 工具调用（native / mcp 共用）。 */
export async function runSpoorTool(name: string, args: unknown): Promise<string> {
  assertObject(args);
  if (name === "read_document") {
    return readDocument(assertString(args.path, "path"), {
      pages: pair(args.pages),
      sheet: optStr(args.sheet),
      rows: pair(args.rows),
      columns: strArr(args.columns),
      limit: optNum(args.limit),
      offset: optNum(args.offset),
      provenance: args.provenance === "page" ? "page" : undefined,
    });
  }
  if (name === "extract_document_image") {
    return extractDocumentImage(assertString(args.path, "path"), assertString(args.uri, "uri"));
  }
  throw new Error(`未知的 spoor 工具: ${name}`);
}
