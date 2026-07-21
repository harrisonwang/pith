# answer-trace

这个示例用来检查回答中的引用。向文档提问后，可以逐条查看相关文字是否出现在 spoor 的解析结果中，再回到原 PDF 查看对应位置。

状态分为三种：

- `✓ 已核验`：负责检查引用的模型认为说法有依据，而且它给出的引文能在解析结果中找到；这不代表人工已经确认；
- `~ 需复核`：找到了可能相关的内容，但不是逐字引用；
- `✗ 无法核验`：没有找到足以核对这条说法的内容。

上传 PDF 时，后端使用 `provenance="block"` 获取页码和近似坐标；前端用 PDF.js 显示原页面并标出位置。没有页面坐标时，界面只显示找到的文字，不在 PDF 页面上画框。

## 结构

```text
apps/web    SvelteKit 前端
apps/api    FastAPI + pyspoor 本地后端
apps/edge   独立 Cloudflare Worker 后端
apps/pages  Cloudflare Pages 前后端一体版本
packages/protocol  前后端共享的 AnswerTrace 协议
```

## 本地运行

需要 `pnpm` 和 `uv`：

```bash
pnpm install
cd apps/api && uv sync && cd ../..
pnpm dev
```

也可以分别启动：

```bash
cd apps/api && uv run uvicorn app.main:app --reload --port 8000
pnpm --filter @answer-trace/web dev
```

没有后端时，前端会使用内置样例。要进行实时问答，把 `apps/api/.env.example` 复制为 `.env`，填写 Cloudflare Workers AI 凭据。

## 接口格式

一轮问答使用 `spoor.answer-trace.v1`：

- `answer`：回答文字和需要核对的说法；
- `evidence`：找到的引文、表格单元格，或未找到的状态；
- `source`：内容来自哪份文档；
- `audit`：使用的解析器、回答模型、负责检查引用的模型和时间。

格式定义位于 `packages/protocol/answer-trace.schema.json`，TypeScript 和 Python 使用同一份定义。

## 注意

- 找到引文不等于它足以支持结论；找到出处和判断结论是否成立是两件事。
- “已核验”是演示中的机器判定。涉及单位换算、四舍五入或重要结论时，仍应人工复核。
- 上传文件会发送到所选后端。使用 `apps/api` 时，文件会发送到本机服务；使用 edge 或 pages 时，文件会发送到 Cloudflare。
- 实时问答会把解析后的文本发送给配置的模型服务。
- 这是产品演示，没有身份认证、用量控制、用户数据隔离和长期保存功能。
