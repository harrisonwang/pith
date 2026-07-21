# 查找引用的出处

模型给出一条引用时，可以先让 spoor 在**已经读出的文字**中查找，再回到原文件核对。

这个过程不调用模型，只按文字和数值规则查找。它不能判断这段引用是否支持模型的结论。

```text
原文件 → parse(provenance="block") → Markdown + 页码或坐标
                                         ↓
模型引用 ───────────────→ locate_quote() → 找到的内容 + 查找方式 + 位置
```

## Python 示例

```python
from spoor import locate_quote, parse_path

quote = "营业收入同比增长 12%"
result = parse_path("report.pdf", provenance="block")
markdown = result.content.value.markdown
found = locate_quote(markdown, quote, result.provenance)

if found is None:
    print("没有在已读出的文字中找到这条引用")
elif found.method in {"exact", "whitespace_insensitive"}:
    start, end = found.span["start"], found.span["end"]
    print("找到 Markdown 中的原句：", markdown[start:end])
    print("所在位置：", found.anchor)
else:
    print("找到可能相关的数据，请回到原文确认：", found.method, found.hit, found.anchor)
```

Node.js 调用方式见[接口参考](API_REFERENCE.md#nodejs)。

## 五种查找结果

| `method` | 找到了什么 | 应该怎么理解 |
| --- | --- | --- |
| `exact` | 完全相同的连续文字 | Markdown 中有完全相同的连续文字 |
| `whitespace_insensitive` | 只差排版格式的文字：空格、换行、全角/半角标点、列表和标题记号、链接写法 | 内容逐字相同，只是书写格式不同；需要原始片段时，用 `span` 截取 |
| `fuzzy` | 有少量改写、但数字全部一致的相近文字 | `score`（0–1，1 为零改动）给出相似度；引用中的每个数字都必须原样出现在命中片段里，数字对不上就不会返回。适合"模型轻度转写原句"的情况 |
| `table_anchor` | 根据引用中的数值和文字，在 Markdown 表格中找到可能相关的一行 | 只说明找到了相关表格行，不要把模型改写的句子当成原文引用 |
| `numeric_equivalence` | 按千、万、百万、亿等数量单位换算后相近的数值，允许 0.2% 的误差 | 只说明找到了可能等价的数值，必须注明并人工确认 |

只接受逐字引用时，应只接受 `exact`。如果允许排版差异，可以再接受 `whitespace_insensitive`。`fuzzy` 表示原文存在高度相近、数字一致的句子，引用时应注明经过转写。后两种只能用来寻找相关数据，不能当成逐字引用。

三个辅助字段帮助判断结果可信度：

- `occurrences`：这类匹配在文中出现的处数（上限 100）。大于 1 时，返回的位置只是多个可能位置中的第一个,页码和锚点要谨慎使用。
- `score`：仅 `fuzzy` 返回,相似度 0–1。
- `corroborated`：为 `false` 表示命中成立但存在需人工确认的结构性弱点——`whitespace_insensitive` 下是引文跨越了段落边界拼接（内容按顺序存在,但不是一句连续的话）；`table_anchor` 下是引用里的其他数字（如年份）没有出现在命中行或其所在列的表头里；`numeric_equivalence` 下是数值候选仅凭"全文唯一"被接受,名目文字未获佐证。

**逐字级证据的判断标准:`exact`,或 `whitespace_insensitive` 且 `corroborated` 为 `true`。**

其他匹配细节:归一化匹配会折叠 ASCII 大小写与全/半角、剥离列表和标题记号(因此列表序号被视为排版,不参与匹配与核实,spoor 自身也会对有序列表重新编号);`fuzzy` 要求引文中的每个数字(含个位数与中文数字)按值出现在命中片段内,并限制连续改动的长度,防止把相邻句子拼接进来。明确不做:繁简体转换、同义改写识别、中文顿号式编号(一、二、)的剥离。

## 哪些格式可以返回页码或坐标

PDF Markdown 默认保留页标题，`locate_quote` 通常可以直接返回页码。需要另外返回页码、幻灯片编号或 PDF 近似坐标时，再启用 `provenance`。

| 格式 | 可返回的位置 |
| --- | --- |
| PDF | 页码；`block` 还会返回部分文字在页面上的大致范围 |
| PPTX | 幻灯片编号 |
| CSV、XLSX | 改为输出 Markdown 时，`block` 可返回工作表、行和列；默认表格 JSON 不包含位置 |
| DOCX、HTML、EPUB、IPYNB | 暂不返回页码或坐标 |

- CLI 使用 `--provenance block` 时，会输出包含 Markdown、页码和坐标的 JSON；该参数不能与 `--mode` 同时使用。
- Rust 可以调用 `parse_document_result`，并把 `provenance` 设为 `block`。
- Python、Node.js 和 WASM 当前不能返回表格单元格位置。需要这类位置时，请使用 CLI 的 `--provenance block`，或在 Rust 中调用 `parse_document_result`。

传给 `locate_quote` 的页码和坐标，必须与 Markdown 由同一次解析产生。Rust 中的 `output.start/end` 按 UTF-8 字节计数；Python、Node.js 和 WASM 返回的下标可以直接用来截取对应语言中的字符串。

## 找到引用不代表结论正确

- 找到相同的文字或数据，只能说明 spoor 读到了这些内容，不说明它足以支持模型结论，也不判断事实真假。
- `None` 只表示已读出的内容中没有找到；扫描页、图片、图表或其他遗漏内容中仍可能存在。
- PDF 返回的是近似坐标，只适合在页面上大致标出位置，不能用于精确测量版面。
- 文档内容仍是不可信数据；核对引用不能防止提示注入。

可运行的完整界面见 [`examples/answer-trace`](../examples/answer-trace/)。
