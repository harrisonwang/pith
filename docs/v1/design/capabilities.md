# spoor 能力决策与演进规划

本文从 Agent 使用场景出发，记录 spoor 当前能力、调研结论、立即实现项、后续路线和明确边界。它不是功能愿望清单；任何新增能力都必须回答：

1. 它会让 Agent 做出什么不同决策？
2. spoor 能否确定性、离线、跨宿主一致地完成？
3. 失败或降级时，Agent 能否知道哪里不可信？
4. 是否有可复现 fixture、质量门槛和资源上界？

## 结论

spoor 当前最重要的方向不是继续增加格式，而是让 Agent：

- 读到更正确的顺序和结构；
- 知道哪些位置没有读到或可能读错；
- 在可控预算内完成解析；
- 在 Rust、CLI、Python、Node 和 WASM 中拿到等价决策信号。

调研提出的核心判断成立：市场常在拼格式广度，Agent 真正需要的是“读得对”和“错得明白”。但需求声量不能直接等于实现顺序。对于 PDF 多栏、标题层级、页眉页脚和表格结构，如果没有统一的几何中间模型与质量门槛，快速加入启发式会制造新的静默错误，反而违背 spoor 的定位。

## Agent 决策链

spoor 面向 Agent 的价值链应按以下顺序建设：

| 层级 | Agent 要回答的问题 | spoor 的责任 |
| --- | --- | --- |
| 1. 输入识别 | 这是什么格式，能否安全处理？ | magic bytes、容器识别、稳定错误码 |
| 2. 文本正确性 | 提取出的字符是否可信？ | 编码、PDF 文本层诊断、乱码诊断 |
| 3. 阅读顺序 | 内容顺序是否适合阅读和分块？ | 页/slide/chapter 边界，后续补 PDF/PPTX 布局顺序 |
| 4. 语义结构 | 标题、列表、表格、链接是否保留？ | Markdown/表格 JSON，后续补 PDF 结构 |
| 5. 噪声控制 | 页眉页脚、水印、脚本是否应进入上下文？ | HTML 噪声过滤，后续补 PDF 重复区域分类 |
| 6. 完整性诊断 | 哪些内容缺失、降级或可疑？ | `warnings[]`、稳定 code、结构化位置 |
| 7. 资源边界 | 是否能在预算内完成？ | 字节上限、ZIP 防御、输出上限；后续补运算量上限 |
| 8. 宿主一致性 | 不同运行形态能否按同一信号分支？ | 同一 Rust core 与序列化契约 |

Agent 不应只判断调用是否成功。推荐决策顺序是：

```text
SpoorError.code
  -> 失败处理或改走外部能力

ParseResult.warnings[].code + location
  -> 决定是否信任、重试、调用 OCR/VLM、请求人工确认

ParseContent + stats
  -> 分块、索引、检索或继续读取表格分页
```

## 当前已具备

### 输入与防御

- bytes-only Rust core，不执行文件、网络、脚本、宏、公式或 notebook code。
- 稳定格式检测与错误码。
- 默认 64 MiB 单次解析内存上限。
- ZIP 条目数、单条目大小、压缩比和声明总解压量限制。
- CLI 默认 256 KiB 总输出上限。
- panic 在公共边界转换为结构化 `parse_failed`。

### 重点格式

| 格式 | 当前适合 Agent 使用的能力 | 主要缺口 |
| --- | --- | --- |
| DOCX | 标题层级、段落、列表、链接、脚注、小表、Unicode、内嵌图片安全路径占位符 | 合并表格结构、图片/图表语义、comments/endnotes |
| XLSX | sheet/range/header/preamble/rows、分页筛选、截断诊断 | 公式表达式、语义样式、复杂逻辑表识别 |
| PDF | 文本层、页边界、无文本页/可疑文本层诊断、多栏重排（几何推断 + warning）、空格对齐文本表格重建为 GFM、矢量图页占位与整页 SVG 提取、被裁剪隐形文本抑制、outline 标题层级、URI 链接织入、页眉页脚去重（可关）、行尾断词重合 | 无 outline 的标题推断、GoTo 内部链接、复杂视觉表格 |
| PPTX | slide 边界、文本、小表、speaker notes | shape 阅读顺序、bullet 层级、合并表格、视觉对象语义 |
| HTML/URL | article/main/body、标题、段落、列表、链接、表格、代码、图片 alt | 完整 readability、相对链接、复杂嵌套 |
| EPUB | OPF spine 顺序、章节边界、复用 HTML 语义渲染 | 固定版式、图片/音视频、复杂导航 |
| IPYNB | markdown/code cell、顺序、语言提示 | outputs、widgets、富媒体语义 |

### 本轮已直接新增

本轮优先实现“让 Agent 知道自己读不对”，因为它不依赖不成熟的版面推断，且能立即改变 Agent 行为。

| 能力 | 稳定 code | Agent 行为 |
| --- | --- | --- |
| PDF 混合文档中的无文本页诊断 | `pdf_page_no_text_layer` | 只把对应页转交外部 OCR/VLM，不必拒绝整份文档 |
| PDF 可疑文本层保守诊断 | `pdf_page_suspicious_text_layer` | 不直接信任包含替换字符、控制字符或重复 glyph 占位符的页面 |
| DOCX/PPTX 合并表格降级诊断 | `merged_table_structure_not_preserved` | 不把 GFM 空白/重复单元格当作原始 rowspan/colspan |
| DOCX/PPTX 视觉对象省略诊断 | `embedded_visuals_omitted` | 知道文本输出是残缺视图；DOCX/PPTX 可按安全图片占位符提取相关图片，其他对象按需调用外部视觉解析 |
| DOCX/PPTX 内嵌图片位置与安全路径 | `spoor://docx/part/word/media/*` 与 `spoor://pptx/part/ppt/media/*` | Agent 可按正文/幻灯片顺序选择图片，通过 CLI 安全提取后交给外部 VLM |
| 单资源内嵌媒体提取 | `--extract <uri>` / `extract_media` | 使用格式无关入口按安全 URI 原样输出；当前仅支持 DOCX，后续格式沿用同一入口 |
| warning 结构化位置 | `location.kind = page/slide` | 精确路由受影响页或幻灯片 |
| CLI in-band warning | stdout + stderr | 只读取 stdout 的 Agent 也不会错过完整性警告 |
| PDF 中间页失败传播 | `parse_failed` | 不再把前几页的部分结果误报为完整成功 |
| PDF 分页提取复用页映射与解析器状态 | 内部优化 | 避免页数增长时反复构建整份页映射，降低大 PDF 的不必要开销 |

Rust、Python、Node、WASM 均通过同一个 `ParseResult.warnings` 契约获得这些信息。Rust 中需要完整诊断时应调用 `parse` 或 `parse_document_result`；`parse_document` 是只返回 Markdown 的兼容便捷接口，会丢弃 warnings。

### 引文核验

`locate_quote(markdown, quote)`（Rust/Python/Node/WASM 同名导出，Rust 另有可复用
`Locator`）在 spoor 产出的 Markdown 中定位模型引文，依次尝试四档确定性匹配：
精确子串、忽略空白、表格单元格锚点、数值/单位换算。命中返回 span、上下文、页码
与所用档位；四档全部落空时调用方应把对应结论按"无法核验"处理。它与 provenance
（`--provenance page`）共同构成"引用 → 输出位置 → 源页码"的溯源链路；块级/坐标级
锚点见 [provenance.md](provenance.md) 的 M2/M3 规划。

## 调研能力决策

### 立即加入

| 调研主题 | 决策 | 原因 |
| --- | --- | --- |
| 页级无文本层/乱码诊断 | 已加入 | 直接改变 Agent 是否信任页面、是否转 OCR 的行为；纯解析、低误伤、跨宿主一致 |
| 合并单元格静默丢失 | 已先加入诊断 | 当前立即可靠的动作是显式告知结构未保留；直接输出 HTML 前必须先有正确 span 模型 |
| 图片/图表不可见 | DOCX 栅格图片已加入安全占位符；其他视觉对象保留诊断 | Agent 可对 DOCX 图片形成按需外部 VLM 闭环，同时不为浏览器或其他格式增加过早的二进制 API |
| PDF 中间错误假成功 | 已修复 | 这是比新增启发式更优先的正确性缺陷 |

### 下一阶段优先实现

以下能力适合新增，但必须作为一个统一的 PDF 结构工程推进，而不是四组互不一致的字符串后处理。

| 能力 | Agent 场景 | 前置设计 | 验收门槛 |
| --- | --- | --- | --- |
| ~~PDF 多栏阅读顺序~~（已落地） | 学术论文、法律文书、报告进入 RAG | 内部 `PdfLayoutDocument` IR 已建于 `parse/pdf_layout.rs` | 已实现：按列重排并发 `pdf_multi_column_reading_order` warning，失败回退原始顺序 |
| ~~PDF 页眉/页脚分类~~（已落地） | 避免重复噪声污染检索与回答 | 跨页位置与文本重复统计 | 已实现：去重保留首现 + `pdf_repeated_region_deduplicated` warning + 四宿主 `keep_repeated_regions` 保留选项；掏空页面时整体放弃 |
| ~~PDF 标题层级~~（outline 已落地） | 章节分块、父级元数据、目录导航 | outline 优先；字号/字重/编号启发式作为推断 | 已实现 outline 确定性来源（整行匹配才提升、找不到不伪造）；字号推断仍需带置信度后再进入 |
| ~~PDF 文本层清洗~~（断词已落地） | 修复断词、连字、重复绘制 | 基于 span/行模型，Unicode 字符级处理 | 已实现保守断词重合（小写-小写、复合词保留连字符、软连字符清除、字符级）；重复绘制抑制此前已落地 |
| ~~PDF 超链接~~（已落地） | Agent 获取可执行的下一步来源 | annotation/anchor 与文本 span 关联 | 已实现：rect 边界切分 span 精确锚定，跨行 anchor 空白不敏感匹配，无法关联时页尾 `<url>` 兜底；仅放行 http/https/mailto |
| 合并表格 HTML 降级 | 保留 DOCX/PPTX rowspan/colspan | 统一 `TableIR`，正确处理 continuation cell | 有真实合并 fixture；结构不确定时保留 warning，不伪造 HTML |
| PPTX 与非栅格视觉占位符 | Agent 决定是否调用外部 VLM | relationship、类型、位置、alt/caption、稳定 id | 文本流位置稳定；可把外部 caption 回填到相同 id |

建议的 `PdfLayoutIR` 最小结构：

```text
Document
  pages[]
    page_number
    width / height
    spans[]
      text
      bbox
      font_size
      font_flags
      source_order
    links[]
    diagnostics[]
```

标题、多栏、页眉页脚、文本清洗和链接都依赖这层数据。先建立一个可测试的中间模型，能减少重复启发式，并让每次推断都能附带来源与诊断。

### 资源与超时

调研把“提取超时与资源上界”列为 P0，需求判断正确，但不能通过一个表面上的 `timeout` 参数完成。

当前限制：

- Rust 原生同步解析不能安全强杀当前线程。
- 浏览器主线程中的同步 WASM 调用不能被普通 JavaScript timeout 中断。
- native/FFI 或单个长循环即使被 future timeout 包裹，也可能继续占用 CPU。
- 仅按输入字节限制，无法覆盖大量 PDF 对象、XML 事件或深层嵌套带来的计算量。

因此当前不增加一个无法保证生效的 core timeout。正确路线是：

1. ~~为每个解析器定义可测量的 work unit~~（已落地：`max_work_units` 与稳定错误 `work_budget_exceeded`）。
2. ~~在循环和递归边界执行合作式预算检查，超限返回稳定错误与已知位置~~（已落地，见 `limits.rs` 的 work budget）。
3. CLI/服务端对不可信输入使用独立 worker 进程或容器，由宿主执行真实 wall-clock timeout 和 RSS 限制。
4. 浏览器将解析放入 Web Worker；超时后终止 Worker，而不是阻塞主线程。
5. 对每个宿主分别验证 timeout/cancel 真正生效，禁止只验证配置能被传入。

core 侧合作式预算（第 1、2 步）已落地；剩余第 3–5 步是宿主级配套，仍列为平台加固 P1，而不是用不可兑现的参数冒充 P0 已完成。

### 后续按信号投入

| 能力 | 何时值得做 | 当前不优先原因 |
| --- | --- | --- |
| PDF 表格结构还原 | 有明确 PDF 表格摄取用户与质量数据后 | 难度最高；半成品比纯文本更容易误导 Agent |
| DOCX OMML 转 LaTeX | 科研/技术文档采用信号增强后 | 单源需求，且公式转换边界复杂 |
| XLSX 语义样式 | Agent 确实需要通过颜色/粗体判断业务语义时 | 样式常是装饰，也可能携带语义；需定义可移植 schema |
| 编码探测加固 | 新增真实失败样本后 | 当前已有 UTF-8/GBK/UTF-16 等覆盖，继续扩展应由 fixture 驱动 |
| 图片导出适配器 | 外部 VLM 回填协议稳定后 | 当前通用入口仅支持按安全占位符提取单个 DOCX 图片；批量导出和更多格式实现仍需按采用信号投入 |
| 输出无损不变式测试 | 与结构化 IR 同步推进 | 很有价值，但“无损”的定义必须按格式与场景拆分 |
| 密码文档解析 | 有可信密钥输入与宿主安全模型后 | 密钥生命周期、日志泄露、浏览器交互和错误语义都未定义 |

### 明确不进入核心

| 能力 | 决策 | 原因 |
| --- | --- | --- |
| 内置 OCR/VLM | 不做，保留外接路由 | 模型体积、内存、幻觉、更新频率和离线依赖破坏核心定位 |
| bbox 级公共 grounding API | 暂不作为产品能力 | bbox 是内部布局 IR 的必要数据，但公开契约会显著扩大兼容负担 |
| 表格转图片 | 不做 | 不改善 Agent 的结构理解，增加二进制和视觉处理负担 |
| 自定义分页符 flag | 不做 | 很少改变 Agent 决策，优先级低于结构和完整性 |
| 自动执行宏、公式、notebook code | 不做 | 破坏确定性和安全边界 |

## Agent 集成建议

Agent 应把 warnings 当作控制信号，不只是日志：

```python
result = spoor.parse_bytes(data, source_name=name)

for warning in result.warnings:
    code = warning["code"]
    location = warning.get("location")

    if code in {"pdf_page_no_text_layer", "pdf_page_suspicious_text_layer"}:
        route_location_to_external_ocr_or_vlm(location)
    elif code == "embedded_visuals_omitted":
        extract_relevant_docx_placeholders_or_mark_context_incomplete(location)
    elif code == "merged_table_structure_not_preserved":
        avoid_table_fact_extraction_without_review(location)
```

建议的信任策略：

- 无 warning：表示未发现已知降级，不代表内容事实正确。
- 有 location warning：保留其余位置，只隔离受影响位置。
- 有文档级 warning：把整份输出标记为不完整，避免做高风险结构化抽取。
- 有 recoverable error：根据 `code` 路由外部能力或请求用户补充。
- 有 non-recoverable error：停止自动重试，向用户说明输入边界。

## 质量门槛

任何新的“读得更对”能力必须同时满足：

1. **真实 fixture**：至少包含正常、边界、反例和恶意/高复杂度样本。
2. **失败可见**：不能确定时保留原始顺序/文本，并返回稳定 warning。
3. **跨宿主等价**：Rust、CLI、Python、Node、WASM 的 code 与位置一致。
4. **Unicode 安全**：文本处理基于字符或 grapheme，不做字节级切割。
5. **预算可测**：明确字节、对象、事件或工作量上界。
6. **不以演示替代质量**：单个漂亮样本不足以上线 PDF 表格或标题推断。
7. **向后兼容**：新增字段优先使用可选字段；稳定 code 一旦公开，不随消息文案变化。

## 路线建议

### 阶段 A：完整性诊断

状态：已落地。

- 文档解析 warnings 真正贯通 core 与绑定。
- 页/slide 结构化位置。
- PDF 混合无文本页、可疑文本层诊断。
- Office 合并表格和视觉对象省略诊断。
- CLI in-band warning。
- PDF 中间页错误不再静默截断。
- PDF 分页提取不再为每页重复构建页映射。

### 阶段 B：PDF 结构纵深

状态：已基本完成（2026-07）。

- ~~建立 `PdfLayoutIR`~~：内部 `PdfLayoutDocument` 已建于 `parse/pdf_layout.rs`。
- ~~多栏阅读顺序~~：已落地（几何列检测 + 重排 + warning）。
- ~~文本表格重建~~：已落地（空格对齐表格保守重建为 GFM）。
- ~~超链接~~：已落地（rect 切分 span 锚定 + 页尾兜底 + scheme 白名单）。
- ~~标题层级~~：outline 确定性来源已落地；无 outline 的字号推断待置信度设计。
- ~~页眉/页脚分类~~：已落地（跨页统计 + 去重保留首现 + keep 选项四宿主贯通）。
- ~~断词清洗~~：已落地（保守小写-小写重合 + 软连字符清除）。
- 剩余：无 outline 标题推断、复杂视觉表格结构、跨页断词。

### 阶段 C：运算量上限与取消

状态：core 侧已落地，宿主侧待做。

- ~~设计跨解析器 work unit~~：已落地（`max_work_units` / `work_budget_exceeded`）。
- ~~在 XML/PDF/表格循环中合作式检查~~：已落地。
- 为 CLI/服务示例提供独立 worker 隔离方案。
- 为浏览器示例提供 Web Worker 取消方案。

### 阶段 D：Office 结构增强

- 建立 `TableIR`，实现合并表格 HTML 降级。
- 建立视觉对象稳定 id、alt/caption 与外部 VLM 回填协议。
- 根据采用信号评估 OMML、comments/endnotes 和 PPTX shape 顺序。

## 外部证据与使用方式

以下 issue 验证了 Agent/RAG 场景中的真实痛点：

- [Docling #287: 标题父子层级影响 chunking](https://github.com/docling-project/docling/issues/287)
- [MarkItDown #1211: Markdown 表格无法表达合并单元格](https://github.com/microsoft/markitdown/issues/1211)
- [Docling #960: 用户首先需要识别不可读 PDF](https://github.com/docling-project/docling/issues/960)
- [Docling #1283: 大型 PDF 长时间不返回](https://github.com/docling-project/docling/issues/1283)
- [Kreuzberg #766: 单页大量图片对象导致挂起，timeout 无法中断](https://github.com/kreuzberg-dev/kreuzberg/issues/766)

这些证据用于确认场景和风险，不直接决定实现。最终优先级仍由 Agent 行为收益、确定性、工程前置条件、误判成本和跨宿主可验证性共同决定。
