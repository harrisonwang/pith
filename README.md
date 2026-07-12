# spoor

把文档转成 LLM 可直接消费的文本。同一套引擎，根据运行环境提供 CLI、原生库、WASM 三种交付形态。敏感文件始终不离开你的运行环境。

> **当前状态**：`spoor-core`、CLI、Python、Node 与 WASM 入口均已落地。完整规划见 [docs/v1/](docs/v1/)。

## 核心特性

- **按形态自动分派输出**：文档型（PDF/DOCX/PPTX/EPUB/IPYNB/HTML）→ Markdown，表格型（CSV/XLSX）→ JSON（headers + preview + range）
- **离线、单二进制**：无云依赖，不需要 Python 环境，敏感文件本地处理
- **Agent 友好**：结构化错误、带页/slide 位置的完整性 warnings、输出自描述（usage/truncated/warnings）、JSON 扁平 `tables[]`
- **内建防御**：限制单次解析的数据量、ZIP 炸弹三重防御（entry/ratio/total cap）、256 KiB 输出封顶
- **重点格式**：DOCX、XLSX、PDF、PPTX、HTML/URL、EPUB、IPYNB
- **基础格式**：CSV/TSV、Markdown、纯文本与常见代码文件

包体大小（2026-06-12 实测）：

| 形态 | 大小 |
|------|------|
| `spoor-core` crate | < 140 KiB |
| CLI（macOS arm64 单二进制） | ~4.7 MiB |
| `pyspoor` abi3 wheel | ~1.3 MiB |
| Node addon | ~2.8 MiB |
| `core-formats` WASM（可选裁剪构建） | ~1.4 MiB raw / ~578 KiB gzip |
| 默认发布 WASM（全格式） | ~2.1 MiB raw / ~841 KiB gzip |

## 安装

```bash
# macOS / Linux
brew install harrisonwang/tap/spoor

# Windows
scoop bucket add harrisonwang https://github.com/harrisonwang/scoop-bucket
scoop install spoor

# 跨平台 CLI（npm）
npm install -g @harrisonwang/spoor-cli

# 源码安装（需 Rust toolchain）
cargo install spoor-cli
```

## 使用

```bash
# 文档型 → Markdown
spoor report.pdf
spoor report.docx slides.pptx
spoor https://example.com/article

# 表格型 → JSON（schema + preview）
spoor data.xlsx
spoor data.xlsx --sheet Sheet1 --rows 5:104 --columns 分类,金额
spoor data.csv | jq '.tables[0].headers'

# stdin / pipe
cat data.csv | spoor --format csv -

# glob
spoor "docs/*.pdf"

# 按正文占位符提取单个 DOCX/PPTX 图片
spoor document.docx --extract spoor://docx/part/word/media/image1.png > image.png
spoor deck.pptx     --extract spoor://pptx/part/ppt/media/image1.png  > image.png

# 直接喂给 LLM
spoor report.pdf | llm "总结风险和行动项"
```

输出模式按格式自动分派，`-m` 可显式覆盖。表格型 JSON 默认返回前 100 行预览，通过 `--rows` / `--columns` / `--limit` / `--offset` 收窄。详见 `spoor --help`。

DOCX、PPTX 内嵌栅格图片会在原始正文位置输出统一的安全占位符，例如
`![DOCX image 1](spoor://docx/part/word/media/image1.png)` 或
`![PPTX image 2 (slide 1)](spoor://pptx/part/ppt/media/image1.png)`；PDF 同理用
`![PDF image 1 (p1)](spoor://pdf/obj/{id}/{gen})`。Agent 可使用
`spoor document.docx --extract spoor://docx/part/word/media/image1.png > image.png`
提取相关图片并交给外部 VLM；`--extract` 只接受 spoor 输出的安全 URI，
只支持单个输入和单个资源。spoor 不解码或理解图片。

## 嵌入

Rust core 只接收 bytes 与 metadata，不执行文件、网络或进程 I/O：

```rust
let mut request = spoor_core::ParseRequest::new(bytes);
request.source_name = Some("report.docx");
let result = spoor_core::parse(&request)?;
```

需要按解析结果中的安全 URI 提取单个内嵌媒体时，使用格式无关的
`spoor_core::extract_media(&request, uri)`；当前支持 `spoor://docx/part/`、
`spoor://pptx/part/` 与 `spoor://pdf/obj/`。

Agent 应优先调用 `parse` 并处理 `warnings[]`。只需要 Markdown 的兼容场景可调用
`parse_document`；需要强制文档输出并保留 warnings 时使用 `parse_document_result`。

Python 使用 `pyspoor` 的 `parse_bytes` / `parse_path`；Node.js 使用
`@harrisonwang/spoor`；浏览器与 Edge Runtime 使用
`@harrisonwang/spoor-wasm`。表格筛选（`sheet`/`rows`/`columns`/`limit`/`offset`）、
PDF 页码筛选（`pages`）与内嵌媒体提取（`extract_media`）在 CLI、Python、Node、WASM
行为等价，嵌入式调用可直接分页拉取整张表或只取 PDF 指定页。PDF 默认解析全部页；
`stats.page_count` 始终报告总页数（即便只取了某几页），所以可以用 `--pages 1:1`
廉价地"探一眼"页数，再决定要不要、要哪段。`--provenance page`（各绑定为 `provenance`
选项）返回每页"输出 markdown 字节区间 → 源页码"的来源定位映射，便于把 LLM 引用锚定
回原文页；`--provenance block` 进一步细到行级，born-digital 行附 PDF 原生坐标
包围盒（可直接交给 PDF.js 画高亮框），被改写或几何不可信的字节保持页级锚定；
默认关闭。从 `v0.8.3` 起，发布的
WASM 包默认包含全部重点格式；
需要更小体积时可自行构建 `core-formats`。

解析产物还可以直接核验模型引文：`spoor_core::locate_quote(markdown, quote)`
（Python/Node/WASM 同名导出）在 spoor 产出的 Markdown 里定位模型引用的原文，
依次尝试四档确定性匹配——精确子串、忽略空白、表格单元格锚点（模型引用表格数据时
常把"列名 行名 数值"重组成原文中并不连续的一句话）、数值/单位换算
（7771 亿 = 777102 百万）。命中返回区间、上下文、页码和所用档位；四档全部落空
说明原文没有这句话，调用方应把对应结论按"无法核验"处理，而不是相信模型的自引。
Rust 的 `span` 是 UTF-8 字节区间，Python/Node/WASM 返回各自语言的字符串下标，
直接切片即得命中原文；Rust 侧可用 `Locator` 对同一文档核验多条引文而只建一次索引。

主示例：

| 示例 | 展示能力 | 在线地址 |
| --- | --- | --- |
| [`examples/cloudflare-pages`](examples/cloudflare-pages/) | Cloudflare Pages 本地 WASM 演示 + Pages Functions 边缘 API | [`spoor-pages-demo.pages.dev`](https://spoor-pages-demo.pages.dev) |
| [`examples/local-corpus-explorer`](examples/local-corpus-explorer/) | 浏览器内混合文档批处理、跨文件检索与 JSONL 导出 | [`spoor-corpus-demo.pages.dev`](https://spoor-corpus-demo.pages.dev) |

集成形态：

| 示例 | 展示能力 |
| --- | --- |
| [`examples/cloudflare-worker`](examples/cloudflare-worker/) | 独立 Cloudflare Worker 文档解析 API |
| [`examples/tauri-desktop`](examples/tauri-desktop/) | 完整 Tauri 2 本地桌面应用 |
| [`examples/electron-desktop`](examples/electron-desktop/) | 使用原生 Node binding 的 Electron 桌面应用 |
| [`examples/rust-core-embed`](examples/rust-core-embed/) | 把 `spoor-core` 嵌入 Rust 程序的最小形态（无框架） |
| [`examples/serverless-lambda`](examples/serverless-lambda/) | AWS Lambda Layer 中调用 CLI 二进制 |
| [`examples/agent-spoor`](examples/agent-spoor/) | 把 spoor 接入 AI Agent 的三种方式：原生工具 / MCP Server / Skill（Node · Python · Rust） |

> [`wasm/demo`](wasm/demo/) 不是用户示例，而是 `spoor-wasm` 的浏览器开发/冒烟页：CI 用 `node wasm/demo/smoke.mjs` 跑全格式与恶意输入的 WASM 契约回归。

## 限制与边界

- core 默认单次共享解析内存上限为 64 MiB；CLI 默认总输出上限为 256 KiB。
- CSV/XLSX 默认仅返回每个表前 100 条数据行，完整读取需要使用筛选与分页参数。
- 不执行 OCR、宏、公式、notebook code、脚本或内嵌二进制；加密文件与旧版 Office 格式不支持。
- 浏览器和边缘示例额外采用 16 MiB 请求/单文件上限，并受宿主内存、CPU 与请求限制约束。

各格式保留内容、已知缺口、格式检测规则和每个示例的限制见
[能力与限制](docs/v1/design/limitations.md)。

## 错误契约

所有入口共享 `SpoorError`，消费者只按稳定 `code` 分支：

| code | 含义 |
| --- | --- |
| `pdf_no_extractable_content` | PDF 无文本层也无可提取图片，无内容可抽取 |
| `parse_budget_exceeded` | 输入、解压或结果超过解析内存上限 |
| `work_budget_exceeded` | 解析运算量（如 PDF 操作数）超过 `max_work_units` 上限 |
| `unsupported_format` | 无法识别或不支持格式 |
| `encrypted_pdf` | PDF 受密码保护 |
| `legacy_or_encrypted_office` | 旧版或加密 Office 容器 |
| `invalid_container` | ZIP 类容器为空、损坏或类型不符 |
| `parse_failed` | 已规范化的其他解析失败；查看 `stage` |

成功解析也可能包含完整性 warning。当前稳定 warning code：

| code | 含义 |
| --- | --- |
| `pdf_page_no_text_layer` | 混合 PDF 的某页没有可提取文本层 |
| `pdf_page_suspicious_text_layer` | 某页文本层包含明显可疑字符或 glyph 占位符 |
| `pdf_multi_column_reading_order` | 某页检测到多栏版面，已按列重排阅读顺序（几何推断，可能不完美） |
| `merged_table_structure_not_preserved` | DOCX/PPTX 合并单元格未被 GFM 表格完整保留 |
| `embedded_visuals_omitted` | DOCX/PPTX 中存在尚未被理解或未进入文本输出的视觉对象；DOCX/PPTX 内嵌栅格图片可能已有 `spoor://docx/part/` / `spoor://pptx/part/` 占位符，PDF 同理用 `spoor://pdf/obj/` |
| `vector_graphics_omitted` | PDF 某页含矢量绘制的图（流程图/图表/示意图），未转成文本输出；正文该页末尾附 `spoor://pdf/page/N` 链接，可 `--extract` 取该页 SVG 图交视觉模型 |
| `pdf_repeated_region_deduplicated` | PDF 跨页重复的页眉/页脚已去重，仅保留首次出现；`--keep-repeated-regions`（各绑定为 `keep_repeated_regions` 选项）可保留逐字原文 |

warning 可带 `location: {kind: "page" | "slide", number}`。CLI 会同时在 stderr 和
Markdown stdout 尾部显示这些 warning，避免只读 stdout 的 Agent 静默忽略。

## 开发

```bash
# 构建
cargo build --release

# 测试
cargo test --locked --workspace --all-targets

# 代码检查
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings

# 快照测试（insta）
cargo insta review
```

## 文档

| 文档 | 内容 |
| --- | --- |
| [能力决策与演进规划](docs/v1/design/capabilities.md) | Agent 场景、调研结论、立即实现项、后续路线与明确边界 |
| [定位与工程规划](docs/v1/planning/overview.md) | 一句话定位、设计原则、交付形态、推进顺序 |
| [路线图与竞品分析](docs/v1/planning/roadmap.md) | 竞品调研、平台约束、差异化自查 |
| [架构设计](docs/v1/design/architecture.md) | Core 边界、错误契约、PyO3 接口、迁移顺序 |
| [工程决策](docs/v1/design/decisions.md) | 产品边界、输出模式、格式取舍、安全策略 |
| [能力与限制](docs/v1/design/limitations.md) | 文件大小、格式保留内容、运行形态和宿主限制 |
| [测试矩阵](docs/v1/test-matrix/) | 按格式维护的测试覆盖 |
| [安全模型](SECURITY.md) | 威胁、默认防御、边界与结构化错误 |
