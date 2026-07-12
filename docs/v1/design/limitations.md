# 能力与限制

本文是 spoor `v0.9.1` 的能力边界。它描述代码当前实际行为，而不是未来路线图。

## 总体资源限制

| 限制 | 默认值 | 作用范围 | 说明 |
| --- | ---: | --- | --- |
| core 共享解析内存上限 | 64 MiB | 单次 `ParseRequest` | 输入、ZIP 声明总解压量、中间文本与保留结果共享预算；最小可配置值 1 KiB |
| CLI 共享解析内存上限 | 64 MiB | 整次命令 | 多个输入共同消耗，不是每个文件各 64 MiB |
| CLI stdout 上限 | 256 KiB | 整次命令 | Markdown 会附截断标记；表格 JSON 保持合法并写入 warning；最小可配置值 1 KiB |
| ZIP 条目数 | 10,000 | DOCX/PPTX/EPUB 等 ZIP 容器 | 不提供公开覆写接口 |
| ZIP 单条目解压大小 | 50 MiB | 单个 ZIP entry | 仍受更小的共享解析内存上限约束 |
| ZIP 压缩比 | 200x | 单个 ZIP entry | 超出即拒绝 |
| ZIP 声明总解压量 | 共享解析内存上限 | 整个 ZIP | 在真正解压前检查中央目录 |
| 运算量上限 `max_work_units` | 默认无（可选） | 目前 PDF 内容流操作 | 合作式上限，约束字节上限管不到的 CPU；超限返回 `work_budget_exceeded`。不是可强制中断的超时 |
| core 进程内超时 | 无 | 所有原生/WASM 调用 | 合作式 `max_work_units` 只能在循环边界中止；真正的 wall-clock 取消仍需调用方在 worker、容器或独立进程设置 |
| 严格 RSS 上限 | 无 | 所有原生调用 | 数据量预算不等于操作系统内存上限；第三方解析器与跨语言传输可能产生额外副本 |

解析复杂度不只由压缩文件大小决定。小型 PDF、XLSX 或高压缩比 Office 文件也可能
比同等字节数的纯文本消耗更多 CPU 和内存。

## 重点格式

| 格式 | 当前保留 | 当前明确不保留或限制 |
| --- | --- | --- |
| DOCX | 标题 1-6、段落、粗体/斜体、列表、小型 GFM 表格、链接、脚注、Unicode、插入型 tracked changes；内嵌栅格图片以安全 `spoor://docx/part/word/media/*` 占位符保留正文位置，CLI 可通过 `--extract` 提取单个占位符资源；合并表格/视觉对象会返回 warning | 不理解或批量导出图片；不保留样式与版式、删除型 tracked changes、comments/endnotes、复杂编号重启、复杂合并表格、图表与嵌入对象 |
| XLSX | sheet、range、标题/表头/preamble、文本/数字/布尔/日期、缓存公式结果、错误单元格、合并单元格左上值 | 默认每个 sheet 仅前 100 条数据行；不计算公式、不保留公式表达式/样式/图表；Excel 1904 date system 尚未完整处理；一个 sheet 按一个逻辑 table 输出 |
| PDF | text layer、页顺序、`## Page N` 边界、`--pages` 页码区间、`stats.page_count` 总页数（不随切片变化）；`--provenance page` 返回每页"输出字节区间 → 源页码"的来源定位映射；对清晰双栏版面基于字形几何重排阅读顺序并返回 `pdf_multi_column_reading_order` warning（保守、可回退）；混合文档的无文本页与明显可疑文本层返回带页码 warning | 默认解析全部页；不做 OCR；不恢复标题语义；来源定位当前仅页级（无 bbox）；多于两栏或复杂版面不保证阅读顺序（保守判定为单栏，回退原始顺序）；不去重页眉页脚、不修复断词；无文本且无图片的 PDF（空白/纯矢量）与加密 PDF 返回结构化错误 |
| PPTX | 按数字 slide 顺序输出文本、小型表格、speaker notes；内嵌栅格图片以安全 `spoor://pptx/part/ppt/media/*` 占位符保留正文位置，CLI 可通过 `--extract` 提取单个占位符资源；合并表格/视觉对象省略会返回带 slide 位置 warning | 不按 shape 坐标恢复视觉阅读顺序；不保留 bullet 层级、主题、动画、图表或嵌入对象 |
| HTML / URL | 优先 `article`、其次 `main`、最后 `body`；标题、段落、列表、链接、表格、引用块、代码块、image alt、粗体/斜体；跳过常见导航与脚本噪声 | 不是完整 readability 引擎；不解析相对链接；caption 与嵌套列表仍有限；core 不抓 URL，只有 CLI 会发起网络请求 |
| EPUB | OPF spine 阅读顺序、章节边界，并复用 HTML Markdown renderer | 不处理 DRM、固定版式视觉结构、图片/音视频、复杂导航与 CSS 布局 |
| IPYNB | markdown cell、code cell、cell 顺序、kernelspec language hint | 从不执行代码；跳过 raw cell、outputs、widgets、HTML output 与 base64 图片 |

## 基础格式

- CSV/TSV：第 1 行视为 header，自动识别常见 delimiter 与编码；默认只返回前
  100 条数据行，不做强类型推断。
- Markdown、纯文本、常见代码与配置文件：做字符集解码后保留文本；不是语法分析器。
- 不支持旧版二进制 Office `.doc` / `.xls` / `.ppt`、密码保护 Office、OCR、
  宏执行、公式执行、notebook 执行、脚本执行或通用内嵌二进制提取。
- 内嵌媒体提取（DOCX 图片占位符、可直出的 PDF 图片）通过格式无关的 `extract_media`
  入口按安全 URI 取单个资源，CLI（`--extract`）、Python / Node（`extract_media`）与
  浏览器/WASM（`extract_media`）行为一致；spoor 只暴露经校验的安全 URI，不解码或理解字节。

## 格式检测限制

检测顺序大致为：

1. PDF、ZIP、OLE/CFB 等 magic bytes；
2. ZIP 内部结构区分 DOCX/XLSX/PPTX/EPUB；
3. 文件名或 URL 扩展名；
4. URL 的部分 `content-type` 辅助判断；
5. 可解码文本回退为纯文本。

`source_name` / `x-filename` 很重要。没有扩展名的 CSV、IPYNB、Markdown 与普通
JSON/文本可能无法可靠区分；错误扩展名或错误 `content-type` 也可能导致错误分派。
对 stdin 中的 CSV 应显式传 `--format csv`。OLE/CFB 会在扩展名回退前被拒绝，
避免把旧版或加密 Office 当成普通文本。

## 运行形态与示例限制

| 形态 / 示例 | spoor 限制 | 额外宿主限制 |
| --- | --- | --- |
| CLI | 默认整次命令 64 MiB 解析内存上限、256 KiB stdout；URL 读取 30 秒 timeout | URL 抓取不是爬虫：未提供 host allowlist、robots、登录态或完整 redirect 策略 |
| Rust / Python / Node | 默认每次调用 64 MiB；无输出封顶、无进程内 timeout | 在调用方进程内执行；恶意多租户输入应使用进程或容器隔离 |
| 默认发布 WASM | `v0.8.3` 起包含全部重点格式；默认每次调用 64 MiB | 浏览器/WASM 线性内存、JS heap 和 bytes/字符串副本会降低实际可用上限 |
| Cloudflare Worker / Pages Function 示例 | 显式拒绝超过 16 MiB 的请求并把解析内存上限设为 16 MiB | Worker isolate 内存 128 MB，包含 JS 与 WASM；Free HTTP CPU 10 ms，Paid 默认 30 s、最高 5 min；公开演示没有认证、限流或租户隔离 |
| Cloudflare Pages 本地模式 | 显式 16 MiB 单文件上限 | 文件留在浏览器，但仍受浏览器内存与主线程响应性约束 |
| 本地语料库示例 | 16 MiB/文件；局部失败不阻断批次 | 没有文件数量、总字节数、总输出或取消上限；会同时保留原文件、解析结果和 chunk，不能当作无限语料库 |
| Electron 示例 | 显式 64 MiB/文件 | 解析在主进程内；生产应用应使用 Utility Process/worker 做隔离 |
| Tauri 示例 | core 默认 64 MiB/文件 | 当前 `Array.from(Uint8Array)` 到 `Vec<u8>` 会复制数据，不适合超大文件 |
| Lambda 示例 | CLI 默认 64 MiB 解析内存上限、256 KiB 输出 | 同步请求/响应各 6 MB，异步 payload 1 MB，最长 15 分钟；大文件应通过 S3 |

Cloudflare 官方当前还限制 Worker 压缩后体积为 Free 3 MB / Paid 10 MB，Pages
单个静态资源最大 25 MiB。CI 会对 `full` 与 `core-formats` WASM 都执行 3 MiB gzip
门禁。平台限制会变化，部署前应重新检查官方文档：

- [Cloudflare Workers limits](https://developers.cloudflare.com/workers/platform/limits/)
- [Cloudflare Pages limits](https://developers.cloudflare.com/pages/platform/limits/)
- [AWS Lambda quotas](https://docs.aws.amazon.com/lambda/latest/dg/gettingstarted-limits.html)

## 内容与安全边界

- spoor 只提取现有文本和结构，不判断事实真伪，也不会识别 prompt injection。
- 文档中的恶意指令会作为普通文本进入输出；交给 LLM 前必须按不可信数据处理。
- “本地解析”只表示 spoor 不主动上传；宿主应用、浏览器扩展、日志、剪贴板和后续
  LLM 调用仍可能泄露内容。
- `ParseResult` 的 `warnings`、表格的 `truncated` / `warnings` 和稳定错误 `code`
  是调用方必须处理的契约，不能只判断调用是否成功。
- Agent 应使用 `parse` 或 `parse_document_result` 获取文档完整性 warnings；
  `parse_document` 只返回 Markdown，兼容调用会丢弃 warnings。
- 当前文档 warning code 为 `pdf_page_no_text_layer`、
  `pdf_page_suspicious_text_layer`、`merged_table_structure_not_preserved` 和
  `embedded_visuals_omitted`；位置使用 `location.kind=page/slide`。
- 边缘公开示例用于展示能力，不是生产文档服务。生产部署至少需要认证、限流、
  请求审计、内容保留策略、超时与并发隔离。

## 当前能力短板

近期最值得增强的不是继续增加格式数量，而是：

1. 在已落地的 PDF 页级来源定位之上，做块级来源定位（段落 `bbox`），再扩展到纯文本/表格的输入区间与单元格锚点；OCR 保持外置。
2. 在已落地的 PDF 布局中间模型与双栏阅读顺序之上，继续做页眉页脚分类、标题层级与断词修复。
3. 建立 DOCX/PPTX 表格 span 模型，把合并表格从“显式 warning”升级为 HTML 降级输出。
4. 为 PPTX 按 shape 坐标恢复阅读顺序并保留 bullet 层级。
5. 把运算量上限（`max_work_units`）从 PDF 扩展到 XML/表格等其余解析器，并由批处理宿主补上可真实终止的 wall-clock 超时、取消和进程/容器隔离。

PDF 布局中间模型（基于字形几何收集 span）已落地，并已用于清晰双栏版面的阅读顺序
重排（保守判定、失败回退原始顺序、返回 `pdf_multi_column_reading_order` warning）。

来源定位（provenance）页级已落地：`--provenance page` 与各绑定的 `provenance` 选项返回
“输出 markdown 字节区间 → 源页码”的映射，默认关闭、确定性、无 ML；块级坐标见
[docs/v1/design/provenance.md](provenance.md)。

表格分页筛选（`sheet`/`rows`/`columns`/`limit`/`offset`）、PDF 页码筛选（`pages`）
与内嵌媒体提取（`extract_media`）现已在 CLI、Python、Node、WASM 全部贯通，四个宿主
共用 `TableFilter::build` / `DocumentFilter::build` 校验与 `parse` 契约，行为等价；
嵌入式调用可直接分页拉取整张表、或只解析 PDF 的指定页，不再被默认 100 行预览、整份
PDF 解析或 CLI 子进程限制。

完整能力决策与先后顺序见 [docs/capabilities.md](../../capabilities.md)。
