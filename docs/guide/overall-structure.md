# spoor 文档整体结构（Phase 1 产物）

> 本文件是 scaffold-docs 的规划底稿，供你审阅与编辑。后续每个阶段都会重新读它。
> 文档成品将落在 `docs/guide/` 下：`getting-started.md`、`diving-deeper/<topic>.md`、`reference/<module>.md`。

## 受众

**主受众：** 把 spoor 嵌入 LLM/Agent 管线的应用工程师
在产品里调用 `parse()`（Rust / Python / Node / WASM 任一形态）把用户提供的 PDF/DOCX/表格转成 LLM 可直接消费的文本。带着 API 集成经验而来，关心统一的 `ParseResult` 形状、`warnings` 与 `SpoorError` 的处理、大输入的收窄，以及用 provenance 把回答锚回原文。

**次要受众：**
- 边缘 / 沙箱工程师 —— 在浏览器、Cloudflare Workers、Lambda、多租户容器里跑 WASM/原生引擎，要求文档不出端、恶意文件可隔离。关心 parse/work 预算、ZIP 炸弹防御、输出封顶、安全 `spoor://` URI、WASM 构建变体。仅作"平局裁决"，不凌驾主受众。

## 已确认决策（2026-06-30）

1. **Getting Started 交付形态：Python（pyspoor）。** 跨形态等价由 Reference 兜住（DD-6 未选，等价说明并入 Reference/GS 旁注）。
2. **要写的 Diving Deeper 主题：DD-1（provenance，旗舰）、DD-4（不可信输入安全解析，服务次要受众）、DD-5（两种输出契约）。** DD-2 / DD-3 / DD-6 / DD-7 本轮不写（保留为候选）。
3. **输出目录：`docs/guide/`**（与内部规划 `docs/v1/` 分开）。
4. GS 6 步顺序/分组维持不变，无跨桶移动。

## 选定的主用例（Getting Started）

**统一 `parse()` 闭环**：构建一个对任意上传文件都健壮的解析函数。一次 `parse()`，按 `content.kind` 自动分派——文档型 → Markdown 喂 LLM，表格型 → schema+preview JSON；统一处理 `warnings` 与 `SpoorError`；用 narrowing（pages / sheet / rows / columns / limit / offset）把喂给 LLM 的体量控制住。

选它的理由：最完整地呈现 spoor 的定义性行为"同一套引擎按文件形态自动分派"，给嵌入者最快建立全局心智模型；provenance / 答案溯源、extract_media 等留给 Diving Deeper。

## Getting Started 顶层提纲

一篇叙事式端到端教程，用 Python（pyspoor）实现一个"上传文件 → LLM 载荷"的解析函数。开篇用一段交代"你将构建什么、为什么是这条路径"，随后 6 步：

| 步骤 | 读者动作 | 覆盖的组件 / 概念 |
| --- | --- | --- |
| 1 | 安装 pyspoor 并解析第一个文件 | 安装；`parse_path` / `parse_bytes`；`ParseResult` 顶层形状（`content` / `warnings` / `stats`） |
| 2 | 按 `content.kind` 分派文档与表格 | `ParseContent` 分派；`DocumentResult`（`markdown`）vs `TableResult`（`tables[]`）；"按形态自动分派"心智模型 |
| 3 | 把 Markdown 与表格 JSON 组装成 LLM 载荷 | 两个分支各自如何成为 LLM 输入；为什么文档用 Markdown、表格用 JSON；token 经济与 `stats.output_bytes` |
| 4 | 读 warnings：成功不等于完整 | `SpoorWarning` / `WarningCode` / `WarningLocation`；成功仍可能不完整（如 `embedded_visuals_omitted`、`pdf_page_no_text_layer`） |
| 5 | 按稳定 code 兜住 SpoorError | `SpoorError` 契约；`ErrorCode`（8 个稳定 code）；`recoverable` / `hint` / `stage` 的用法 |
| 6 | 用 narrowing 控制喂给 LLM 的体量 | 表格 `sheet`/`rows`/`columns`/`limit`/`offset`；PDF `pages`；`rows` 与 `limit`/`offset` 互斥；`page_count` 廉价探页；`truncated` |

读完即触及：`parse`、`ParseResult`、文档与表格两个分派分支、Markdown、表格 schema、warnings、错误、narrowing、stats——覆盖最重要的组件。

## Diving Deeper 候选主题

每个主题：意图 → 关键设计决策与原因 → 按"对象的意图"组织的 API 走查。标 ★ 为建议第一梯队。

| ID | 主题（按读者动作命名） | 意图与覆盖 | 启发式依据 |
| --- | --- | --- | --- |
| **DD-1 ★** | 用 provenance 把 LLM 引用锚回原文 | 不信任模型自述出处，把输出片段映射回源页/区间做"答案溯源"。覆盖 `ProvenanceLevel`、`Provenance`/`ProvenanceSpan`/`TextRange`/`SourceAnchor`、跨形态的字节区间切片（Rust UTF-8 / Python bytes / Node·WASM Buffer）。配 `examples/answer-trace` 真实示例。 | 旗舰用例（GS 亚军）+ 心智模型 |
| **DD-2** | 把内嵌图片与矢量图交给外部 VLM | spoor 不解码图片：正文留安全 `spoor://` 占位符，按需取原始字节交给 VLM。覆盖 `extract_media`、安全 URI 方案（docx/pptx part、pdf obj、pdf page→SVG）、`embedded_visuals_omitted` / `vector_graphics_omitted` warning、URI 校验如何防穿越与跨容器伪造。 | 备选路径 + 心智模型 |
| **DD-3** | 收窄大输入：表格分页与 PDF 页范围 | 把 token 预算与解析开销限定住。覆盖 `TableFilter`、`DocumentFilter`、共享的 1-based 闭区间契约、`rows` 与 `limit`/`offset` 互斥、`page_count` 廉价探页、多 sheet 的 `workbook_sheets`、JSON envelope 的 `usage`/`truncated` 自描述。（比 GS 步骤 6 更深。） | 定制 + 落选用例 |
| **DD-4 ★** | 在不可信输入上安全解析：预算与 ZIP 炸弹防御 | 为什么 core 只收 bytes、且不让 panic 越过公开边界；parse 字节预算、work 运算量预算、ZIP 三重上限、输出封顶；如何为多租户/不可信环境接线。覆盖 `ParseLimits`（`max_parse_bytes`/`max_work_units`）、`DEFAULT_*` 常量、边界 panic 归一化。**直接服务次要受众。** | 心智模型 + 定制 |
| **DD-5 ★** | 区分两种输出：文档 Markdown 与表格 schema+preview JSON | 为什么有两种输出形态、为什么 token 经济、字段为何自描述。覆盖 `OutputMode`/`default_mode_for`、`DocumentResult` vs `JsonOutput`/`TableEntry` schema（`schema_version`、`usage`、`headers`/`HeaderInfo`、`preamble`、`row_range`、`header_row`、`workbook_sheets`、`delimiter`）、截断行为。 | 心智模型 |
| **DD-6** | 在 CLI / Rust / Python / Node / WASM 间保持等价 | 同一套 parse 契约与错误/warning code 跨宿主；每个宿主如何编组选项与字节区间；何时选哪种形态；WASM `core-formats` vs `full` 构建变体。服务"跨绑定"需求与次要受众。 | 备选路径 + 心智模型 |
| **DD-7** | 按稳定 code 与 warning 设计失败处理 | 把稳定 code 当作集成契约：`recoverable`/`hint`/`stage`、成功带 warning、`location`。覆盖 `ErrorCode`(8) 与 `WarningCode`(9) 的设计动机。**注意：与 GS 步骤 4–5 有重叠**，可考虑并入 Reference/errors 导言而非单列。 | 心智模型（可选） |

## Reference 模块（纯 API 规格，按对象组织）

| 文件 | 内容 |
| --- | --- |
| `reference/api.md` | 调用入口（各形态映射到同一组函数）：`parse` / `parse_bytes` / `parse_path`、`parse_document` / `parse_document_result`、`parse_tables`、`detect_format`、`extract_media`；CLI `spoor` 命令与全部 flag |
| `reference/request.md` | 输入类型：`ParseRequest`、`ParseLimits`、`TableFilter`、`DocumentFilter`、`ProvenanceLevel`、`Format`，以及各绑定的可选参数名（`sheet`/`rows`/`columns`/`limit`/`offset`/`pages`/`provenance`/`max_*`）；输出封顶与解析预算常量 |
| `reference/result.md` | 结果类型：`ParseResult`、`ParseContent`、`DocumentResult`、`TableResult`、`ParseStats`、`Provenance` 同族（`ProvenanceSpan`/`TextRange`/`SourceAnchor`）、`SpoorWarning`/`WarningCode`/`WarningLocation` |
| `reference/table-json.md` | 表格 JSON schema：`JsonOutput`、`TableEntry`、`HeaderInfo`、`PreambleInfo`、`RowRange`、`TABLE_SCHEMA_VERSION`、`TABLE_USAGE`、`a1_range`、`cells_to_values` |
| `reference/errors.md` | 错误契约：`SpoorError`(`StructuredError`)、`ErrorCode`（8 个稳定 code 表）、`ParseStage` |

## 给主受众的范围说明

- **跨形态等价**是反复出现的主线：核心契约（`parse` → `ParseResult`、稳定 `code`、narrowing 选项）在 CLI、Rust、Python、Node、WASM 完全一致。GS 用一种形态写实，Reference 与 DD-6 兜住其余形态。
- **安全基座**（预算 / ZIP 炸弹防御 / 输出封顶 / 安全 URI）既是主受众处理不可信上传的需要，也是次要受众的核心关切，集中在 DD-4。
- **不在范围内**：各解析器内部实现（PDF 引擎、布局推断、OOXML 细节）属内部工程文档（见 `docs/v1/`），本套面向集成者的文档不展开。
