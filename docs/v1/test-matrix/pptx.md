# PPTX 测试矩阵

PPTX 测试重点是 slide 级边界、文本内容、表格和 speaker notes。视觉主题、动画、转场默认不属于 LLM mode 契约。

| Fixture | Test | 验证契约 | 价值 | 状态 | 后续缺口 |
| --- | --- | --- | --- | --- | --- |
| `pptx/01_basic.pptx` | `basic_slides_with_titles_and_bullets` | 每页输出 `## Slide N`；title/ctrTitle 占位符 → `###` 标题并置顶；body/obj 占位符段落 → bullet 列表（`lvl` 嵌套缩进）；subTitle → 正文 | 保留演示文稿阅读边界与大纲语义 | passed | — |
| `pptx/02_with_table.pptx` | `tables_in_slides` | slide 内表格输出 GFM table | 防止表格被压平成一列文本 | passed | merged table cells |
| `pptx/03_with_notes.pptx` | `speaker_notes_are_included` | speaker notes 输出到 slide 下方 | notes 常包含演讲者真实上下文 | passed | notes 与 slide text 的顺序/标题规范 |
| `pptx/04_empty.pptx` | `empty_deck_with_blank_slide` | 空白 slide 输出稳定 | 边界输入 | passed | 是否省略完全空 slide |
| `pptx/05_ordering.pptx` | `slide_ordering_handles_double_digits`、`slide_narrowing_follows_source_numbers_and_reports_full_count`（core api.rs） | `slide11.xml` 排在 `slide2.xml` 之后；`--pages 2:3` 只出 Slide 2/3 且编号跟随源页、`page_count` 报总数、起始越界报错 | 防止字典序导致 slide 顺序错误；slide 收窄与 PDF `--pages` 同契约 | passed | — |
| `pptx/06_merged_table.pptx` | `merged_table_and_visual_omissions_are_located_by_slide` | 合并表格返回 `merged_table_structure_not_preserved` 与 slide 位置 | Agent 不把降级 GFM 当原始结构 | passed | span 模型与 HTML 降级 |
| `pptx/07_embedded_visual.pptx` | `merged_table_and_visual_omissions_are_located_by_slide` | 图片省略返回 `embedded_visuals_omitted` 与 slide 位置 | Agent 可精确路由受影响 slide 到外部 VLM | passed | 稳定 visual id、alt/caption |
| `pptx/08_image_placeholders.pptx` | `image_placeholders_follow_slide_order_and_only_reference_safe_entries`、`slide_with_images_carries_extract_wording_in_warning`、`extract_outputs_the_referenced_pptx_media_bytes`、`extract_rejects_paths_that_were_not_emitted_as_safe_pptx_uris` | 内嵌图片按 slide 顺序输出安全 `spoor://pptx/part/ppt/media/*` 占位符；image 编号跨 slide 自增；OPC 校验拒绝跨容器/路径穿越；CLI `--extract` 原样输出单个安全资源 | Agent 可按需选择单张 slide 图片并取出交给 VLM，无需系统 unzip | passed | alt/caption、非 CLI 宿主提取接口 |
| `pptx/09_reordered.pptx` | `slides_follow_presentation_order_not_filename_order` | sldIdLst 把 slide3 排到首位时，输出按放映顺序编号（Slide 1 = Gamma） | slide 编号是锚点/警告的坐标系，必须与 PowerPoint 显示一致 | passed | — |
| `pptx/10_hidden_slide.pptx` | `hidden_slides_keep_their_number_and_surface_a_warning` | 隐藏页正文省略、`## Slide 2` 保号、返回 `hidden_slide_omitted` 定位到 slide 2 | Agent 不引用作者已撤下的内容，编号不塌缩错位 | passed | 后续可选 include-hidden 开关 |
| `pptx/11_image_only.pptx` | `image_only_slides_carry_the_no_text_layer_posture`（CLI）、`slide_provenance_tiles_output_with_slide_anchors`、`locate_quote_grounds_a_pptx_citation_to_its_slide`（core api.rs，用 01_basic） | 正文零文本+含视觉 → `slide_no_text_layer`，措辞区分演讲者备注有无；纯文本控制页不触发 | "什么都没拿到"与"拿到但不完整"是两档置信度，Agent 据此决定 VLM 是必选还是增强 | passed | chart-only 页的专属措辞 |
| `pptx/12_reading_order.pptx` | `reading_order_follows_geometry_not_z_order` | XML（z-）序为 Bottom/Top-right/Top-left 时，输出按几何序 Top-left/Top-right/Bottom | z-order 交错会让模型读到乱序文本 | passed | 多栏行带聚簇（纯 (top,left) 对双栏并列仍会交错） |
| `pptx/13_bullets.pptx` | `bullet_levels_numbering_and_opt_out_are_preserved` | `lvl` 嵌套缩进、`buAutoNum` → `1.`、`buNone` 退回正文 | 层级承载"子论点属于谁"的语义 | passed | buAutoNum 编号类型（罗马/字母）细分 |
| `pptx/14_alt_text.pptx` | `author_alt_text_rides_on_the_placeholder_sanitized` | `cNvPr@descr` 经消毒后附在占位符 alt 上；`](evil)` 注入被中和 | 作者写的图片描述帮 Agent 决定是否值得走 VLM | passed | — |
| `pptx/15_group_shapes.pptx` | `group_shapes_flatten_in_visual_order` | group 作为整体按自身位置排序，组内子 shape 按局部坐标排序展平 | 组合图形文本不丢、不与组外交错 | passed | 组内 chOff/负缩放极端情形 |
| `pptx/16_notes_furniture.pptx` | `notes_page_number_furniture_does_not_leak` | notes 里 `sldNum/dt/ftr/hdr/sldImg` 占位符文本被过滤，仅保留真实备注 | 模板页码数字混进备注是 Agent 毒药（Tika skipPlaceholders 教训） | passed | — |
| `pptx/17_no_presentation.pptx` | `missing_presentation_part_falls_back_to_filename_order` | 缺 presentation.xml（或其损坏）时确定性回退文件名数字序 | 手工/损坏包不失败、不猜 | passed | — |
| `pptx/18_chart.pptx` | `chart_data_is_extracted_as_a_table_with_no_incompleteness_warning` | chart part 缓存的 c:cat/c:val 按系列渲成带标题的 GFM 表；解构完整的图表页**零 warning** | 商业 deck 的核心数字常只在图表里；Agent 从"知道自己瞎"变成"拿到数字" | passed | scatter xVal/yVal 走同一路径但无 fixture；截断 note（>100 点 / >12 系列） |
| `pptx/19_smartart.pptx` | `smartart_node_text_is_extracted_as_a_list` | `ppt/diagrams/dataN.xml` 的 `dgm:t` 节点文本按数据模型序输出为列表；零 warning | python-pptx 系全体丢失 SmartArt 文本；不重建图形关系（那是版面解读） | passed | 未解析引用回落 `未能解构` warning（单元测试覆盖） |

## 下一批优先用例

- 多栏版式的行带聚簇阅读顺序（垂直重叠归同行，行内再按 left）。
- 在现有省略 warning 上增加 chart/image 稳定 id 与外部回填位。
- 合并表格 span 模型与 HTML 降级（与 DOCX 共用）。
