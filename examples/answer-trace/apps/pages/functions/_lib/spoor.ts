// spoor-wasm 初始化 + 类型化封装。Pages Functions bundler 原生支持 .wasm import。
import wasmModule from "@harrisonwang/spoor-wasm/spoor_wasm_bg.wasm";
import * as bindings from "@harrisonwang/spoor-wasm/spoor_wasm_bg.js";

const instance = new WebAssembly.Instance(wasmModule, {
  "./spoor_wasm_bg.js": bindings,
});
bindings.__wbg_set_wasm(instance.exports as WebAssembly.Exports);

export interface TableEntry {
  source: string;
  format: string;
  sheet?: string;
  title?: string;
  headers: Record<string, { column_index: number }>;
  rows: Record<string, string>[];
  [k: string]: unknown;
}

/** spoor 的来源定位 span:output 是 markdown 的 UTF-8 字节区间,source 为
 *  源页码 + born-digital 行的近似坐标框(PDF 原生用户空间,y 向上)。 */
export interface ProvenanceSpan {
  output: { start: number; end: number };
  source: { kind: "page"; number: number; bbox?: { x0: number; y0: number; x1: number; y1: number } };
}

export interface ParseResult {
  content:
    | { kind: "document"; value: { source: string; format: string; markdown: string } }
    | { kind: "tables"; value: { tables: TableEntry[]; serialized_bytes: number } };
  warnings: { code: string; message: string; location?: { kind: string; number: number } }[];
  stats: { input_bytes: number; output_bytes: number; format: string; page_count?: number };
  provenance?: { spans: ProvenanceSpan[] };
}

export function parseBytes(
  bytes: Uint8Array,
  sourceName?: string,
  maxParseBytes?: number,
  provenance?: "page" | "block",
): ParseResult {
  return bindings.parse_bytes(
    bytes,
    sourceName,
    undefined,
    undefined,
    maxParseBytes,
    undefined,
    undefined,
    undefined,
    undefined,
    undefined,
    undefined,
    undefined,
    provenance,
  ) as ParseResult;
}

export function extractMedia(
  bytes: Uint8Array,
  uri: string,
  sourceName?: string,
  maxParseBytes?: number,
): Uint8Array {
  return bindings.extract_media(bytes, uri, sourceName, undefined, undefined, maxParseBytes);
}
