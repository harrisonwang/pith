# 来源定位（Provenance）设计

状态：**M1（PDF 页级）、M2（行级 + 坐标）、M3（线性格式与表格）、M4（PPTX slide 级）均已落地**。本文记录已实现行为与剩余边界。

## 一句话

让 spoor 输出的每一段文本，都能反查"它来自原文哪里"——PDF 给到页码（后续再给页面坐标），纯文本类（计划中）给到输入字节区间。这样下游 Agent 引用某段内容时，能机械地核对出处，而不是靠模型自己说。

## 为什么做这个

- 2025–2026 最强的需求是**可核查引用 / source grounding**：Anthropic Citations API 已经把"字符级来源定位"做成 API 原语；调研里 deep-research Agent 的引用"链接有效 >94%、但事实支持率只有 39–77%"，说明引用看着对、其实常常不支持论点。
- 价值因此迁移到**上游**：谁产出可信、带位置的文本，谁就握住这条契约的关键一半。spoor 产出逐页/逐段、可定位的文本，正是这一半。
- 契合 spoor 定位：这是解析输出的**一个属性**，确定性、无 ML、不联网。**它不是 RAG**——不做切块、不做向量、不做检索，只回答"这段输出对应原文哪个位置"。

## 一个具体场景

1. Agent 拿 spoor 输出的 markdown 喂给模型，模型答："据第 3 页，营收同比 +12%"。
2. Agent 把模型引用的那段文字在 markdown 里的位置（字节区间）拿出来。
3. 在 `provenance.spans` 里查找包含该区间的条目，得到 `Page { number: 3 }`。
4. Agent 就能把这句话归到原始 PDF 第 3 页，或让校验模型只读那一页核对真假。

## 数据模型（core 对外契约）

`ParseResult` 增加一个**可选**字段，默认不产出，旧调用方完全不受影响：

```rust
pub struct ParseResult {
    pub content: ParseContent,
    pub warnings: Vec<SpoorWarning>,
    pub stats: ParseStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

pub struct Provenance {
    pub spans: Vec<ProvenanceSpan>,       // 按 output.start 升序、互不重叠
}

pub struct ProvenanceSpan {
    pub output: TextRange,                 // 在本次返回的 markdown 里的位置
    pub source: SourceAnchor,              // 原文里的位置
}

pub struct TextRange { pub start: usize, pub end: usize }  // UTF-8 字节区间

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceAnchor {
    /// 页式格式（当前 PDF）：1-based 源页码；block 级下 born-digital 行
    /// 附近似包围盒（PDF 原生用户空间，y 向上，与 /MediaBox 同坐标系）。
    Page { number: usize, bbox: Option<Rect> },
    /// 幻灯片格式（当前 PPTX）：1-based 放映顺序页码（sldIdLst 位置，与
    /// PowerPoint 显示及 WarningLocation::Slide 一致）。slide 本身足够小，
    /// slide 粒度已能把引用锚死，不产坐标。
    Slide { number: usize },
    /// 线性格式（纯文本 / Markdown）：输出片段来自输入的哪个字节区间。
    Input { start: usize, end: usize },
    /// CSV/XLSX 文档模式渲染表的单元格：row 为渲染表 1-based 数据行，
    /// column 为表头单元格文本，sheet 为 XLSX 工作表名（CSV 无）。
    Cell { sheet: Option<String>, row: usize, column: String },
}
```

风格上对齐现有的 `WarningLocation`（同样是 `tag = "kind"` 的 tagged enum），调用方处理方式一致；用 tagged enum 正是为了后续加锚点种类时不破坏消费者。

## 偏移语义（关键，跨语言别踩坑）

- `output.start/end` 是 **UTF-8 字节偏移**，针对**本次 `ParseResult` 返回的那串 markdown**（不是原文）。理由：spoor 字节进字节出，字节偏移无歧义、和 `stats.output_bytes` 同一把尺子、和"内容寻址 / 确定性"一致。
- 各宿主取子串的方式：
  - Rust：`&markdown[start..end]`
  - Python：`markdown.encode("utf-8")[start:end].decode("utf-8")`
  - Node/WASM：`Buffer.from(md, "utf8").subarray(start, end)`（JS 字符串是 UTF-16，不能直接用这个 index 切）

## 各格式能给到什么

| 格式 | 锚点 | 状态 |
| --- | --- | --- |
| PDF | `Page { number, bbox }` | **页级与行级均已落地（M1+M2）**；bbox 来自字形几何（基线 ± 字号近似），仅 born-digital |
| 纯文本 / Markdown | `Input { start, end }` | **已落地（M3）**：输出与输入逐字节相同时精确（page 级整篇一条、block 级按行铺满）；重编码输入（GBK 等）退化为整篇一条粗粒度映射，不猜逐行偏移 |
| CSV / XLSX | `Cell { sheet, row, column }` | **已落地（M3）**：文档模式（Markdown 表）block 级逐数据单元格锚定；行号为渲染表数据行、列为表头文本；表头行与表格骨架不铺 span。注意 `parse` 自动分派对表格走 JSON 输出（无 markdown 可锚），Cell 锚点需走文档模式（Rust `parse_document_result` / CLI `--provenance block`） |
| PPTX | `Slide { number }` | **已落地（M4）**：page 与 block 级均为逐 slide 一条 span（span 覆盖整个 `## Slide N` 区块含 notes）；编号为放映顺序（sldIdLst），隐藏页保号；`--pages` 切片时编号跟随源页。slide 即引用单元，不产 shape 坐标（Agent 校验引用只需"读第 N 页"，坐标是给人类 UI 的，成本收益不成立） |
| DOCX / HTML / EPUB | — | 需要解析器保留段落/元素序号，成本较高，更靠后 |

## 开关与分级

`ParseRequest` 上的等级开关，**默认关闭**：

```rust
pub enum ProvenanceLevel { Off, Page, Block }   // 默认 Off

pub struct ParseRequest<'a> {
    // ...现有字段
    pub provenance: ProvenanceLevel,     // 默认 Off
}
```

- `Off`：不产出 `provenance`（与今天完全一致）。
- `Page`：PDF 每页一条 span。粒度粗、条目少。**已落地。**
- `Block`：PDF 每条重建行一条 span，带 `bbox`；被渲染改写（链接织入、表格重排）
  或几何不可信的字节落在无 bbox 的页级 gap span 里，因此 Block 是 Page 的严格
  细化——任何输出字节都仍能解析到页。**已落地。**

**为什么默认关闭、还要分级**：调研里一条硬约束是"边界税"——纯 Rust 引擎如果默认跨 WASM/PyO3/napi 边界吐一大堆 span，序列化开销会把自己的速度优势吃光。所以来源定位必须是**按需、可控量**的：要的人开，开多细自己定。这跟现有的页/表筛选是同一套"只返回被要的那部分"的设计原则。

## 分阶段落地

**M1 · 页级 ✅ 已落地**
- 仅 PDF，`ProvenanceLevel::Page`。
- 在 `render_layout`（`pdf.rs`）拼 markdown 时，记录每个 `## Page N` 区块的起止字节，产出 `Page { number }`。`output` 区间覆盖整个区块（含 `## Page N` 标题），页间 `\n\n` 分隔符不归任何页。
- 几乎零新增逻辑：每页 offset 在拼接时本就可知。
- 页码跟随源页：`--pages 2:2` 时只有一条 span，仍锚定源第 2 页。
- 四宿主贯通（CLI `--provenance page`、Python/Node/WASM `provenance` 选项）+ 测试。

**M2 · 行级 + 坐标 ✅ 已落地**
- `ProvenanceLevel::Block`，PDF born-digital；`SourceAnchor::Page` 增加可选 `bbox`，新增 `Rect`（f32，`ParseResult` 已改为仅 `PartialEq`）。
- 实现取行级粒度：几何行（`reading_order_lines`）在最终渲染文本中按前进游标做空白不敏感匹配（复用 locate 索引），行框 = 基线 ± 字号近似（ascent ≈ 1.0×、descent ≈ 0.25×字号）。
- 坐标系：PDF 原生用户空间（y 向上，与 `/MediaBox` 一致），PDF.js 可直接 `convertToViewportRectangle`。
- 断词行去掉行尾连字符后按前缀匹配；标题行匹配在 `### ` 前缀之后；被链接织入/表格重排改写的行与被去重的页眉页脚自然落空，字节归入无 bbox 的页级 gap。
- 多栏重排页：markdown 与区间都按**重排后**的文本生成，`output` 区间与 `bbox` 一一对应。

**M3 · 线性格式与表格 ✅ 已落地**
- 纯文本 / Markdown：`Input { start, end }`。输出=输入时 block 级按行铺满（含换行、无缝隙）、page 级整篇一条；重编码输入退化为整篇一条。
- CSV/XLSX 文档模式：block 级逐数据单元格 `Cell { sheet, row, column }`；`gfm_table_with_spans` 在渲染时逐格记录字节区间，零重复渲染逻辑。
- `locate_quote_grounded` 对表格命中直接返回该单元格锚点（重叠最大者）。

**M4 · PPTX slide 级 ✅ 已落地**
- `SourceAnchor::Slide { number }`：page/block 级均为逐 slide 一条 span，拼接时记录偏移（PDF M1 同款零成本模式）；span 覆盖 `## Slide N` 标题到该页内容（含 speaker notes）结束，页间分隔符不归任何页。
- 同批落地的 slide 契约件：`sldIdLst` 放映序（编号与 PowerPoint 一致，缺 presentation.xml 时确定性回退文件名序）、`--pages` 幻灯片切片（编号跟随源页、起始越界报错，与 PDF 同契约）、`stats.page_count`、隐藏页省略保号（`hidden_slide_omitted`）、无文本层姿态信号（`slide_no_text_layer`，区分备注有无）。
- 刻意不做：shape 级 bbox / EMU 坐标 / shape id。slide 是足够小的引用单元，坐标只服务人类 UI 框选；等 answer-trace 出现真实需求再借 tagged enum 无破坏加字段。

## 确定性与边界

- **确定性**：同输入字节 + 同 `ProvenanceLevel` → 同 `provenance`（可哈希、可缓存，呼应"内容寻址"方向）。
- **无 ML**：页码（及后续 bbox）全部来自内容流几何；判不准就少给（退回页级），不猜。
- **born-digital 限定**（M2 坐标）：扫描件不给坐标；无文本层的页沿用现有 `pdf_page_no_text_layer` warning。
- **向后兼容**：`Off` 时输出与之前逐字节一致，`provenance` 字段整个不出现在 JSON 里，不动现有快照。

## 跨宿主暴露（已落地）

- core：`ParseResult.provenance` + `ParseRequest.provenance`（`ProvenanceLevel`）。
- CLI：`--provenance page|block`（默认 off）。开启时 stdout 输出整个 `ParseResult` 的 JSON（含 markdown 与 provenance），**仅支持单个文档型输入**（偏移针对单份 markdown）；支持 PDF / PPTX / 纯文本 / Markdown / CSV / XLSX，其余格式报友好错误；与 `--mode`、`--extract` 互斥。仍受 `--max-output-kib` 约束（超限报错而非截断，避免破坏 JSON）。
- Python / Node / WASM：`parse_*` 增加 `provenance` 选项（字符串 `"page"`/`"block"`），返回结构带 `provenance`；只有开启时才序列化，规避边界税。绑定层把整份 `ParseResult` 直接序列化，因此 provenance 自动透传。

## 已定决策

1. **偏移单位**：UTF-8 字节（无歧义、与 `output_bytes` 一致）。
2. **CLI 形式**：单独的 JSON（整份 `ParseResult`），单输入；不污染 Markdown 主输出。
3. **M1 的 output 区间**：覆盖整个 `## Page N` 区块（含标题），最简单也够用。

## 明确不做

- 不做 RAG：不切块、不向量化、不检索、不重排相关性。
- 不内置 OCR/VLM：扫描件不产坐标。
- 不做"猜"的版面理解：判不准就降级，不输出低置信结果。
