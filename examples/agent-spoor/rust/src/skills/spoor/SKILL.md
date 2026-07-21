---
name: spoor
description: 在本地读取 PDF、DOCX、XLSX、CSV、PPTX、EPUB、HTML 等文件。文档输出 Markdown，表格输出 JSON；可以只读取指定页、工作表、行或列，也可以提取文档中的图片。
---

# 使用 spoor 读取文档

当用户要读取、总结或提取 PDF、Word、Excel、PPT、EPUB 或网页文件时，用 `run_shell` 调用 `spoor` CLI。

## 基本用法

- 读取整份文档：`spoor data/byd.pdf`
- 查看表格结构和前几行：`spoor data/sales.csv`
- 只读取 PDF 的几页：`spoor data/byd.pdf --pages 1:3`
- 指定 XLSX 的工作表、列和行数：`spoor data/book.xlsx --sheet Sheet1 --columns 分类,金额 --limit 20`
- 行区间（与 --limit/--offset 互斥）：`spoor data/sales.csv --rows 2:4`

## 输出怎么读

- 文档返回 Markdown。表格返回 JSON，其中包含表头、实际返回的行和是否截断。
- 结尾可能有警告。例如，`pdf_page_no_text_layer` 表示某页没有可提取文字。必须告诉用户，不要假装读到了这一页。

## 提取图片

- 正文里出现 `![...](spoor://...)` 链接时，用 `spoor <文件> --extract <spoor://...>` 提取。
  `run_shell` 会把图片存到 `.spoor-media/`。spoor 本身不识别图片内容，需要时可以交给视觉模型。

## 出错怎么办

- 根据错误 `code` 处理，并把 `hint` 告诉用户：`unsupported_format`、`encrypted_pdf`、`parse_budget_exceeded`（文件或中间结果超过大小限制，应只读取需要的部分）等。
