# 入门指南

运行 spoor 并传入文件即可。普通文档会转成 Markdown，表格会返回 JSON。

## 1. 安装

```bash
# macOS / Linux
brew install harrisonwang/tap/spoor

# Windows
scoop bucket add harrisonwang https://github.com/harrisonwang/scoop-bucket
scoop install spoor

# 已安装 Node.js
npm install -g @harrisonwang/spoor-cli

# 已安装 Rust
cargo install spoor-cli
```

`@harrisonwang/spoor-cli` 需要 Node.js 16+，`@harrisonwang/spoor` 需要 Node.js 18+。从源码构建需要 Rust 1.85+。

npm 预编译包目前覆盖 macOS arm64/x64、Linux GNU x64 和 Windows x64。其他平台可以尝试 `cargo install spoor-cli`。

安装后运行 `spoor --version` 确认命令可用。

## 2. 解析第一个文件

```bash
spoor report.pdf
```

PDF 输出会按页分开：

```markdown
## Page 1

...
```

常用输入方式：

```bash
spoor report.docx slides.pptx           # 多文件
spoor "docs/*.pdf"                     # 通配符
cat data.csv | spoor --format csv -    # 标准输入
spoor https://example.com               # URL，仅 CLI 会联网
```

`spoor-core` 本身不联网；只有 CLI 在收到网址时才会联网读取。

## 3. 只解析需要的部分

处理大文档时，最好先读取相关页或表格行，不要一开始就调高输出上限：

```bash
spoor report.pdf --pages 1:3
spoor deck.pptx --pages 8:12
spoor data.xlsx --sheet Sheet1 --rows 5:104 --columns 分类,金额
spoor data.csv --limit 50 --offset 100
```

`--pages` 和 `--rows` 都从 1 开始，并包含区间两端。`--rows` 不能与 `--limit`、`--offset` 同时使用。CSV、TSV 和 XLSX 默认只返回每张表前 100 条数据行。

## 4. 别忽略警告

即使命令成功结束，仍可能有部分内容没读出来。检测到没有文字层的 PDF 页面、异常文字、合并单元格或未识别的图片时，spoor 会给出警告，例如：

```text
embedded_visuals_omitted · page 6
```

CLI 会把警告写到标准错误，并在 Markdown 末尾保留警告说明。在程序中使用时，应检查 `result.warnings`；读取表格时还要检查 `truncated`。完整警告码见[接口参考](API_REFERENCE.md#警告)。

## 5. 提取图片

对于可以提取的图片，DOCX、PPTX 和部分 PDF 会在 Markdown 中留下 `spoor://` 链接：

```markdown
![DOCX image 1](spoor://docx/part/word/media/image1.png)
```

用同一份文件取出对应图片：

```bash
spoor report.docx \
  --extract spoor://docx/part/word/media/image1.png \
  > image.png
```

spoor 只返回图片内容，不识别图片里写了什么。需要识别时，可以再把图片交给视觉模型。

Python、Node.js、Rust 和 WASM 的用法见[接口参考](API_REFERENCE.md)。需要查找模型引用的出处，请看[引用核对](QUOTE_VERIFICATION.md)。
