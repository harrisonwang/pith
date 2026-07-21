<!-- Pass A：章节骨架 + 学习要点 + 代码弧线。子标题与散文待 Pass B / Pass C。 -->

# Getting Started：把任意上传文件解析成 LLM 载荷

## 你将构建什么

你将学到：

- spoor 做的一件事：把文档与表格转成 LLM 可直接消费的文本
- 本教程的目标——一个对任意上传文件都健壮的函数 `to_llm_input(name, data)`
- 为什么"一次 `parse()`、按文件形态自动分派"是嵌入 spoor 的核心路径
- 用 Python 绑定 `pyspoor` 实现；同一套契约在 CLI / Rust / Node / WASM 等价

```python
# 教程终点：一个吃 bytes、吐 LLM 输入的函数
payload = to_llm_input("report.pdf", data)   # 文档 → Markdown
payload = to_llm_input("data.xlsx", data)    # 表格 → JSON
```

## 1. 安装 pyspoor 并解析第一个文件

你将学到：

- 安装 `pyspoor`
- 两个入口：`parse_path`（按路径）与 `parse_bytes`（按字节，适合上传场景）
- `ParseResult` 的顶层三件套：`content` / `warnings` / `stats`
- spoor 只围绕 bytes 工作，不做隐式文件/网络 I/O

```bash
pip install pyspoor
```

```python
from spoor import parse_path

result = parse_path("note.txt")

print(result.content.kind)            # "document"
print(result.content.value.markdown)  # "hello path\n"
print(result.stats.format)            # "text"
```

```python
from spoor import parse_bytes

# 上传场景拿到的是 bytes：传 source_name 帮助格式检测
result = parse_bytes(data, source_name="note.txt")
```

## 2. 按 content.kind 分派文档与表格

你将学到：

- `content.kind` 只有两种取值：`"document"` 与 `"tables"`
- 文档分支：`content.value` 是 `DocumentResult`，取 `.markdown`
- 表格分支：`content.value` 是 `TableResult`，取 `.tables`（一组自描述的 dict）
- 这一步就是 spoor 的定义性行为：同一次调用，按形态自动分派

```python
from spoor import parse_bytes

def to_llm_input(name: str, data: bytes):
    result = parse_bytes(data, source_name=name)
    if result.content.kind == "document":
        return result.content.value.markdown          # 文档 → Markdown 字符串
    return result.content.value.tables                 # 表格 → list[dict]
```

```python
# 表格 dict 是自描述的：headers / rows / row_range / truncated / warnings ...
tables = to_llm_input("data.csv", data)
print(tables[0]["headers"])     # {"Name": {"column_index": 0}, ...}
print(tables[0]["rows"][0])     # {"Name": "Alice", "Score": "1", ...}
```

## 3. 把 Markdown 与表格 JSON 组装成 LLM 载荷

你将学到：

- 文档分支直接就是 Markdown，可整段喂模型
- 表格分支用 `json.dumps` 序列化成 JSON 字符串喂模型
- 每个表格 entry 自带 `truncated` / `warnings`，模型能看见"数据被截断了"
- 用 `stats.output_bytes` 估算载荷体量，控制 token 预算

```python
import json
from spoor import parse_bytes

def to_llm_input(name: str, data: bytes) -> str:
    result = parse_bytes(data, source_name=name)
    if result.content.kind == "document":
        payload = result.content.value.markdown
    else:
        payload = json.dumps(result.content.value.tables, ensure_ascii=False)
    print(f"{name}: {result.stats.output_bytes} 字节输出")
    return payload
```

## 4. 读 warnings：成功不等于完整

你将学到：

- `result.warnings` 是一组 dict，即使解析成功也可能非空
- 每条 warning 有稳定 `code` 与 `message`，可选 `location`（页/slide）
- 常见 code：`embedded_visuals_omitted`、`pdf_page_no_text_layer`、`merged_table_structure_not_preserved`
- 把 warnings 透传给调用方或日志，避免静默丢内容

```python
result = parse_path("扫描混排.pdf")

for w in result.warnings:
    loc = w.get("location")
    where = f"（{loc['kind']} {loc['number']}）" if loc else ""
    print(f"[{w['code']}]{where} {w['message']}")

# 例如：[pdf_page_no_text_layer]（page 2） 第 2 页没有可提取的文本层
```

## 5. 按稳定 code 兜住 SpoorError

你将学到：

- `from spoor import SpoorError`，用 `try/except` 包住 `parse_*`
- 按稳定 `error.code` 分支，而不是匹配错误消息
- 用 `error.recoverable` / `error.hint` / `error.stage` 决定下一步与给用户的提示
- 批处理时单个文件失败不连累其它

```python
from spoor import SpoorError, parse_bytes

def to_llm_input(name: str, data: bytes) -> str | None:
    try:
        result = parse_bytes(data, source_name=name)
    except SpoorError as e:
        if e.code == "unsupported_format":
            return None                       # 跳过：不是我们要的文件
        if e.code in ("encrypted_pdf", "legacy_or_encrypted_office"):
            raise UserVisible(e.hint)         # 不可恢复：提示用户处理后重传
        # parse_budget_exceeded / work_budget_exceeded / invalid_container ...
        log.warning("解析失败 code=%s stage=%s reason=%s", e.code, e.stage, e.reason)
        return None
    ...
```

```python
# 批量上传：逐个 try，单个失败记下来继续
results = []
for f in files:
    payload = to_llm_input(f.name, f.data)
    results.append({"name": f.name, "ok": payload is not None})
```

## 6. 用 narrowing 控制喂给 LLM 的体量

你将学到：

- 大表格用 `sheet` / `columns` / `limit` / `offset` 或 `rows` 收窄
- `rows`（1-based 闭区间）与 `limit`/`offset` 互斥
- 大 PDF 用 `pages` 只解析需要的页
- `stats.page_count` 始终报告总页数，可用 `pages=(1,1)` 廉价探页

```python
# 大表格：只取需要的列 + 行窗口
result = parse_bytes(xlsx_bytes, source_name="data.xlsx",
                     sheet="Sheet1", columns=["分类", "金额"], limit=50, offset=100)

# 或按行号区间（与 limit/offset 互斥）
result = parse_bytes(csv_bytes, source_name="data.csv", rows=(5, 104))
```

```python
# 大 PDF：只解析前 3 页
result = parse_path("report.pdf", pages=(1, 3))

# 廉价探页：先取第 1 页拿到总页数，再决定要哪段
probe = parse_path("report.pdf", pages=(1, 1))
print(probe.stats.page_count)     # 总页数（即便只取了 1 页）
```

## 下一步

你将学到：

- 同一套 `parse` 契约与稳定 code 在 CLI / Rust / Node / WASM 等价（见 Reference）
- 用 provenance 把 LLM 引用锚回原文页（Diving Deeper：答案溯源）
- 在不可信输入上设预算、抵御 ZIP 炸弹（Diving Deeper：安全解析）
- 两种输出契约的字段与设计（Diving Deeper：输出契约）
