# answer-trace

**答案溯源** —— 对一份真实文档做问答,并**逐条证明 AI 的回答有原文出处、不是杜撰的**。

每条结论就地标可信度(✓已核验 / ~需复核 / ✗无法核验)并附**原文证据**(引文片段,或表格的「表·行·列=值」+ 页码);hover 看证据,点「定位原文」滑出原文下钻。演示数据是真实研报(东吴证券《比亚迪 2024 年报点评》)经 spoor 解析的内容。

前后端一个工程,中间夹一份**共享协议** `AnswerTrace`(matcher 后端 → viewer 前端的唯一契约)。

## 结构

```
answer-trace/
  apps/
    web/        # SvelteKit 前端(对话式 + 内联 claim + 证据 popover + 原文抽屉)
    api/        # FastAPI 后端(pyspoor)：/api/demo · /api/ask · /api/upload
    edge/       # 边缘后端：独立 Cloudflare Worker(spoor-wasm + KV)，前端跨源调
    pages/      # ★ Cloudflare Pages：前端(静态) + Pages Functions 后端合一，同源一次部署；
                #   并升级为分级核验（金档数值/单位归一 + 银档语义蕴含）
  packages/
    protocol/   # 共享协议:answer-trace.schema.json + TS 类型 + pydantic + 真实 fixture
  Taskfile.yml  pnpm-workspace.yaml  package.json
```

> 三种后端形态可任选：`api`(Python 本地) · `edge`(独立 Worker) · `pages`(Pages Functions 合一，含分级核验)。前端 `web` 与它们靠 `/api/*` 契约解耦。

> **路线图(暂未建,需要时再加):** `apps/worker`(异步解析/核验任务)、`packages/ui`(抽成可嵌入 Svelte 组件库)、`infra/`(docker-compose + Dockerfile,部署时)、`openapi.yaml + generated/`(协议漂移了再上 codegen)、**PDF.js 画框**(数据已就绪:上传语料经 spoor `provenance:"block"` 解析,quote 证据带 `anchor{page,bbox}`——bbox 为 PDF 原生用户空间坐标,直接 `viewport.convertToViewportRectangle`;原始 PDF 在 KV `raw:${corpusId}:${i}`,补一个 raw 端点 + pdfjs-dist 单页渲染即可)、**matcher 表格 cell 证据**(live 答案现在都走 quote 证据)、**live 答案的原文下钻**(现在仅内置三轮可「定位原文」)。

## 跑起来

需要 `pnpm` 和 `uv`。

```bash
# 安装
pnpm install
cd apps/api && uv sync && cd ../..

# 一条命令同时启动前后端
pnpm dev

# 或装了 go-task 后
task dev

# 也可以分别在两个终端启动
cd apps/api && uv run uvicorn app.main:app --reload --port 8000   # http://localhost:8000
pnpm --filter @answer-trace/web dev                               # http://localhost:5173
```

web 会先用内置 fixture 立即渲染,再向 api 刷新;**api 没起也能独立看**(右上角标「内置 fixture / 实时·api」)。

如果项目目录被移动或重命名后出现 `Failed to spawn: uvicorn`，在 `apps/api` 下执行
`uv sync --reinstall --locked`，重写虚拟环境内带绝对路径的启动脚本。

### 真问真答(phase 2,经 Cloudflare Workers AI)

底部输入框提问会调 `/api/ask`:**gemma 生成答案 → qwen3 逐条判定**,且**每条证据必须在真实 spoor 产物里逐字定位到才算数,定位不到一律降级为「无法核验」**(反幻觉闸门,见 `apps/api/app/services/locate.py`)。需要 CF 免费凭据(Workers AI 免费额度 10,000 neurons/天):

```bash
cd apps/api && cp .env.example .env   # 填 CF_ACCOUNT_ID 与 CF_API_TOKEN
```

没填凭据时 `/api/ask` 返回 503、前端给友好提示;内置三轮 demo 不受影响。Workers AI 走 REST,不必部署到 CF Workers。

底部 **📎 可多文件上传**:经 **pyspoor**(由 `uv sync` 从 PyPI 安装预编译 wheel,无需本地 Rust/maturin)把 PDF/DOCX/XLSX 解析成 markdown 作为「当前依据」,之后的问答与「定位原文」都针对上传文件。「定位原文」对**所有**回答(内置 / 实时 / 上传)都生效——抽屉只渲染命中所在那一页并定位。

## 协议:`spoor.answer-trace.v1`

一轮问答 = 一个 `AnswerTrace`:答案拆成 `text` / `claim` 片段(claim 带三态 + 证据 id),`evidence` 是 `quote`(引文)/ `cell`(表格)/ `none`(无法核验,给原因+真值)三选一,外加 `source`(被核验的产物)和 `audit`(parser / generator / judge / judgedAt —— 可复现 + 留痕,审计卖点)。

- 权威 schema:[`packages/protocol/answer-trace.schema.json`](packages/protocol/answer-trace.schema.json)
- TS 类型:`packages/protocol/src/index.ts` · pydantic:`packages/protocol/python/answer_trace.py`
- 真实样例:`packages/protocol/fixtures/demo.json`

前端只认这一个契约。matcher(`apps/api/app/services/matcher.py`)已接上 **Cloudflare Workers AI** 产出同一套 AnswerTrace,`web` 零改动(数据出口集中在 `apps/web/src/lib/loadTrace.ts`)。
