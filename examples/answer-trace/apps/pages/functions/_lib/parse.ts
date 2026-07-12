// 上传解析 + 图片类型判定（spoor-wasm）。

import { parseBytes, type ProvenanceSpan, type TableEntry } from "./spoor";

export const MAX_REQUEST_BYTES = 16 * 1024 * 1024;

export interface ParsedDoc {
  markdown: string;
  /** 块级来源定位(spoor --provenance block);表格型无。 */
  provenance?: ProvenanceSpan[];
}

/** 解析并顺带要块级来源定位:文档命中的证据能锚回源页坐标。 */
export function parseDoc(name: string, data: Uint8Array): ParsedDoc {
  const res = parseBytes(data, name, MAX_REQUEST_BYTES, "block");
  if (res.content.kind === "document") {
    return { markdown: res.content.value.markdown, provenance: res.provenance?.spans };
  }
  return { markdown: tablesToMarkdown(res.content.value.tables) };
}


// 表格类（CSV/XLSX）渲染成 markdown 表格，让定位器一视同仁地在文本里找证据。
function tablesToMarkdown(tables: TableEntry[]): string {
  const out: string[] = [];
  for (const t of tables) {
    const name = (t.sheet as string) || (t.title as string) || (t.source as string) || "表";
    const headersMeta = t.headers ?? {};
    const headers = Object.keys(headersMeta).sort(
      (a, b) => headersMeta[a].column_index - headersMeta[b].column_index,
    );
    out.push(`## ${name}\n`);
    if (headers.length) {
      out.push("| " + headers.join(" | ") + " |");
      out.push("| " + headers.map(() => "---").join(" | ") + " |");
      for (const row of t.rows ?? []) {
        out.push("| " + headers.map((h) => String(row[h] ?? "")).join(" | ") + " |");
      }
    }
    out.push("");
  }
  return out.join("\n");
}

// 按魔数判定图片类型，无需 URI 后缀（PDF 整页图是 SVG、内嵌图可能 PNG/JPEG）。
export function contentType(raw: Uint8Array): string {
  if (raw[0] === 0x89 && raw[1] === 0x50 && raw[2] === 0x4e && raw[3] === 0x47) return "image/png";
  if (raw[0] === 0xff && raw[1] === 0xd8 && raw[2] === 0xff) return "image/jpeg";
  if (raw[0] === 0x47 && raw[1] === 0x49 && raw[2] === 0x46) return "image/gif";
  if (
    raw[0] === 0x52 && raw[1] === 0x49 && raw[2] === 0x46 && raw[3] === 0x46 &&
    raw[8] === 0x57 && raw[9] === 0x45 && raw[10] === 0x42 && raw[11] === 0x50
  ) {
    return "image/webp";
  }
  const head = new TextDecoder().decode(raw.subarray(0, 64)).trimStart().toLowerCase();
  if (head.startsWith("<?xml") || head.startsWith("<svg")) return "image/svg+xml";
  return "application/octet-stream";
}
