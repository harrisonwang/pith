# PDF 测试矩阵

PDF 覆盖 text layer 抽取、多栏阅读顺序、outline 标题、超链接、页眉页脚去重与断词清洗。PDF 是版面格式，不是语义格式，所以推断类能力全部保守优先、失败回退原文。

| Fixture | Test | 验证契约 | 价值 | 状态 | 后续缺口 |
| --- | --- | --- | --- | --- | --- |
| `pdf/01_basic.pdf` | `basic_text_layer` | 单页 text layer 可抽取并带 `## Page 1` | 基础 PDF 文本抽取 | passed | 标题/段落结构推断 |
| `pdf/02_multipage.pdf` | `multipage_has_page_boundaries` | 多页按顺序输出精确的 `## Page N` 边界 | 支持页码定位并防止只读第一页 | passed | 页眉页脚去重 |
| `pdf/03_ascii_only.pdf` | `ascii_baseline` | ASCII 文本不被编码处理破坏 | 最小稳定基线 | passed | Unicode PDF 字体映射 |
| `pdf/04_image_only.pdf` | `image_only_pdf_is_surfaced_for_vision_instead_of_failing` | 有图无文本层时不报错，渲染页骨架+图片 marker 交给视觉模型 | 让扫描件可经 `--extract`/视觉模型读取，而非死路 | passed | 更广泛编码/色彩空间 |
| `pdf/06_vector_only.pdf` | `no_text_and_no_images_returns_structured_error` | 无文本层且无图片时返回可解析的 JSON 错误 `pdf_no_extractable_content` | 防止 Agent 把空输出当成功并猜测内容 | passed | — |
| `pdf/05_mixed_text_and_image.pdf` | `mixed_pdf_reports_page_level_missing_text` | 混合 PDF 仅对无文本层页返回 `pdf_page_no_text_layer` 与页码 | Agent 可只把缺失页路由到外部 OCR/VLM | passed | 更广泛乱码/Type3 字体 corpus |
| `pdf/07_two_column.pdf` | `two_column_pdf_is_read_left_column_then_right_with_warning` | 双栏页按左栏→右栏重排并发 `pdf_multi_column_reading_order` 页级 warning | 论文/报告不再交错成噪声，Agent 可按 warning 回退 | passed | 三栏/侧注/跨栏元素 |
| `pdf/08_links.pdf` | `uri_link_annotations_are_woven_into_markdown` | 锚定链接就地 `[锚文本](url)`，无锚点目标页尾 `<url>` 兜底，javascript:/file: 丢弃 | URL 永不丢失且不引入可执行 scheme | passed | GoTo 内部链接、跨页 anchor |
| `pdf/09_outline.pdf` | `outline_titles_promote_matching_lines_to_headings` | outline 标题恰为整行时提升为 ###/####（封顶 h6），找不到不伪造 | 章节分块与目录导航有真实来源（outline），零推断 | passed | 无 outline 时字号/字重推断（须带置信度） |
| `pdf/10_header_footer.pdf` | `repeated_headers_and_footers_deduplicate_with_warning` + `keep_repeated_regions_retains_verbatim_page_text` | 跨页重复页眉/页脚去重保留首现，发 `pdf_repeated_region_deduplicated`；keep 选项四宿主等价保留逐字原文 | 去掉检索/分块噪声且信息不静默丢失 | passed | 中缝/旋转水印文本 |
| `pdf/11_hyphenation.pdf` | `line_end_hyphenation_rejoins_conservatively` | 小写-小写断词重合、复合词保留连字符；独立减号/大写/数字/CJK 不动 | 检索 token 完整、引文可核验 | passed | 词典辅助歧义消解（暂不做） |

## 明确不覆盖

- image-only PDF：不做 OCR；有图的扫描件交给视觉模型（页骨架+图片 marker），无文本且无图片时返回 `pdf_no_extractable_content` 结构化错误。
- 三栏及以上、侧注等复杂版面：多栏检测保守，仅在清晰中央栏沟时重排，否则保持原序。
- 无 outline 的标题推断：字号/字重启发式须带 `source=inferred` 与置信度后再进入。
- 可疑文本层仅做保守诊断，不自动改写或 OCR。

## 下一批优先用例

- 三栏/侧注版面 fixture 与回退断言。
- 跨页断词（当前只在页内合并）。
- 真实扫描研报 corpus 的页眉页脚回归（含旋转水印文本反例）。
