# answer-trace Cloudflare Worker 后端

这是一个独立的 Cloudflare Worker。它使用 `spoor-wasm` 解析上传的文档，将文档暂存在 KV 中，再通过兼容 OpenAI 接口的模型服务或 Cloudflare Workers AI 完成问答。

它提供与 `apps/api` 基本相同的接口：

| 方法 | 路径 | 作用 |
| --- | --- | --- |
| `GET` | `/api/demo` | 返回内置演示 |
| `POST` | `/api/ask` | 根据问题返回 AnswerTrace；可以用 `corpusId` 指定刚上传的一组文档 |
| `POST` | `/api/upload` | 解析多个文件并临时保存 |
| `GET` | `/api/media` | 按 `spoor://` 链接提取图片或嵌入文件 |
| `GET` | `/api/health` | 健康检查 |

## 本地运行

```bash
pnpm install
pnpm --filter @answer-trace/edge exec wrangler kv namespace create CORPUS
pnpm --filter @answer-trace/edge exec wrangler kv namespace create CORPUS --preview
```

把生成的 KV id 写入 `wrangler.toml`，再把 `.dev.vars.example` 复制为 `.dev.vars` 并填写模型凭据：

```bash
pnpm --filter @answer-trace/edge dev
VITE_API_URL=http://localhost:8787 pnpm --filter @answer-trace/web dev
```

检查与部署：

```bash
pnpm --filter @answer-trace/edge typecheck
pnpm --filter @answer-trace/edge test
pnpm --filter @answer-trace/edge deploy
```

## 注意

- 请求上限为 16 MiB；复杂文档还受 Worker CPU 和内存限制。
- 上传的文件会暂存在 KV 中，每组文件都有一个 `corpusId`，默认 24 小时后删除。KV 写入后不一定立即可见，刚上传的内容可能暂时查不到。
- 这是演示服务，没有身份认证、用户数据隔离和用量限制。
- 文件会上传到 Cloudflare，解析后的文字还可能发送给配置的模型服务。因此，这个示例不属于本地解析。
