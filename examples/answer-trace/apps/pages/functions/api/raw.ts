// GET /api/raw?corpus={id}&doc={i} —— 返回语料文档的原始字节。
// PDF.js 用它取原 PDF 渲染页并按证据 anchor 画高亮框;无 corpus 时回退内置演示 PDF。
import type { Env } from "../_lib/config";
import * as corpus from "../_lib/corpus";
import { json } from "../_lib/http";

export const onRequestGet: PagesFunction<Env> = async ({ request, env }) => {
  const url = new URL(request.url);
  const corpusId = url.searchParams.get("corpus");
  const index = Number.parseInt(url.searchParams.get("doc") ?? "0", 10);
  if (!Number.isInteger(index) || index < 0) return json({ detail: "doc 参数无效" }, 400);

  const doc = await corpus.getDoc(env, request.url, corpusId, index);
  if (!doc) return json({ detail: "语料不存在或已过期(演示数据 24h 自清理)。" }, 404);

  const isPdf = doc.bytes[0] === 0x25 && doc.bytes[1] === 0x50 && doc.bytes[2] === 0x44;
  return new Response(doc.bytes, {
    headers: {
      "content-type": isPdf ? "application/pdf" : "application/octet-stream",
      "cache-control": "private, max-age=3600",
      "content-disposition": `inline; filename*=UTF-8''${encodeURIComponent(doc.name)}`,
    },
  });
};
