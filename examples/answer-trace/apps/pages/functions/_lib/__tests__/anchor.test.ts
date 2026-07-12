// anchorFor：把「拼接语料里的 UTF-16 命中区间」锚回「单文档 provenance
// （UTF-8 字节）」的换算与重叠逻辑。含 CJK 混排（换算差异最大的场景）。

import { describe, expect, it } from "vitest";

import { anchorFor } from "../locate";

const DOC_A_MD = "## Page 1\n\n营业总收入 7,771.02 亿元，同比增长 29.0%。";
const DOC_B_MD = "## Page 1\n\nRevenue grew steadily.";

function utf8Len(text: string): number {
  return new TextEncoder().encode(text).length;
}

// 文档 A 的行级 provenance：正文行带 bbox，页头是无 bbox 的 gap。
const lineStart = utf8Len("## Page 1\n\n");
const DOC_A = {
  name: "a.pdf",
  markdown: DOC_A_MD,
  provenance: [
    {
      output: { start: 0, end: lineStart },
      source: { kind: "page", number: 1 },
    },
    {
      output: { start: lineStart, end: utf8Len(DOC_A_MD) },
      source: {
        kind: "page",
        number: 1,
        bbox: { x0: 72, y0: 700, x1: 400, y1: 715 },
      },
    },
  ],
};
const DOC_B = { name: "b.pdf", markdown: DOC_B_MD };

// 与 corpus.joinMarkdown 相同的拼接。
function joined(docs: { name: string; markdown: string }[]): string {
  return docs.map((d) => `# 文件:${d.name}\n\n${d.markdown}`).join("\n\n");
}

describe("anchorFor：跨文档段表 + UTF-16→UTF-8 + 最大重叠", () => {
  it("CJK 命中锚回正文行的 bbox（第二个文档段）", () => {
    const docs = [DOC_B, DOC_A];
    const md = joined(docs);
    const hitText = "同比增长 29.0%";
    const start = md.indexOf(hitText);
    const anchor = anchorFor({ start, end: start + hitText.length }, docs);

    expect(anchor).not.toBeNull();
    expect(anchor!.page).toBe(1);
    expect(anchor!.doc).toBe(1);
    expect(anchor!.bbox).toEqual({ x0: 72, y0: 700, x1: 400, y1: 715 });
  });

  it("命中页头 gap 时给页码、不给 bbox", () => {
    const docs = [DOC_A];
    const md = joined(docs);
    const start = md.indexOf("## Page 1");
    const anchor = anchorFor({ start, end: start + 4 }, docs);

    expect(anchor).not.toBeNull();
    expect(anchor!.page).toBe(1);
    expect(anchor!.bbox).toBeUndefined();
  });

  it("无 provenance 的文档命中返回 null（内置演示/表格型）", () => {
    const docs = [DOC_B];
    const md = joined(docs);
    const start = md.indexOf("Revenue");
    expect(anchorFor({ start, end: start + 7 }, docs)).toBeNull();
  });

  it("命中 `# 文件:` 拼接头（任何文档 markdown 之外）返回 null", () => {
    const docs = [DOC_A];
    expect(anchorFor({ start: 0, end: 5 }, docs)).toBeNull();
  });
});
