# spoor

**智能体专用文档解析引擎。**

spoor 把文档、表格和网页转换成大模型能直接读取的文本。检测到没有文字的页面、未识别的图片或合并单元格时，它会明确提醒；能提取的图片和图表会保留链接，方便智能体继续处理。

[官网](https://spoor.pages.dev/) · [浏览器本地解析](https://spoor-pages-demo.pages.dev) · [浏览器内批量检索](https://spoor-corpus-demo.pages.dev) · [完整文档](docs/README.md)

## 为什么用 spoor

- **尽量保留文档结构。** 标题、段落、列表、表格和分页信息不会被混成一段文字。
- **发现有内容可能没读出来时，会明确提醒。** 例如，检测到扫描页、异常文字、合并单元格或没有文字的幻灯片时，spoor 会说明具体情况。
- **引用能找到出处。** `locate_quote` 会检查引用是否出现在解析结果中，并在支持的格式里返回所在页、幻灯片或近似坐标。
- **默认在本地运行。** 除非让 CLI 读取网址，否则 spoor 不联网，也不上传文件。你的程序如果继续把结果交给云端服务，数据仍可能离开本机。

无论使用 CLI、Rust、Python、Node.js 还是 WASM，警告码和错误码都相同，页码、行号和列号的计算方式也一致。

隐私和部署注意事项见[安全说明](SECURITY.md)。

## 快速开始

如果已经安装 Node.js：

```bash
npm install -g @harrisonwang/spoor-cli
spoor report.pdf > report.md
```

也可以一次处理多个文件或表格：

```bash
spoor report.docx slides.pptx
spoor data.xlsx > data.json
```

Homebrew、Scoop 和 Cargo 的安装方式见[入门指南](docs/GETTING_STARTED.md)。

## 查找引用的出处

```bash
pip install pyspoor
```

```python
from spoor import locate_quote, parse_path

result = parse_path("report.pdf", provenance="block")
markdown = result.content.value.markdown
found = locate_quote(markdown, "营业收入同比增长 12%", result.provenance)

if found is None:
    print("没有在已读出的文字中找到这条引用")
elif found.method in {"exact", "whitespace_insensitive"}:
    start, end = found.span["start"], found.span["end"]
    print("找到 Markdown 中的原句：", markdown[start:end])
    print("所在位置：", found.anchor)
else:
    print("找到可能相关的数据，请回到原文确认：", found.method, found.hit, found.anchor)
```

有些结果只能说明找到了相关表格行或数值，仍需人工确认。没有找到，也不代表原文件一定没有相关内容。五种查找方式的区别见[引用核对说明](docs/QUOTE_VERIFICATION.md)。

## 在哪儿运行

| 使用场景 | 选择 | 安装 |
| --- | --- | --- |
| 命令行、脚本、Agent Skill | `spoor` CLI | Homebrew、Scoop、npm 或 `cargo install spoor-cli` |
| Rust 应用 | `spoor-core` | `cargo add spoor-core` |
| Python 应用 | `pyspoor` | `pip install pyspoor` |
| Node.js / Electron | `@harrisonwang/spoor` | `npm install @harrisonwang/spoor` |
| 浏览器 / Cloudflare Worker | `@harrisonwang/spoor-wasm` | `npm install @harrisonwang/spoor-wasm` |

各语言的函数和返回值见[接口参考](docs/API_REFERENCE.md)。

## 支持的文件

文档：PDF、DOCX、PPTX、HTML、EPUB、IPYNB。表格：XLSX、CSV、TSV。

spoor 不做 OCR，不执行宏、公式、脚本或 Notebook 中的代码，也不支持旧版 Office 文件和加密文档。每种格式能读出什么、可能漏掉什么，见[格式与限制](docs/FORMATS_AND_LIMITS.md)。

## 继续了解

- [入门指南](docs/GETTING_STARTED.md)：安装、解析指定页、行或列，以及提取图片。
- [引用核对说明](docs/QUOTE_VERIFICATION.md)：五种查找方式分别能说明什么。
- [`answer-trace`](examples/answer-trace/)：核对回答中的引用，并在原 PDF 中标出位置。
- [`agent-spoor`](examples/agent-spoor/)：直接调用、MCP 和 Skill 三种用法。
- [示例目录](examples/README.md)：浏览器、Cloudflare、桌面和 Lambda 示例。
- [设计说明](docs/DESIGN_NOTES.md)：spoor 负责什么，为什么这样设计。

当前版本为 `v0.13.0`，仍处于 `0.x` 阶段。问题与建议请提交 GitHub issue；安全问题请通过 GitHub Security 私下提交。项目采用 [MIT 许可证](LICENSE)。
