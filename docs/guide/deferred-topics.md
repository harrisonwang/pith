# Deferred topics（Getting Started 期间裁掉的旁支）

Pass C 后作为 Diving Deeper 候选复核。多数已在 `overall-structure.md` 的 DD 列表里。

- **表格 JSON 的全局 envelope（`schema_version` / `usage` / 顶层 `truncated`）** —— 这是 CLI 输出（`JsonOutput`）的形态；pyspoor 的 `TableResult` 直接给 `tables` + `serialized_bytes`，自描述落在每个 table entry 上。差异本身值得在 DD-5（输出契约）讲清。
- **`extract_media` 取内嵌图片/矢量图交给 VLM** —— 步骤 4 的 `embedded_visuals_omitted` / `vector_graphics_omitted` warning 是入口，但取图流程刻意留给 DD-2（本轮未选）。
- **`detect_format` 独立调用** —— GS 未单独展示；属 Reference/api。
- **provenance（答案溯源）** —— 明确留给 DD-1。
- **预算与 ZIP 炸弹防御的"为什么"** —— 步骤 5 只示范了按 code 兜 `parse_budget_exceeded`；设计动机留给 DD-4。
