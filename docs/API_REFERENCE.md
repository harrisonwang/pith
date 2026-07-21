# 接口参考

把文件内容、文件名和可选参数传给 spoor。成功时返回 `ParseResult`，失败时返回带固定 `code` 的 `SpoorError`。

## CLI

```text
spoor [OPTIONS] <input>...
```

常用参数：

| 参数 | 作用 |
| --- | --- |
| `--format <format>` | 无法自动识别时，手动指定输入文件的格式 |
| `-m, --mode <md\|json>` | 手动指定返回 Markdown 还是 JSON；JSON 仅用于表格 |
| `--pages <first:last>` | PDF 页或 PPTX 幻灯片范围，从 1 开始且包含两端 |
| `--sheet <name>` | 只读一个 XLSX 工作表 |
| `--rows <first:last>` | 表格行范围；与 `--limit/--offset` 互斥 |
| `--columns <a,b>` | 按表头保留指定列 |
| `--limit <n>` / `--offset <n>` | 表格分页 |
| `--provenance <page\|block>` | 返回页码或更细的坐标；仅支持单文件，与 `--mode` 互斥 |
| `--extract <spoor://...>` | 取出链接对应的图片或其他内容；仅支持单文件 |
| `--max-parse-mib <n>` | 调整单次解析的大小上限，默认 64 MiB |
| `--max-work-units <n>` | 限制单次解析的工作量，默认不限制 |
| `--max-output-kib <n>` | 调整 CLI 输出上限，默认 256 KiB |

完整列表以当前版本的 `spoor --help` 为准。

## Rust

```bash
cargo add spoor-core
```

```rust
use spoor_core::{ParseContent, ParseRequest, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read("report.pdf")?;
    let mut request = ParseRequest::new(&bytes);
    request.source_name = Some("report.pdf");

    let result = parse(&request)?;
    match result.content {
        ParseContent::Document(document) => println!("{}", document.markdown),
        ParseContent::Tables(tables) => println!("{:?}", tables.tables),
    }
    Ok(())
}
```

主要函数：

| 函数 | 用途 |
| --- | --- |
| `parse` | 推荐使用；自动区分文档和表格，并返回警告、统计信息，以及需要时生成的页码和坐标 |
| `parse_document_result` | 按文档处理，同时保留警告 |
| `parse_document` | 旧接口；只返回文档内容，不返回单独的警告列表 |
| `parse_tables` | 按表格处理 |
| `detect_format` | 只检测格式 |
| `extract_media` | 按 `spoor://` 链接取出图片或其他内容 |
| `locate_quote` | 按五种规则查找引文或相关数据 |
| `locate_quote_grounded` | 查找引文或相关数据，并返回页码或坐标 |
| `Locator` | 适合连续查找多条引用，索引只建立一次 |

## Python

```bash
pip install pyspoor
```

```python
from pathlib import Path
from spoor import parse_bytes, parse_path

pdf = parse_path("report.pdf", pages=(1, 3), provenance="block")
xlsx = parse_bytes(
    Path("data.xlsx").read_bytes(),
    source_name="data.xlsx",
    sheet="Sheet1",
    limit=50,
)
```

Python 包提供 `parse_path`、`parse_bytes`、`detect_format`、`extract_media` 和 `locate_quote`。返回值附有 Python 类型标注；底层错误会转成 `SpoorError`。

## Node.js

```bash
npm install @harrisonwang/spoor
```

```js
const fs = require('node:fs');
const { parseBytes } = require('@harrisonwang/spoor');

const result = parseBytes(fs.readFileSync('report.pdf'), {
  sourceName: 'report.pdf',
  pages: [1, 3],
  provenance: 'block',
});

console.log(result.content.value.markdown);
```

可以使用 `parseBytes`、`detectFormat`、`extractMedia` 和 `locateQuote`。错误对象会附带 `code`、`reason`、`hint`、`recoverable` 和可选的 `stage`。

## WASM

```bash
npm install @harrisonwang/spoor-wasm
```

```js
import { parse_bytes } from '@harrisonwang/spoor-wasm';

const input = document.querySelector('input[type="file"]');
input.addEventListener('change', async () => {
  const file = input.files[0];
  const bytes = new Uint8Array(await file.arrayBuffer());
  const result = parse_bytes(bytes, file.name, file.type || undefined);
  console.log(result.content.value.markdown);
});
```

WASM 还导出 `detect_format`、`extract_media` 和 `locate_quote`。调用 `parse_bytes` 时，参数应按声明顺序传入；完整签名以包内 TypeScript 声明为准。

浏览器和 Cloudflare Worker 不能直接读取本地路径。你需要先读出文件字节、提供 `source_name`，并限制内存、执行时间和并发数。

## ParseResult

返回结果大致如下：

```json
{
  "content": {
    "kind": "document",
    "value": {
      "source": "report.pdf",
      "format": "pdf",
      "markdown": "..."
    }
  },
  "warnings": [],
  "stats": {
    "input_bytes": 1234,
    "output_bytes": 5678,
    "format": "pdf",
    "page_count": 12
  },
  "provenance": {
    "spans": []
  }
}
```

`content.kind` 只有两种：

- `document`：`content.value.markdown` 是解析后的 Markdown。
- `tables`：`content.value.tables` 是表格数组。

每张表都会说明来自哪个文件、哪个工作表，包含哪些表头，实际返回了哪些行，以及是否被截断。XLSX 还可能返回工作表列表、标题、表头所在行，以及表头之前的说明。

## 通用筛选参数

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `source_name` / `sourceName` | 字符串 | 文件名或网址；文件头不足以判断格式时会用到 |
| `content_type` / `contentType` | 字符串 | 可选 MIME 类型 |
| `format` | 字符串 | 手动指定文件格式 |
| `sheet` | 字符串 | XLSX 工作表 |
| `rows` | 两个整数 | 从 1 开始、包含两端的行范围 |
| `columns` | 字符串数组 | 按表头筛选列 |
| `limit` / `offset` | 整数 | 表格分页 |
| `pages` | 两个整数 | PDF 页或 PPTX 幻灯片范围 |
| `provenance` | `page` 或 `block` | 返回页码或更细的坐标；默认表格 JSON 不返回单元格位置 |
| `keep_repeated_regions` / `keepRepeatedRegions` | 布尔值 | 保留 PDF 重复页眉页脚 |
| `max_parse_bytes` / `maxParseBytes` | 整数 | 单次处理的大小上限 |
| `max_work_units` / `maxWorkUnits` | 整数 | 单次解析工作量上限；不能代替强制超时 |

## 警告

出现警告时，返回的内容仍可使用，但可能有内容没读出来，文字顺序也可能经过调整。程序应根据 `code` 处理；`message` 只用于展示。

| code | 含义 |
| --- | --- |
| `pdf_page_no_text_layer` | PDF 某页没有可提取文字 |
| `pdf_page_suspicious_text_layer` | PDF 某页文本层包含明显异常字符 |
| `pdf_multi_column_reading_order` | 检测到多栏，并根据文字坐标重新排序；结果可能不准确 |
| `merged_table_structure_not_preserved` | 合并单元格没有被 Markdown 表格完整保留 |
| `embedded_visuals_omitted` | 仍有未识别的图片、图形或嵌入对象 |
| `vector_graphics_omitted` | PDF 页含未转成文本的矢量图形 |
| `pdf_repeated_region_deduplicated` | 重复页眉页脚已去重 |
| `slide_no_text_layer` | 幻灯片除标题外没有正文文字；如果同时出现 `embedded_visuals_omitted`，说明提取出的正文不足以代表该页 |
| `hidden_slide_omitted` | 隐藏幻灯片已省略，幻灯片编号仍保留 |

位置字段为 `{kind: "page" | "slide", number: N}`。

## 错误码

| code | 含义 |
| --- | --- |
| `pdf_no_extractable_content` | PDF 没有文字层，也没有可提取图片 |
| `parse_budget_exceeded` | 输入、解压内容或中间结果超出大小上限 |
| `work_budget_exceeded` | 单次解析工作量超过设定上限 |
| `unsupported_format` | 无法识别或不支持格式 |
| `encrypted_pdf` | PDF 受密码保护 |
| `legacy_or_encrypted_office` | 旧版二进制或加密 Office 文件 |
| `invalid_container` | ZIP 格式的文档包为空、损坏或内部结构不符 |
| `parse_failed` | 其他解析错误；结合 `stage` 排查 |

不要根据错误消息的文字判断错误类型。
