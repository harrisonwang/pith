use crate::engine::DocumentFilter;
use crate::error::StructuredError;
use crate::limits;
use crate::parse::ExtractedMarkdown;
use crate::parse::pdf_layout::{
    PdfImageObject, PdfLayoutDocument, PdfLayoutPage, reading_order_text,
};
use crate::parse::pdf_media;
#[cfg(test)]
use crate::parse::pdf_media::PageImage;
use crate::result::{ProvenanceSpan, SourceAnchor, SpoorWarning, TextRange, WarningCode};
use crate::source::Source;
use anyhow::{Result, anyhow};

/// Minimum painted Bézier curves on a page for it to count as bearing a
/// vector-drawn figure (flowchart, chart, diagram). Curves — rounded boxes,
/// arrowheads, smooth shapes — are the high-precision signal a table of rules
/// and cell fills does not produce. Validated across real PDFs to separate
/// figure pages from plain text/table pages; kept conservative to favor
/// precision over recall. Calibrated on a real-document corpus: genuine
/// flowcharts/diagrams land at 48–92+ curves while pages with a few decorative
/// rounded text boxes sit at ~12–16, so 20 keeps figures and drops decoration.
const VECTOR_FIGURE_MIN_CURVES: u32 = 20;

fn is_vector_figure(ink: &super::pdf_engine::VectorInk) -> bool {
    ink.curves >= VECTOR_FIGURE_MIN_CURVES
}

pub fn extract(
    source: &Source<'_>,
    document_filter: &DocumentFilter,
    max_parse_bytes: usize,
) -> Result<ExtractedMarkdown> {
    let page_range = document_filter.page_range;

    // Load and decrypt the PDF once; every per-page pass below reuses this single
    // parse instead of re-loading the document for each (text, spans, images,
    // page count) as it did before.
    let doc = super::pdf_engine::load_pdf(source.bytes()).map_err(map_pdf_error)?;
    let page_count = doc.get_pages().len();

    // A page slice starting past the end is a clear caller error; say so instead
    // of letting the empty result fall through to "no extractable content".
    if let Some((first, _)) = page_range {
        if first > page_count {
            return Err(StructuredError::parse_failed(
                format!("请求的页码超出文档范围：起始页 {first} 超过总页数 {page_count}。"),
                crate::error::ParseStage::Parse,
            )
            .into());
        }
    }

    let pages =
        super::pdf_engine::extract_text_by_page_range(&doc, page_range).map_err(map_pdf_error)?;

    // Best-effort: positioned spans let us rebuild reading order for multi-column
    // pages. A discovery failure just leaves the content-stream-ordered text as
    // is, so this never regresses the flat-text path.
    let span_pages: std::collections::HashMap<usize, super::pdf_engine::EnginePage> =
        super::pdf_engine::extract_spans_by_page_range(&doc, page_range)
            .unwrap_or_default()
            .into_iter()
            .collect();

    // Replace only confidently multi-column pages with column-ordered text;
    // single-column pages keep their existing output verbatim. Remember which
    // pages were reordered so the agent can be told (and can fall back).
    let mut reordered_pages: Vec<usize> = Vec::new();
    let pages: Vec<(usize, String)> = pages
        .into_iter()
        .map(|(number, flat)| match span_pages.get(&number) {
            Some(page) => match reading_order_text(page) {
                Some((ordered, true)) => {
                    reordered_pages.push(number);
                    (number, ordered)
                }
                _ => (number, flat),
            },
            None => (number, flat),
        })
        .collect();

    // Classify and deduplicate cross-page repeated headers/footers before any
    // per-page rewriting: the first occurrence stays, later repeats are
    // dropped, and each region draws a stable warning naming what moved.
    // `keep_repeated_regions` skips the pass for consumers that need verbatim
    // page text.
    let mut pages = pages;
    let mut furniture_warnings: Vec<SpoorWarning> = Vec::new();
    if !document_filter.keep_repeated_regions {
        for region in super::pdf_furniture::dedupe_repeated_regions(&mut pages) {
            let pages_list = format_page_list(&region.removed_from_pages);
            furniture_warnings.push(SpoorWarning::new(
                WarningCode::PdfRepeatedRegionDeduplicated,
                format!(
                    "跨页重复的页眉/页脚「{}」已去重：保留首次出现，移除第 {pages_list} 页的重复；需要逐字原文时可开启 keep_repeated_regions。",
                    region.sample
                ),
            ));
        }
    }

    // Promote outline-named lines to Markdown headings first (the PDF's own
    // structure declaration; nothing is inferred), then weave URI link
    // annotations into the text: anchored links become [text](url) in place,
    // and targets whose anchor cannot be recovered are appended as autolinks,
    // so the agent never loses a destination.
    let mut headings_by_page: std::collections::HashMap<usize, Vec<(usize, String)>> =
        std::collections::HashMap::new();
    for entry in super::pdf_engine::outline_headings(&doc) {
        headings_by_page
            .entry(entry.page)
            .or_default()
            .push((entry.level, entry.title));
    }
    let pages: Vec<(usize, String)> = pages
        .into_iter()
        .map(|(number, text)| {
            let text = match headings_by_page.get(&number) {
                Some(headings) => super::pdf_outline::apply_headings(&text, headings),
                None => text,
            };
            let links = span_pages
                .get(&number)
                .map(super::pdf_links::page_links)
                .unwrap_or_default();
            if links.is_empty() {
                (number, text)
            } else {
                (number, super::pdf_links::apply_links(&text, &links))
            }
        })
        .collect();

    // Best-effort: locate image XObjects so the renderer can mark their page
    // position and warn. A discovery failure just yields no image markers.
    let images = pdf_media::discover_images_from_doc(&doc, pages.len(), page_range);

    // A vector-drawn figure (flowchart/chart/diagram) on a text-bearing page with
    // no raster image slips past both existing signals: it has text (so not
    // pdf_no_extractable_content) and no image XObject (so not
    // embedded_visuals_omitted). Flag those pages so the renderer marks them with
    // an extractable handle and the agent can pull the page as SVG for a VLM.
    let vector_figures: Vec<bool> = pages
        .iter()
        .enumerate()
        .map(|(index, (number, text))| {
            let has_text = !text.trim().is_empty();
            let has_images = images.get(index).is_some_and(|imgs| !imgs.is_empty());
            has_text
                && !has_images
                && span_pages
                    .get(number)
                    .is_some_and(|page| is_vector_figure(&page.vector))
        })
        .collect();
    let layout =
        PdfLayoutDocument::from_numbered_page_text_and_images(pages, images, vector_figures);

    // A PDF with no text layer is only a dead end when it also has no images to
    // hand off. When it *does* (a scan or an exported diagram), surface the page
    // skeleton plus image markers/handles and let the agent read them with a
    // vision model — hard-failing here would block exactly that handoff.
    if !layout.has_text() && !layout.has_images() {
        return Err(StructuredError::pdf_no_extractable_content().into());
    }

    let (markdown, provenance) = render_layout(&layout);
    limits::ensure_parse_size(markdown.len(), max_parse_bytes, "PDF Markdown rendering")?;

    let mut warnings = layout_warnings(&layout);
    warnings.extend(furniture_warnings);
    for number in reordered_pages {
        warnings.push(SpoorWarning::at_page(
            WarningCode::PdfMultiColumnReadingOrder,
            format!("第 {number} 页为多栏版面，已按栏重排顺序；若顺序异常可回退原始文本。"),
            number,
        ));
    }

    Ok(ExtractedMarkdown {
        markdown,
        warnings,
        // Total pages regardless of any --pages slice, so a cheap one-page peek
        // still tells the caller how big the document is.
        page_count: Some(page_count),
        provenance,
    })
}

#[cfg(test)]
fn render_pages(pages: &[String], images: &[Vec<PageImage>]) -> String {
    let layout = PdfLayoutDocument::from_page_text_and_images(pages.to_vec(), images.to_vec());
    render_layout(&layout).0
}

/// Render the page-oriented Markdown and, alongside it, a page-level provenance
/// span per page: the half-open byte range its `## Page N` block occupies in the
/// returned Markdown, mapped to that source page. Computing it here is free —
/// each page's start/end offset is already known while concatenating.
fn render_layout(layout: &PdfLayoutDocument) -> (String, Vec<ProvenanceSpan>) {
    let mut markdown = String::new();
    let mut image_number = 0usize;
    let mut spans = Vec::with_capacity(layout.pages.len());

    for (index, page) in layout.pages.iter().enumerate() {
        if index > 0 {
            markdown.push_str("\n\n");
        }

        // The block spans from here (after any inter-page separator) to the end
        // of this page's rendered text, so a quote landing anywhere in it maps
        // back to this page; the 2-byte gap between blocks belongs to no page.
        let start = markdown.len();
        render_page(page, &mut markdown, &mut image_number);
        spans.push(ProvenanceSpan {
            output: TextRange {
                start,
                end: markdown.len(),
            },
            source: SourceAnchor::Page {
                number: page.number,
            },
        });
    }

    (markdown, spans)
}

fn render_page(page: &PdfLayoutPage, markdown: &mut String, image_number: &mut usize) {
    let number = page.number;
    markdown.push_str(&format!("## Page {number}\n\n"));
    // Recover real Markdown tables from the space-aligned columns the PDF text
    // layer preserves; prose is left untouched. Helps the human preview and the
    // agent reading this output equally.
    markdown.push_str(&super::pdf_tables::tableize(page.text().trim()));

    for image in &page.images {
        *image_number += 1;
        markdown.push_str("\n\n");
        render_image_marker(number, *image_number, image, markdown);
    }

    if page.has_vector_figure {
        // A real handle, mirroring the extractable-image marker: `--extract
        // spoor://pdf/page/N` renders the page (text + vector shapes) to SVG.
        markdown.push_str("\n\n");
        markdown.push_str(&format!(
            "![PDF figure (p{number})](spoor://pdf/page/{number})"
        ));
    }
}

fn render_image_marker(
    page_number: usize,
    image_number: usize,
    image: &PdfImageObject,
    markdown: &mut String,
) {
    if image.extractable {
        // A real handle: `--extract` returns the JPEG/JPEG2000 bytes.
        markdown.push_str(&format!(
            "![PDF image {image_number} (p{page_number})](spoor://pdf/obj/{}/{})",
            image.id, image.generation
        ));
    } else {
        // Present but not directly extractable; mark position only so
        // the agent knows the page is more than its text.
        markdown.push_str(&format!(
            "[PDF image {image_number} (p{page_number})：内嵌图，编码需外部渲染]"
        ));
    }
}

#[cfg(test)]
fn page_warnings(pages: &[String], images: &[Vec<PageImage>]) -> Vec<SpoorWarning> {
    let layout = PdfLayoutDocument::from_page_text_and_images(pages.to_vec(), images.to_vec());
    layout_warnings(&layout)
}

fn layout_warnings(layout: &PdfLayoutDocument) -> Vec<SpoorWarning> {
    let mut warnings = Vec::new();
    for page in &layout.pages {
        let number = page.number;
        if page.text().trim().is_empty() {
            warnings.push(SpoorWarning::at_page(
                WarningCode::PdfPageNoTextLayer,
                format!("第 {number} 页无文本层，输出为空；需 VLM 处理。"),
                number,
            ));
        } else if suspicious_text_layer(page.text()) {
            warnings.push(SpoorWarning::at_page(
                WarningCode::PdfPageSuspiciousTextLayer,
                format!("第 {number} 页文本层含乱码或占位符，可能不可靠；建议用 VLM 交叉验证。"),
                number,
            ));
        }

        if !page.images.is_empty() {
            let total = page.images.len();
            let extractable = page.images.iter().filter(|image| image.extractable).count();
            let unextractable = total - extractable;
            let message = if extractable == total {
                format!(
                    "第 {number} 页有 {total} 张位图未转成文本；可携带 --extract <链接> 参数提取图片，交由 VLM 识别。"
                )
            } else if extractable == 0 {
                format!(
                    "第 {number} 页有 {total} 张位图未转成文本，但编码无法直接导出；需用外部工具渲染该页后交 VLM 识别。"
                )
            } else {
                format!(
                    "第 {number} 页有 {total} 张位图未转成文本：{extractable} 张可携带 --extract <链接> 参数提取，其余 {unextractable} 张编码无法直接导出、需外部渲染。"
                )
            };
            warnings.push(SpoorWarning::at_page(
                WarningCode::EmbeddedVisualsOmitted,
                message,
                number,
            ));
        }

        if page.has_vector_figure {
            warnings.push(SpoorWarning::at_page(
                WarningCode::VectorGraphicsOmitted,
                format!(
                    "第 {number} 页含矢量图形/图表未转成文本；该页末尾已附 spoor://pdf/page/{number} 链接，可携带 --extract <链接> 参数提取该页 SVG 图，交由 VLM 识别。"
                ),
                number,
            ));
        }
    }
    warnings
}

/// Human-readable page list for a warning message: "2、3、4"; long lists are
/// elided after eight entries.
fn format_page_list(pages: &[usize]) -> String {
    const SHOWN: usize = 8;
    let mut listed = pages
        .iter()
        .take(SHOWN)
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join("、");
    if pages.len() > SHOWN {
        listed.push_str(&format!("…（共 {} 页）", pages.len()));
    }
    listed
}

fn suspicious_text_layer(page: &str) -> bool {
    if page.chars().any(|character| {
        character == '\u{fffd}' || (character.is_control() && !character.is_whitespace())
    }) {
        return true;
    }

    let lower = page.to_ascii_lowercase();
    lower.match_indices("glyph<").nth(1).is_some()
}

/// A password-protected PDF is a hard boundary like an image-only one: no
/// retry or flag can succeed, so it gets a structured, branchable error
/// instead of the library's misleading "password is incorrect" text (lopdf
/// probes with an empty password the user never supplied).
fn map_pdf_error(error: super::pdf_engine::OutputError) -> anyhow::Error {
    match error {
        super::pdf_engine::OutputError::PdfError(super::pdf_engine::Error::Decryption(_)) => {
            StructuredError::encrypted_pdf().into()
        }
        super::pdf_engine::OutputError::WorkBudgetExceeded => {
            StructuredError::work_budget_exceeded().into()
        }
        error => anyhow!("pdf-extract failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{map_pdf_error, page_warnings, render_pages, suspicious_text_layer};
    use crate::error::{ErrorCode, StructuredError};
    use crate::result::{WarningCode, WarningLocation};

    #[test]
    fn page_boundaries_preserve_blank_pages() {
        let pages = vec!["first".into(), " \n".into(), "third".into()];
        let images = vec![Vec::new(); pages.len()];

        assert_eq!(
            render_pages(&pages, &images),
            "## Page 1\n\nfirst\n\n## Page 2\n\n\n\n## Page 3\n\nthird"
        );
    }

    #[test]
    fn extractable_image_renders_handle_others_render_marker() {
        let pages = vec!["text".to_string()];
        let images = vec![vec![
            super::PageImage {
                id: 7,
                generation: 0,
                extractable: true,
            },
            super::PageImage {
                id: 9,
                generation: 0,
                extractable: false,
            },
        ]];

        let markdown = render_pages(&pages, &images);
        assert!(markdown.contains("![PDF image 1 (p1)](spoor://pdf/obj/7/0)"));
        assert!(markdown.contains("[PDF image 2 (p1)：内嵌图，编码需外部渲染]"));
    }

    #[test]
    fn decryption_failure_maps_to_encrypted_pdf() {
        let error = map_pdf_error(super::super::pdf_engine::OutputError::PdfError(
            super::super::pdf_engine::Error::Decryption(
                super::super::pdf_engine::encryption::DecryptionError::IncorrectPassword,
            ),
        ));

        let structured = error
            .downcast_ref::<StructuredError>()
            .expect("structured error");
        assert_eq!(structured.code, ErrorCode::EncryptedPdf);
        assert!(!structured.recoverable);
    }

    #[test]
    fn other_pdf_errors_stay_unstructured() {
        let error = map_pdf_error(super::super::pdf_engine::OutputError::FormatError(
            std::fmt::Error,
        ));

        assert!(error.downcast_ref::<StructuredError>().is_none());
        assert!(error.to_string().contains("pdf-extract failed"));
    }

    #[test]
    fn mixed_pdf_reports_blank_page_without_failing_document() {
        let pages = vec!["text".to_string(), " \n".to_string(), "more".to_string()];
        let warnings = page_warnings(&pages, &vec![Vec::new(); pages.len()]);

        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, WarningCode::PdfPageNoTextLayer);
        assert_eq!(
            warnings[0].location,
            Some(WarningLocation::Page { number: 2 })
        );
    }

    #[test]
    fn pages_with_images_warn_with_page_location() {
        let pages = vec!["text".to_string(), "more".to_string()];
        let images = vec![
            Vec::new(),
            vec![super::PageImage {
                id: 5,
                generation: 0,
                extractable: false,
            }],
        ];

        let warnings = page_warnings(&pages, &images);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, WarningCode::EmbeddedVisualsOmitted);
        assert_eq!(
            warnings[0].location,
            Some(WarningLocation::Page { number: 2 })
        );
    }

    #[test]
    fn vector_figure_threshold_keys_on_curves_not_fills() {
        use crate::parse::pdf_engine::VectorInk;
        // Curves are the signal: at the threshold it counts as a figure.
        assert!(super::is_vector_figure(&VectorInk {
            curves: super::VECTOR_FIGURE_MIN_CURVES,
            fills: 0,
            strokes: 0,
        }));
        // A fill-heavy, curve-light page (shaded table / colored boxes) is not.
        assert!(!super::is_vector_figure(&VectorInk {
            curves: super::VECTOR_FIGURE_MIN_CURVES - 1,
            fills: 300,
            strokes: 0,
        }));
    }

    #[test]
    fn vector_figure_page_warns_and_marks_with_extract_handle() {
        use crate::parse::pdf_layout::PdfLayoutDocument;
        // A text-bearing page flagged as a vector figure (no raster image).
        let layout = PdfLayoutDocument::from_numbered_page_text_and_images(
            vec![(1usize, "diagram caption".to_string())],
            vec![Vec::new()],
            vec![true],
        );

        let warnings = super::layout_warnings(&layout);
        assert!(
            warnings
                .iter()
                .any(|w| w.code == WarningCode::VectorGraphicsOmitted),
            "figure page must warn"
        );

        let markdown = super::render_layout(&layout).0;
        assert!(
            markdown.contains("spoor://pdf/page/1"),
            "figure page must carry an extractable page handle: {markdown}"
        );
    }

    #[test]
    fn plain_page_has_no_vector_figure_signal() {
        use crate::parse::pdf_layout::PdfLayoutDocument;
        let layout = PdfLayoutDocument::from_numbered_page_text_and_images(
            vec![(1usize, "just prose".to_string())],
            vec![Vec::new()],
            vec![false],
        );
        assert!(
            !super::layout_warnings(&layout)
                .iter()
                .any(|w| w.code == WarningCode::VectorGraphicsOmitted)
        );
        assert!(
            !super::render_layout(&layout)
                .0
                .contains("spoor://pdf/page/")
        );
    }

    #[test]
    fn suspicious_text_detection_is_conservative() {
        assert!(suspicious_text_layer("GLYPH<28> GLYPH<27>"));
        assert!(suspicious_text_layer("valid text \u{fffd}"));
        assert!(!suspicious_text_layer("正常中文、代码 glyph<T> 与正文"));
    }
}
