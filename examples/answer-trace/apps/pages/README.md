# answer-trace Cloudflare Pages 版

把 SvelteKit 前端和 Pages Functions 后端放进一个 Cloudflare Pages 项目，前端通过同一域名下的 `/api/*` 调用解析和问答接口。

这一版分两步查找引用：

- 先按逐字、忽略空白、表格行和数量单位换算等规则查找；
- 如果按文字和数值规则没有找到，再让模型寻找可能相关的引文。模型给出的引文必须能在解析结果中找到，并在界面上标为“需复核”。

## 本地运行

```bash
pnpm install
pnpm --filter @answer-trace/pages exec wrangler kv namespace create CORPUS
```

把 KV id 写入 `wrangler.toml`，复制 `.dev.vars.example` 为 `.dev.vars` 并填写模型凭据：

```bash
pnpm --filter @answer-trace/pages run dev:pages
```

只启动静态前端：

```bash
pnpm --filter @answer-trace/pages dev
```

检查与部署：

```bash
pnpm --filter @answer-trace/pages run check
pnpm --filter @answer-trace/pages run check:functions
pnpm --filter @answer-trace/pages test
pnpm --filter @answer-trace/pages run deploy
```

## 注意

- 上传内容会发送到 Cloudflare Pages Functions，并临时写入 KV；不是浏览器本地解析。
- 让模型继续查找会多调用一次模型，结果始终标为“需复核”。
- 这是演示服务，没有身份认证、用户数据隔离和用量限制。
