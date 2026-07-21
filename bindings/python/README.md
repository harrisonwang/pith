# pyspoor

在 Python 应用中解析文件内容，提示可能遗漏的部分，并查找模型引用。它和 CLI 共用同一套 Rust 解析代码，支持 Python 3.9 及以上版本。

```bash
pip install pyspoor
```

```python
from spoor import locate_quote, parse_path

result = parse_path("report.pdf", pages=(1, 3), provenance="block")

for warning in result.warnings:
    print(warning["code"], warning.get("location"))

markdown = result.content.value.markdown
found = locate_quote(markdown, "需要查找的引文", result.provenance)
```

表格可以使用 `sheet`、`rows`、`columns`、`limit` 和 `offset` 筛选；PDF/PPTX 使用 `pages`。`extract_media(data, uri, ...)` 可以通过解析结果中的 `spoor://` 链接提取对应图片或文件。

本地开发使用 `maturin develop`。完整参数、返回值、警告和错误码见[接口参考](../../docs/API_REFERENCE.md)。
