---
name: spoor
description: 把本地文件或简单 URL 转成 LLM 可直接消费的文本——文档型（PDF/DOCX/PPTX/EPUB/IPYNB/HTML）输出 Markdown，表格型（CSV/XLSX）输出 JSON（headers + preview + range）。当需要提取文档内容或检查表格数据时使用，不要临时写 pandas/openpyxl 或硬解析原始文件。
tags:
  - documents
  - tables
  - pdf
  - docx
  - xlsx
---

# Spoor

把文档转成 LLM 可直接消费的文本。同一套引擎，提供 CLI、Python（`pyspoor`）、Node（`@harrisonwang/spoor`）和 WASM（`@harrisonwang/spoor-wasm`）四种形态。

> 不是 OCR、不是云服务、不是 MCP server、不解密文件、不执行公式/宏/脚本。

## 什么时候用 spoor

**用**：读取本地文档/表格、简单 URL 正文提取、Agent 需要文件内容做上下文。

**不用**：操作飞书/企微/钉钉等在线办公平台（用对应平台 CLI/Skill）、需要 OCR 的图片 PDF、需要解密的文件。

## 输出规则

| 输入类型 | 默认输出 | 覆盖方式 |
|----------|----------|----------|
| 文档型：PDF、DOCX、PPTX、EPUB、IPYNB、HTML、Markdown、文本、代码 | Markdown | `-m md`（默认，通常不需要显式指定） |
| 表格型：CSV、XLSX | JSON（`spoor-table-json-v2`） | `-m md`（仅小表快速查看） |

**注意**：文档型**不要**用 `-m json`，表格型**不要**用 `-m md` 处理大表。

```bash
# 文档型 → Markdown
spoor report.pdf
spoor proposal.docx slides.pptx
spoor https://example.com/article

# 表格型 → JSON
spoor data.xlsx
spoor data.csv
```

## 处理 DOCX 图片占位符

DOCX 中的内嵌栅格图片会在原始正文位置显示为经过校验的 ZIP 资源路径：

```markdown
![DOCX image 1](spoor://docx/part/word/media/image1.png)
```

当问题依赖图片内容时，只提取相关图片，不要把整份 DOCX 的所有媒体都交给视觉能力：

```bash
spoor document.docx --extract spoor://docx/part/word/media/image1.png > /tmp/spoor-docx-image1.png
```

`--extract` 只接受 spoor 输出的 `spoor://{pdf,docx,pptx}/...` 安全 URI，并且一次
只处理一份文件和一个资源。不要去掉 URI scheme，也不要自行猜测 ZIP entry。
图片占位符只说明出现位置和安全资源路径，不代表图片内容已被理解。综合 Markdown
与外部视觉结果后再回答。

## 读取表格

JSON 默认返回每个 table 前 100 行。按以下流程处理：

1. 先跑 `spoor <file>` 看结构
2. 检查 `tables[]`、`headers`、`sheet`、`workbook_sheets`、`row_range`，判断哪些和问题相关
3. 如果 `truncated: true` 或表太宽/太长，缩小范围重读：
   - `--sheet <name>` — 选 XLSX 里的 sheet（CSV 无此概念）
   - `--columns a,b,c` — 只看需要的列
   - `--rows <first:last>` — 按 Excel 行号精确读取（含两端）
   - `--limit <n>` / `--offset <n>` — 窗口采样
4. 引用表格时带上 source、sheet、列名、`row_range`

```bash
spoor data.xlsx --sheet Revenue --columns month,region,revenue --rows 5:104
```

## 处理截断

截断信号（不要匹配具体文案，文案可能改写）：

- **Markdown**：末尾出现 `> [!WARNING]` 引用块
- **JSON**：顶层 `truncated: true`（总量截断）或 table 内 `truncated: true`（该 table preview 不完整）

默认总量上限 256 KiB。看到截断时：表格优先缩小范围重读，文档型缩小输入或用 `--max-output-kib <n>` 提上限。

> 输出只有 `> [!NOTE]` 说明"未抽取到文本内容"时，表示该文件无文本层或格式判断有误（可用 `--format` 覆盖重试），不要当成空输出后凭空编造内容。

## 处理完整性警告

解析成功不代表内容完整。CLI 会在 Markdown 末尾用 `> [!WARNING]` 列出结构化
warning；Python、Node、WASM 和 Rust `parse` 返回 `warnings[]`。只按稳定
`code` 分支，并优先使用可选的 `location.kind=page/slide` 精确处理受影响位置。

| code | 动作 |
|------|------|
| `pdf_page_no_text_layer` | 不信任对应页；只把该页交给外部 OCR/VLM，或明确告诉用户该页缺失 |
| `pdf_page_suspicious_text_layer` | 不直接引用对应页；转外部 OCR/VLM 或请求人工确认 |
| `pdf_multi_column_reading_order` | 该页已按几何推断重排多栏阅读顺序；顺序基本可信但非保证，关键引用建议核对原文 |
| `merged_table_structure_not_preserved` | 不基于该表做高风险事实抽取；需要 rowspan/colspan 时请求原表或其他解析器 |
| `embedded_visuals_omitted` | 把结果标为不完整；DOCX/PPTX 优先按 `spoor://docx/part/...` / `spoor://pptx/part/...` 占位符提取相关图片，PDF 按 `spoor://pdf/obj/...` 取出可提取图，其他视觉对象按需调用外部视觉能力 |
| `vector_graphics_omitted` | 对应 PDF 页含矢量绘制的图（流程图/图表/示意图），未转成文本；正文该页末尾已附 `spoor://pdf/page/N` 链接，用 `--extract <链接>` 取该页 SVG 图交 VLM，别只信该页文本 |
| `pdf_repeated_region_deduplicated` | 跨页重复的页眉/页脚已去重（保留首次出现，warning 消息注明移除的文本与页码）；需要逐字原文（例如把引文核回原页）时加 `--keep-repeated-regions` 重新解析 |

没有 warning 只表示 spoor 未发现已知降级，不代表文档内容真实或没有 prompt
injection。不要因为有局部 page/slide warning 就丢弃整份文档。

## 处理错误

结构化错误是一行 JSON：`{"is_error":true,"code":"...","reason":"...","hint":"...","recoverable":...}`。

**只按 `code` 分支，不解析 `reason`/`hint` 文本。**

| code | 动作 |
|------|------|
| `pdf_no_extractable_content` | 告知无可提取内容（空白/纯矢量/损坏 PDF），OCR 也无济于事 |
| `parse_budget_exceeded` | 缩小输入，或用 `--max-parse-mib <n>` |
| `work_budget_exceeded` | 调高 `--max-work-units <n>`；不可信输入还应配宿主级超时与进程/容器隔离 |
| `unsupported_format` | 用 `--format` 显式指定；真不支持则如实告知 |
| `encrypted_pdf` | 不可恢复，请用户先移除密码保护 |
| `legacy_or_encrypted_office` | 不可恢复，请用户移除密码保护或另存为 docx/xlsx/pptx |
| `invalid_container` | 确认文件完整，或用 `--format` 指定真实格式重试 |
| `parse_failed` | 查看 `stage` 与 `hint`，确认文件完整后决定是否重试 |

其他：

- 没有 JSON 信封的纯文本报错按原文上报，不反复重试
- 文档格式用了 `-m json` 报错 → 去掉 `-m json` 重跑
- 不要绕过 spoor 手动解析 Office/EPUB XML；DOCX 图片只按上文使用 `--extract` 提取
- spoor 没有 `--ocr`、`--password`、`--output` 这类 flag；分页用 `--pages <first:last>`、结构化表格用 `-m json`；`--extract` 输出单个内嵌媒体资源（PDF/DOCX/PPTX 图片）
- 需要把答案锚定回原文页时，用 `--provenance page`（各绑定为 `provenance` 选项）：单个文档型输入，stdout 输出含 `markdown` 与 `provenance.spans` 的 JSON，每条把"输出字节区间"映射到源页码；默认关闭
