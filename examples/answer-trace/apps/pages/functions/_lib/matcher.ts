// matcher：question + spoor 产物 → AnswerTrace。
//
// 1) 生成(gen)：仅依据文档回答。
// 2) 判定(judge,JSON)：把回答拆成 text/claim，对每个 claim 给 verdict + 原文 quote。
// 3) 分级核验：
//    金档 = 确定性 locate（逐字/空白/表格/数值单位归一）→ ✓/~；
//    银档 = 对"判定支持但金档定位失败"的 claim 做一次批量蕴含检索，
//           返回的 quote **仍要确定性 locate 到**才算数（不洗白幻觉），命中降一档为 ~ 并标注语义匹配；
//    定位不到 → 「无法核验」(反幻觉闸门)。

import type {
  AnswerPart,
  AnswerTrace,
  Evidence,
  NoEvidence,
  QuoteEvidence,
  SourceRef,
  Verdict,
} from "@answer-trace/protocol";
import { SCHEMA_VERSION } from "@answer-trace/protocol";

import { chat } from "./cf";
import { cfg, type Env } from "./config";
import { anchorFor, type Located, locate } from "./locate";
import type { StoredDoc } from "./corpus";

const GEN_SYS =
  "你是严谨的中文金融文档助手。**只依据下面的【文档】回答**,不要编造文档中没有的数字或事实," +
  "数字必须与文档一致。像专业分析师那样自然作答:直接给结论,能用连贯的段落讲清就不要硬拆成一行一点;" +
  "确需并列多项指标时才用简短列表。关键数字/字段可用 **加粗** 突出,不要用 Markdown 表格或代码块。";

const JUDGE_SYS =
  "你是严格的事实核查器。给你【文档】【问题】【回答】。把【回答】**按出现顺序、逐字**拆成片段:" +
  "普通叙述为 text;可核查的事实陈述为 claim。所有片段的 text 顺序拼接必须等于原【回答】(不增删字)。" +
  "**只输出 JSON,不要解释**。/no_think\n" +
  "claim 规则:必须是**完整、含具体数值/指标**的事实句(例:『Q4 归母净利润 150 亿元,同环比+73%/+29%』)。" +
  "**严禁**把光秃秃的字段名/标题(如『归母净利润:』『毛利率:』)单独当一个 claim——" +
  "claim 的 text 必须覆盖到数值本身,把『字段名 + 数值(+变动)』整段作为一个 claim。\n" +
  "每个 claim 的 quote 必须是【文档】里一字不差的原文子串;没有就留空字符串。\n" +
  "依据若来自表格,quote 取**数据所在的整行原文**(形如 `| 行名 | 值1 | 值2 |`)或**该单元格数值本身**," +
  "切勿把列名/行名/单位拆开重拼成一句——那不是原文子串,会定位失败。\n" +
  '格式:{"parts":[' +
  '{"type":"text","text":"..."},' +
  '{"type":"claim","text":"含数值的完整事实句","verdict":"supported|partial|unsupported",' +
  '"quote":"文档中支持它的原文(逐字),没有就\\"\\"","note":"为何 partial/unsupported,可空",' +
  '"truth":"正确值,仅 unsupported 时可填,可空"}' +
  "]}\n" +
  "判据:supported=核心数值/事实被文档明确支持(允许等价换算,如 1335亿 = 133,454百万);" +
  "partial=措辞接近但需复核(概括/约数等);" +
  "unsupported=文档没有或与文档矛盾(quote 留空)。";

const SILVER_SYS =
  "你是严格的证据检索器。给你【文档】和若干【断言】。对每条断言,判断文档是否**支持**它;" +
  "若支持,给出文档里**能直接支撑它、且能在文档中逐字搜到的最短原文片段**(数字/单位以文档原样为准,别改写)。" +
  "宁缺毋滥:不确定、文档没有、或与文档矛盾,一律 supported=false、quote 留空。**只输出 JSON**。/no_think\n" +
  '格式:{"results":[{"i":1,"supported":true,"quote":"文档原文片段"},{"i":2,"supported":false,"quote":""}]}';

interface JudgeSeg {
  type?: string;
  text?: string;
  verdict?: Verdict;
  quote?: string;
  note?: string;
  truth?: string;
}

interface ClaimRecord {
  eid: string;
  text: string;
  judgeVerdict: Verdict;
  quote: string;
  note: string | null;
  truth: string | null;
  located: Located | null;
  silver: boolean;
}
type Item = { type: "text"; text: string } | { type: "claim"; rec: ClaimRecord };

async function generate(env: Env, question: string, md: string): Promise<string> {
  return chat(
    env,
    cfg(env).genModel,
    [
      { role: "system", content: GEN_SYS },
      { role: "user", content: `【文档】\n${md}\n\n【问题】${question}` },
    ],
    { temperature: 0.3 },
  );
}

async function judge(env: Env, question: string, answer: string, md: string): Promise<{ parts?: JudgeSeg[] }> {
  const raw = await chat(
    env,
    cfg(env).judgeModel,
    [
      { role: "system", content: JUDGE_SYS },
      { role: "user", content: `【文档】\n${md}\n\n【问题】${question}\n\n【回答】${answer}` },
    ],
    { temperature: 0, jsonMode: true },
  );
  return extractJsonObject(raw) as { parts?: JudgeSeg[] };
}

// 银档：对未命中的 claim 批量做蕴含检索，返回它们在文档中的逐字支撑片段（宁缺毋滥）。
async function silverEntail(
  env: Env,
  md: string,
  claims: string[],
): Promise<{ i: number; supported: boolean; quote: string }[]> {
  const list = claims.map((c, k) => `${k + 1}. ${c}`).join("\n");
  const raw = await chat(
    env,
    cfg(env).judgeModel,
    [
      { role: "system", content: SILVER_SYS },
      { role: "user", content: `【文档】\n${md}\n\n【断言】\n${list}` },
    ],
    { temperature: 0, jsonMode: true },
  );
  let parsed: { results?: { i?: number; supported?: boolean; quote?: string }[] };
  try {
    parsed = extractJsonObject(raw) as typeof parsed;
  } catch {
    return [];
  }
  return (parsed.results ?? []).map((r) => ({
    i: Number(r.i) || 0,
    supported: Boolean(r.supported),
    quote: (r.quote ?? "").trim(),
  }));
}

function extractJsonObject(raw: string): unknown {
  const s = raw.trim().replace(/^```(?:json)?|```$/gm, "").trim();
  const m = s.match(/\{[\s\S]*\}/);
  if (!m) throw new Error(`模型未返回 JSON:${raw.slice(0, 200)}`);
  return JSON.parse(m[0]);
}

export async function buildTrace(
  env: Env,
  question: string,
  md: string,
  source: SourceRef,
  docs?: StoredDoc[] | null,
): Promise<AnswerTrace> {
  const answer = await generate(env, question, md);
  const parsed = await judge(env, question, answer, md);

  // Pass 1：按序建 items，claim 跑金档确定性 locate。
  const items: Item[] = [];
  let n = 0;
  for (const seg of parsed.parts ?? []) {
    if (seg.type !== "claim") {
      items.push({ type: "text", text: seg.text ?? "" });
      continue;
    }
    n += 1;
    const quote = (seg.quote ?? "").trim();
    items.push({
      type: "claim",
      rec: {
        eid: `e${n}`,
        text: seg.text ?? "",
        judgeVerdict: seg.verdict ?? "unsupported",
        quote,
        note: seg.note || null,
        truth: seg.truth || null,
        located: quote ? locate(md, quote) : null,
        silver: false,
      },
    });
  }

  // 银档：判定支持但金档没定位到的 claim，批量蕴含检索；返回 quote 仍要 locate 到才采纳。
  const unmatched = items
    .filter((it): it is { type: "claim"; rec: ClaimRecord } => it.type === "claim")
    .map((it) => it.rec)
    .filter(
      (r) => !r.located && (r.judgeVerdict === "supported" || r.judgeVerdict === "partial") && !!r.text,
    );

  if (unmatched.length > 0) {
    const rescued = await silverEntail(env, md, unmatched.map((r) => r.text));
    for (const r of rescued) {
      const rec = unmatched[r.i - 1];
      if (!rec || !r.supported || !r.quote) continue;
      const loc = locate(md, r.quote);
      if (loc) {
        rec.located = loc;
        rec.silver = true;
      }
    }
  }

  // 装配
  const parts: AnswerPart[] = [];
  const evidence: Evidence[] = [];
  for (const it of items) {
    if (it.type === "text") {
      parts.push({ type: "text", text: it.text });
      continue;
    }
    const rec = it.rec;
    let verdict: Verdict = rec.judgeVerdict;
    let ev: Evidence;

    if (rec.located && (rec.judgeVerdict === "supported" || rec.judgeVerdict === "partial")) {
      if (rec.silver) verdict = "partial"; // 银档统一降一档为 ~，并标注语义匹配
      const q: QuoteEvidence = {
        id: rec.eid,
        kind: "quote",
        verdict,
        page: rec.located.page,
        before: rec.located.before,
        hit: rec.located.hit,
        after: rec.located.after,
        span: rec.located.span,
      };
      // 上传语料带块级 provenance 时,把命中锚回源页坐标(内置演示无,自然缺省)。
      if (docs?.length) {
        const anchor = anchorFor(rec.located.span, docs);
        if (anchor) q.anchor = anchor;
      }
      const noteText = rec.silver
        ? rec.note
          ? `${rec.note}（经二次核验的语义匹配，非逐字原文）`
          : "经二次核验的语义匹配（非逐字原文）"
        : rec.note;
      if (noteText) q.note = noteText;
      ev = q;
    } else {
      verdict = "unsupported";
      const ne: NoEvidence = {
        id: rec.eid,
        kind: "none",
        verdict: "unsupported",
        page: null,
        reason:
          rec.note ?? (rec.quote ? "模型给出的依据在原文中未找到,疑似杜撰。" : "原文中未找到支撑该结论的内容。"),
      };
      if (rec.truth) ne.expectedTruth = rec.truth;
      ev = ne;
    }

    evidence.push(ev);
    parts.push({ type: "claim", text: rec.text, verdict, evidenceIds: [rec.eid] });
  }

  return {
    schema: SCHEMA_VERSION,
    question,
    answer: parts,
    evidence,
    source,
    audit: {
      parser: "spoor-wasm@0.11",
      generator: cfg(env).genModel,
      judge: cfg(env).judgeModel,
      judgedAt: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
    },
  };
}
