// 确定性证据定位 —— 反幻觉核心。judge 给的 quote 必须在真实 spoor 产物里定位到，
// 否则上层降级为「无法核验」。分四档，逐档放宽，全部是确定性代码：
//   ① 精确子串；② 忽略全部空白；③ 表格单元格锚点（字符串数字）；
//   ④ 金档：数值/单位归一（同一数值、不同单位/格式，如 7771亿 = 777102百万）。
// 区间以 UTF-16 下标计（与前端 markdown.slice 一致）。

const PAGE = /##\s*Page\s+(\d+)/g;
const NUM = /\d[\d,]*(?:\.\d+)?%?/g;
const LABEL = /[一-鿿]+|[A-Za-z][A-Za-z]+/g;
const CTX = 30;

// —— 金档数值归一用 ——
const UNIT_MULT: Record<string, number> = { 千: 1e3, 万: 1e4, 百万: 1e6, 亿: 1e8 };
// 数字（含千分位/小数）+ 可选中文数量单位
const NUM_UNIT = /(\d[\d,]*(?:\.\d+)?)\s*(百万|亿|万|千)?/g;
// 行/列头里的数量单位提示，如 "（百万元）" "(亿元)"
const LINE_UNIT = /[（(]\s*(百万|亿|万|千)\s*元/;

export interface Located {
  before: string;
  hit: string;
  after: string;
  span: { start: number; end: number };
  page: number | null;
}

// —— 来源锚点解析 ——
// 命中区间在「拼接语料」的 UTF-16 下标里,而 spoor 的 provenance span 是
// 「单文档 markdown」的 UTF-8 字节区间;两步换算后取重叠最大的 span 的 source。

export interface EvidenceAnchor {
  /** 命中所在文档在语料中的下标(0-based)。 */
  doc?: number;
  page: number;
  bbox?: { x0: number; y0: number; x1: number; y1: number };
}

interface AnchorDoc {
  name: string;
  markdown: string;
  provenance?: {
    output: { start: number; end: number };
    source: { kind: string; number: number; bbox?: EvidenceAnchor["bbox"] };
  }[];
}

/** 与 corpus.joinMarkdown 保持一致的段表:每个文档 markdown 在拼接串里的起点。 */
function segmentStarts(docs: AnchorDoc[]): { mdStart: number; mdEnd: number }[] {
  const out: { mdStart: number; mdEnd: number }[] = [];
  let cursor = 0;
  for (const d of docs) {
    const header = `# 文件:${d.name}\n\n`;
    const mdStart = cursor + header.length;
    const mdEnd = mdStart + d.markdown.length;
    out.push({ mdStart, mdEnd });
    cursor = mdEnd + 2; // "\n\n" 分隔
  }
  return out;
}

/**
 * 把拼接语料里的命中区间锚回原始文档:命中落在哪个文档,就在该文档的
 * provenance spans(UTF-8 字节)里找重叠最大的一条,返回其源页与坐标框。
 * 语料无 provenance(内置演示/表格型/旧数据)时返回 null,证据照常无锚点。
 */
export function anchorFor(
  span: { start: number; end: number },
  docs: AnchorDoc[],
): EvidenceAnchor | null {
  const segments = segmentStarts(docs);
  const index = segments.findIndex((s) => span.start >= s.mdStart && span.start < s.mdEnd);
  if (index === -1) return null;
  const doc = docs[index];
  if (!doc.provenance?.length) return null;

  const { mdStart, mdEnd } = segments[index];
  const localStart = span.start - mdStart;
  const localEnd = Math.min(span.end, mdEnd) - mdStart;
  if (localEnd <= localStart) return null;

  const encoder = new TextEncoder();
  const utf8Start = encoder.encode(doc.markdown.slice(0, localStart)).length;
  const utf8End = utf8Start + encoder.encode(doc.markdown.slice(localStart, localEnd)).length;

  let best: { overlap: number; anchor: EvidenceAnchor } | null = null;
  for (const p of doc.provenance) {
    if (p.source.kind !== "page") continue;
    const overlap = Math.min(p.output.end, utf8End) - Math.max(p.output.start, utf8Start);
    if (overlap > 0 && (!best || overlap > best.overlap)) {
      best = {
        overlap,
        anchor: { doc: index, page: p.source.number, bbox: p.source.bbox },
      };
    }
  }
  if (!best) return null;
  if (best.anchor.bbox === undefined) delete best.anchor.bbox;
  return best.anchor;
}

export function pageOf(md: string, pos: number): number | null {
  let page: number | null = null;
  for (const m of md.matchAll(PAGE)) {
    if ((m.index ?? 0) <= pos) page = Number.parseInt(m[1], 10);
    else break;
  }
  return page;
}

let _cacheMd = "";
let _cache: { s: string; map: number[] } | null = null;

function stripped(md: string): { s: string; map: number[] } {
  if (md === _cacheMd && _cache) return _cache;
  const chars: string[] = [];
  const map: number[] = [];
  for (let i = 0; i < md.length; i++) {
    const ch = md[i];
    if (/\s/u.test(ch)) continue;
    chars.push(ch);
    map.push(i);
  }
  _cacheMd = md;
  _cache = { s: chars.join(""), map };
  return _cache;
}

function findSpan(md: string, quote: string): [number, number] | null {
  const idx = md.indexOf(quote);
  if (idx !== -1) return [idx, idx + quote.length];
  const { s, map } = stripped(md);
  const qn = quote.replace(/\s+/gu, "");
  if (!qn) return null;
  const j = s.indexOf(qn);
  if (j === -1) return null;
  return [map[j], map[j + qn.length - 1] + 1];
}

function allOccurrences(md: string, needle: string): number[] {
  const nn = needle.replace(/\s+/gu, "");
  if (!nn) return [];
  const { s, map } = stripped(md);
  const out: number[] = [];
  let i = s.indexOf(nn);
  while (i !== -1) {
    out.push(map[i]);
    i = s.indexOf(nn, i + 1);
  }
  return out;
}

function lineBounds(md: string, pos: number): [number, number] {
  const ls = md.lastIndexOf("\n", pos - 1) + 1;
  const le = md.indexOf("\n", pos);
  return [ls, le === -1 ? md.length : le];
}

function maxByLen(arr: string[]): string {
  let best = arr[0];
  for (const x of arr) if (x.length > best.length) best = x;
  return best;
}

// ③ 表格单元格锚点：以最具辨识度的数字为锚点，标签词校验命中行。
function anchoredSpan(md: string, quote: string): [number, number] | null {
  const nums = [...quote.matchAll(NUM)].map((m) => m[0]);
  if (nums.length === 0) return null;
  const sepNums = nums.filter((n) => /[,.%]/.test(n));
  const anchor = maxByLen(sepNums.length ? sepNums : nums);
  const hasSep = /[,.%]/.test(anchor);
  if (anchor.length < 3 && !hasSep) return null;

  const occ = allOccurrences(md, anchor);
  if (occ.length === 0) return null;

  const labels = [...quote.matchAll(LABEL)].map((m) => m[0]).filter((w) => w.length >= 2);
  const scored = occ.map((start) => {
    const [ls, le] = lineBounds(md, start);
    const line = md.slice(ls, le);
    const score = labels.reduce((acc, w) => acc + (line.includes(w) ? 1 : 0), 0);
    return { score, start, ls, le, line };
  });
  scored.sort((a, b) => b.score - a.score);
  const best = scored[0];

  if (labels.length > 0) {
    if (best.score < 1) return null;
  } else if (!(occ.length === 1 && (anchor.length >= 4 || hasSep))) {
    return null;
  }

  if (best.line.trimStart().startsWith("|")) {
    const ts = best.ls + (best.line.length - best.line.trimStart().length);
    const te = best.le - (best.line.length - best.line.trimEnd().length);
    return [ts, te];
  }
  return [best.start, best.start + anchor.length];
}

function digitsOf(numStr: string): number {
  return numStr.replace(/[.,]/g, "").replace(/^0+/, "").length;
}

function toValue(numStr: string, unit: string | undefined, lineUnit: string | undefined): number | null {
  const v = Number.parseFloat(numStr.replace(/,/g, ""));
  if (!Number.isFinite(v)) return null;
  const u = unit ?? lineUnit;
  return v * (u ? (UNIT_MULT[u] ?? 1) : 1);
}

// ④ 金档：数值/单位归一。只在 quote 的数字带数量单位时启用（说明模型换算过单位）。
// 命中要么标签校验、要么该数值全文唯一，防误配。
function numericSpan(md: string, quote: string): [number, number] | null {
  let target: number | null = null;
  for (const m of quote.matchAll(NUM_UNIT)) {
    const [, numStr, unit] = m;
    if (!unit || digitsOf(numStr) < 3) continue;
    const v = toValue(numStr, unit, undefined);
    if (v !== null && v >= 1000) {
      target = v;
      break;
    }
  }
  if (target === null) return null;

  const TOL = 0.002; // 0.2%：容忍四舍五入（1335亿 vs 1334.54亿），又不撞别的量级
  const labels = [...quote.matchAll(LABEL)].map((m) => m[0]).filter((w) => w.length >= 2);

  const candidates: { start: number; ls: number; le: number; line: string; score: number }[] = [];
  let lineStart = 0;
  for (const line of md.split("\n")) {
    const lineEnd = lineStart + line.length;
    const lu = line.match(LINE_UNIT)?.[1];
    for (const m of line.matchAll(NUM_UNIT)) {
      const [, numStr, unit] = m;
      if (digitsOf(numStr) < 3) continue;
      const v = toValue(numStr, unit, lu);
      if (v === null || v === 0) continue;
      if (Math.abs(v - target) / target <= TOL) {
        const score = labels.reduce((acc, w) => acc + (line.includes(w) ? 1 : 0), 0);
        candidates.push({ start: lineStart + (m.index ?? 0), ls: lineStart, le: lineEnd, line, score });
      }
    }
    lineStart = lineEnd + 1;
  }
  if (candidates.length === 0) return null;

  candidates.sort((a, b) => b.score - a.score);
  const best = candidates[0];
  // 标签命中 ≥1（防撞车）；或全文唯一（够硬，救"营收↔营业总收入"这类同义词假阴性）。
  const accept = (labels.length > 0 && best.score >= 1) || candidates.length === 1;
  if (!accept) return null;

  if (best.line.trimStart().startsWith("|")) {
    const ts = best.ls + (best.line.length - best.line.trimStart().length);
    const te = best.le - (best.line.length - best.line.trimEnd().length);
    return [ts, te];
  }
  const numLen = md.slice(best.start).match(/^\d[\d,]*(?:\.\d+)?/)?.[0].length ?? 1;
  return [best.start, best.start + numLen];
}

function clean(s: string): string {
  return s.replace(/\s+/gu, " ").trim();
}

/** 在 md 里定位 quote；返回可直接进 QuoteEvidence 的片段，找不到返回 null。 */
export function locate(md: string, quote: string): Located | null {
  const q = (quote ?? "").trim();
  if (!q) return null;
  let span = findSpan(md, q);
  if (span === null) span = anchoredSpan(md, q); // ③ 表格单元格
  if (span === null) span = numericSpan(md, q); // ④ 金档：数值/单位归一
  if (span === null) return null;
  const [start, end] = span;
  return {
    before: clean(md.slice(Math.max(0, start - CTX), start)),
    hit: clean(md.slice(start, end)),
    after: clean(md.slice(end, end + CTX)),
    span: { start, end },
    page: pageOf(md, start),
  };
}
