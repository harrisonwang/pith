// POST /api/ask {question, corpusId?} —— 真问真答 + 分级核验 → AnswerTrace。
import { llmEnabled, type Env } from "../_lib/config";
import * as corpus from "../_lib/corpus";
import * as store from "../_lib/store";
import { json } from "../_lib/http";
import { buildTrace } from "../_lib/matcher";

export const onRequestPost: PagesFunction<Env> = async ({ request, env }) => {
  const body = (await request.json().catch(() => ({}))) as { question?: string; corpusId?: string };
  const question = (body.question ?? "").trim();
  if (!question) return json({ detail: "问题不能为空" }, 400);
  if (!llmEnabled(env)) {
    return json(
      {
        detail:
          "未配置模型后端:设 AT_BASE_URL + AT_API_KEY(OpenRouter/DeepSeek/z.ai 等),或 CF_ACCOUNT_ID + CF_API_TOKEN。",
      },
      503,
    );
  }
  const base = request.url;
  const corpusId = body.corpusId ?? null;
  try {
    // 上传语料连 provenance 一起取:证据命中后可锚回源页坐标。
    const docs = await corpus.docs(env, corpusId);
    const md = docs ? corpus.joinMarkdown(docs) : await store.documentMarkdown(env, base);
    const src = docs
      ? corpus.uploadedSourceRef(docs, md)
      : await store.sourceRef(env, base);
    return json(await buildTrace(env, question, md, src, docs));
  } catch (exc) {
    return json({ detail: `模型调用或解析失败:${String(exc)}` }, 502);
  }
};
