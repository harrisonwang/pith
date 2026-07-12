// POST /api/upload (multipart files) —— spoor-wasm 解析 → 写入 KV 语料，返回 corpusId。
import { cfg, type Env } from "../_lib/config";
import * as corpus from "../_lib/corpus";
import { json } from "../_lib/http";
import { rewriteSpoorImages } from "../_lib/mediaUrls";
import { MAX_REQUEST_BYTES, parseDoc } from "../_lib/parse";
import * as store from "../_lib/store";
import { count as countTokens } from "../_lib/tokens";

export const onRequestPost: PagesFunction<Env> = async ({ request, env }) => {
  const declared = Number(request.headers.get("content-length") ?? 0);
  if (declared > MAX_REQUEST_BYTES) return json({ detail: "请求超过此演示的 16 MiB 上限。" }, 413);

  const base = request.url;
  const form = await request.formData();
  // workers-types 把 getAll 标成 string[]，运行时其实是 File；按 arrayBuffer 鸭子判定取文件。
  const files = (form.getAll("files") as unknown[]).filter(
    (f): f is File => typeof f === "object" && f !== null && "arrayBuffer" in f,
  );

  const docs: corpus.DocBytes[] = [];
  const results: { name: string; ok: boolean; chars?: number; error?: string }[] = [];
  for (const f of files) {
    const name = f.name || "file";
    const data = new Uint8Array(await f.arrayBuffer());
    try {
      const parsed = parseDoc(name, data); // 单个文件失败不连累其它
      docs.push({ name, markdown: parsed.markdown, provenance: parsed.provenance, bytes: data });
      results.push({ name, ok: true, chars: parsed.markdown.length });
    } catch (exc) {
      results.push({ name, ok: false, error: String(exc) });
    }
  }

  let corpusId: string | undefined;
  if (docs.length) corpusId = await corpus.setDocs(env, docs);

  // 用本地 docs 直接拼响应，避免 read-after-write 读到刚写的 KV 键（最终一致）。
  const joined = docs.length ? corpus.joinMarkdown(docs) : await store.documentMarkdown(env, base);
  const src = docs.length ? corpus.uploadedSourceRef(docs, joined) : await store.sourceRef(env, base);
  const md = rewriteSpoorImages(joined, 0, corpusId);

  return json({
    files: results,
    source: src,
    markdown: md,
    tokens: countTokens(joined),
    contextLimit: cfg(env).contextLimit,
    corpusId,
  });
};
