# Cloudflare Worker 示例

这是一个简单的 Cloudflare Worker 文档解析 API。客户端把文件内容 `POST` 到 Worker，Worker 调用 `spoor-wasm` 返回 `ParseResult`。

请求应带 `x-filename` 和 `content-type`，以便识别格式。示例的请求和单次处理上限均为 16 MiB。

## 本地运行

```bash
cd examples/cloudflare-worker
npm ci
npm run dev
```

```bash
curl -X POST http://localhost:8787 \
  -H 'x-filename: report.docx' \
  -H 'content-type: application/vnd.openxmlformats-officedocument.wordprocessingml.document' \
  --data-binary @report.docx
```

部署：

```bash
npx wrangler login
npm run deploy
```

## 注意

这是一个没有认证的演示 API，也没有用户数据隔离、长期存储、限流和强制超时。文件会上传到 Cloudflare。即使文件小于 16 MiB，结构复杂的文档也可能超过 Worker 的 CPU 或内存限制。
