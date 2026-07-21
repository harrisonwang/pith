---
name: spoor
description: 在本地读取 PDF、DOCX、XLSX、CSV、PPTX、EPUB、HTML 等文件。文档输出 Markdown，表格输出 JSON；可以只读取指定页、工作表、行或列，也可以提取文档中的图片。
tags:
  - documents
  - tables
  - pdf
  - docx
  - xlsx
---

# 使用 spoor 读取文档

需要读取或总结 PDF、Word、Excel、PPT、EPUB 或网页文件时，调用 `spoor` CLI。

spoor 不做 OCR，不解密文件，不执行公式、宏、脚本或 Notebook 中的代码。

## 基本命令

```bash
# 文档 → Markdown
spoor report.pdf
spoor proposal.docx slides.pptx
spoor report.pdf --pages 1:3

# 表格 → JSON
spoor data.xlsx
spoor data.xlsx --sheet Revenue --columns month,revenue --rows 5:104
```

表格默认只返回每张表前 100 条数据行。先检查 `headers`、`sheet`、`row_range` 和 `truncated`，再用 `--sheet`、`--columns`、`--rows`、`--limit` 或 `--offset` 读取需要的部分。

## 必须处理警告

命令成功不代表内容完整。检查 Markdown 末尾或返回结果中的警告，并告诉用户哪些页或幻灯片可能有遗漏。
Markdown 中固定格式的警告以 `> [!WARNING]` 开头；JSON 使用 `truncated` 字段表示截断。

| code | 处理方式 |
| --- | --- |
| `pdf_page_no_text_layer` | 对应页没有文字；不要假装读到了内容，必要时交给 OCR 或视觉模型 |
| `pdf_page_suspicious_text_layer` | 不直接引用该页，改用 OCR、视觉模型或人工复核 |
| `pdf_multi_column_reading_order` | 多栏文字已按位置重新排序，关键引文仍应核对原页 |
| `merged_table_structure_not_preserved` | 合并单元格结构没有完整保留；涉及金额等关键信息时，回到原文件核对 |
| `embedded_visuals_omitted` | 结果可能漏掉图片或图形；有对应 `spoor://` 链接时再提取，没有链接时应说明限制 |
| `vector_graphics_omitted` | 用页末 `spoor://pdf/page/N` 提取 SVG，再交给视觉模型 |
| `pdf_repeated_region_deduplicated` | 如需逐字页内容，用 `--keep-repeated-regions` 重读 |
| `slide_no_text_layer` | 幻灯片除标题外没有正文文字；结合其他警告判断是否需要查看原幻灯片 |
| `hidden_slide_omitted` | 隐藏页已省略，但幻灯片编号仍与原文件一致 |

可以根据这两个警告判断每页 PPTX 应该怎样处理（标题占位符不算正文）：

| 该页出现的警告 | 应该怎么理解 | 处理方式 |
| --- | --- | --- |
| 无 | 未检测到需要报告的视觉遗漏；不能据此断定是纯文字页 | 使用已读出的文字，重要内容仍应结合原文件确认 |
| 仅 `embedded_visuals_omitted` | 已有正文，但仍有图片或图形内容可能遗漏 | 需要图内信息时，按 `spoor://` 链接提取图片并交给视觉模型 |
| 两者都有 | 除标题外没有正文，主要信息在视觉对象中 | 有链接时提取对应内容，否则查看原幻灯片 |

## 提取图片

正文出现 `![...](spoor://...)` 时，只提取与问题相关的图片：

```bash
spoor document.docx --extract spoor://docx/part/word/media/image1.png > /tmp/spoor-image.png
```

只使用 spoor 输出的 `spoor://` 链接，不自行猜测压缩包内路径。spoor 返回图片字节，但不识别图片内容。

## 分批读取和查找出处

- Markdown 末尾出现截断警告，或 JSON 的 `truncated` 为 `true` 时，先指定页、行或列，再重新读取。
- 需要查找回答的出处时，用 `--provenance page` 或 `--provenance block`。该选项只接受一个输入文件，并返回 JSON。

## 处理错误

只按固定的 `code` 处理，不要匹配自然语言消息：

| code | 处理方式 |
| --- | --- |
| `pdf_no_extractable_content` | 说明该 PDF 没有可用的文字或可提取图片 |
| `parse_budget_exceeded` | 只读取需要的页、行或列，必要时再合理调高 `--max-parse-mib` |
| `work_budget_exceeded` | 调高单次解析的工作量上限；处理不可信文件时，还要限制执行时间并使用独立进程或容器 |
| `unsupported_format` | 必要时用 `--format` 指定；确实不支持时，直接告诉用户 |
| `encrypted_pdf` | 请用户先移除 PDF 密码 |
| `legacy_or_encrypted_office` | 请用户解密或另存为 docx/xlsx/pptx |
| `invalid_container` | 检查文件是否完整、扩展名是否正确 |
| `parse_failed` | 结合 `stage` 和 `hint` 决定是否重试 |
