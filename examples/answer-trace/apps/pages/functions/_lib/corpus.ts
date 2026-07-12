// 当前语料（KV 态）。每次上传生成 corpusId，前端在后续 /ask、/media 带回它，多用户隔离。
// 未上传（无 corpusId）回退到内置 byd.md（经 env.ASSETS 取，故需要 base=request.url）。
// KV 最终一致：upload 响应用本地 docs 直接拼，不 read-after-write。

import type { Env } from "./config";
import type { ProvenanceSpan } from "./spoor";
import * as store from "./store";

const TTL = 60 * 60 * 24; // 24h：演示数据自清理

export interface StoredDoc {
  name: string;
  markdown: string;
  /** 块级来源定位(可缺省:旧语料/表格型没有)。 */
  provenance?: ProvenanceSpan[];
}
interface StoredCorpus {
  docs: StoredDoc[];
}

export interface DocBytes {
  name: string;
  markdown: string;
  provenance?: ProvenanceSpan[];
  bytes: Uint8Array;
}

export function joinMarkdown(docs: StoredDoc[]): string {
  return docs.map((d) => `# 文件:${d.name}\n\n${d.markdown}`).join("\n\n");
}

export function uploadedSourceRef(
  docs: StoredDoc[],
  joined: string,
): { documentId: string; title: string; pages: number } {
  const first = docs[0].name;
  const title = docs.length === 1 ? first : `${first} 等 ${docs.length} 个文件`;
  return { documentId: "uploaded", title, pages: joined.split("## Page ").length - 1 };
}

export async function setDocs(env: Env, docs: DocBytes[]): Promise<string> {
  const corpusId = crypto.randomUUID();
  const stored: StoredCorpus = {
    docs: docs.map((d) => ({ name: d.name, markdown: d.markdown, provenance: d.provenance })),
  };
  await env.CORPUS.put(`corpus:${corpusId}`, JSON.stringify(stored), { expirationTtl: TTL });
  await Promise.all(
    docs.map((d, i) => env.CORPUS.put(`raw:${corpusId}:${i}`, d.bytes, { expirationTtl: TTL })),
  );
  return corpusId;
}

async function getCorpus(env: Env, corpusId: string): Promise<StoredCorpus | null> {
  const raw = await env.CORPUS.get(`corpus:${corpusId}`);
  return raw ? (JSON.parse(raw) as StoredCorpus) : null;
}

/** 上传语料的文档列表(含各自的来源定位);无 corpusId 或已过期时为 null。 */
export async function docs(env: Env, corpusId: string | null): Promise<StoredDoc[] | null> {
  if (!corpusId) return null;
  const c = await getCorpus(env, corpusId);
  return c && c.docs.length ? c.docs : null;
}

export async function markdown(env: Env, base: string, corpusId: string | null): Promise<string> {
  if (corpusId) {
    const c = await getCorpus(env, corpusId);
    if (c && c.docs.length) return joinMarkdown(c.docs);
  }
  return store.documentMarkdown(env, base);
}

export async function sourceRef(
  env: Env,
  base: string,
  corpusId: string | null,
): Promise<{ documentId: string; title: string; pages: number }> {
  if (corpusId) {
    const c = await getCorpus(env, corpusId);
    if (c && c.docs.length) return uploadedSourceRef(c.docs, joinMarkdown(c.docs));
  }
  return store.sourceRef(env, base);
}

export async function getDoc(
  env: Env,
  base: string,
  corpusId: string | null,
  index: number,
): Promise<DocBytes | null> {
  if (corpusId) {
    const c = await getCorpus(env, corpusId);
    if (!c || index < 0 || index >= c.docs.length) return null;
    const buf = await env.CORPUS.get(`raw:${corpusId}:${index}`, "arrayBuffer");
    if (!buf) return null;
    const doc = c.docs[index];
    return { name: doc.name, markdown: doc.markdown, bytes: new Uint8Array(buf) };
  }
  if (index === 0) {
    return {
      name: "byd_report.pdf",
      markdown: await store.documentMarkdown(env, base),
      bytes: await store.builtinRawBytes(env, base),
    };
  }
  return null;
}
