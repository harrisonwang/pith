#![allow(dead_code)]

use crate::parse::pdf_engine::{EnginePage, EngineSpan};
use crate::parse::pdf_media::PageImage;
use std::cmp::Ordering;

/// Internal PDF layout representation.
///
/// This is intentionally not part of the public API yet.  The first milestone
/// keeps the existing page-oriented text output stable while giving future PDF
/// work (span extraction, reading order, header/footer classification, links,
/// and chunk metadata) a single place to attach structure and diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfLayoutDocument {
    pub(crate) pages: Vec<PdfLayoutPage>,
    pub(crate) diagnostics: Vec<PdfLayoutDiagnostic>,
}

impl PdfLayoutDocument {
    pub(crate) fn from_page_text_and_images(
        pages: Vec<String>,
        images: Vec<Vec<PageImage>>,
    ) -> Self {
        let numbered_pages = pages
            .into_iter()
            .enumerate()
            .map(|(index, text)| (index + 1, text));
        Self::from_numbered_page_text_and_images(numbered_pages, images, Vec::new())
    }

    pub(crate) fn from_numbered_page_text_and_images(
        pages: impl IntoIterator<Item = (usize, String)>,
        images: Vec<Vec<PageImage>>,
        vector_figures: Vec<bool>,
    ) -> Self {
        let pages = pages
            .into_iter()
            .enumerate()
            .map(|(index, (number, text))| {
                let page_images = images.get(index).cloned().unwrap_or_default();
                let has_vector_figure = vector_figures.get(index).copied().unwrap_or(false);
                PdfLayoutPage::from_page_text(number, text, page_images, has_vector_figure)
            })
            .collect();

        Self {
            pages,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn has_text(&self) -> bool {
        self.pages.iter().any(PdfLayoutPage::has_text)
    }

    pub(crate) fn has_images(&self) -> bool {
        self.pages.iter().any(|page| !page.images.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfLayoutPage {
    pub(crate) number: usize,
    pub(crate) width: Option<f32>,
    pub(crate) height: Option<f32>,
    pub(crate) spans: Vec<PdfTextSpan>,
    pub(crate) lines: Vec<PdfTextLine>,
    pub(crate) blocks: Vec<PdfBlock>,
    pub(crate) images: Vec<PdfImageObject>,
    pub(crate) links: Vec<PdfLinkAnnotation>,
    /// The page bears a vector-drawn figure (flowchart/chart/diagram) the text
    /// path cannot convey. Drives both the in-body figure marker and the
    /// `vector_graphics_omitted` warning, mirroring how images drive theirs.
    pub(crate) has_vector_figure: bool,
    pub(crate) diagnostics: Vec<PdfLayoutDiagnostic>,
}

impl PdfLayoutPage {
    fn from_page_text(
        number: usize,
        text: String,
        images: Vec<PageImage>,
        has_vector_figure: bool,
    ) -> Self {
        let mut blocks = Vec::new();
        if !text.is_empty() {
            blocks.push(PdfBlock {
                kind: PdfBlockKind::Paragraph,
                text,
                bbox: None,
                reading_order: 0,
                confidence: 1.0,
            });
        }

        let images = images
            .into_iter()
            .enumerate()
            .map(|(index, image)| PdfImageObject {
                id: image.id,
                generation: image.generation,
                page_image_number: index + 1,
                bbox: None,
                extractable: image.extractable,
            })
            .collect();

        Self {
            number,
            width: None,
            height: None,
            spans: Vec::new(),
            lines: Vec::new(),
            blocks,
            images,
            links: Vec::new(),
            has_vector_figure,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn text(&self) -> &str {
        self.blocks
            .iter()
            .find(|block| matches!(block.kind, PdfBlockKind::Paragraph))
            .map(|block| block.text.as_str())
            .unwrap_or("")
    }

    pub(crate) fn has_text(&self) -> bool {
        !self.text().trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfTextSpan {
    pub(crate) text: String,
    pub(crate) bbox: Option<PdfRect>,
    pub(crate) font_size: Option<f32>,
    pub(crate) font_name: Option<String>,
    pub(crate) font_flags: PdfFontFlags,
    pub(crate) source_order: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfTextLine {
    pub(crate) spans: Vec<usize>,
    pub(crate) bbox: Option<PdfRect>,
    pub(crate) baseline_y: Option<f32>,
    pub(crate) column_hint: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfBlock {
    pub(crate) kind: PdfBlockKind,
    pub(crate) text: String,
    pub(crate) bbox: Option<PdfRect>,
    pub(crate) reading_order: usize,
    pub(crate) confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PdfBlockKind {
    Paragraph,
    Heading,
    ListItem,
    HeaderFooter,
    Watermark,
    Caption,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfImageObject {
    pub(crate) id: u32,
    pub(crate) generation: u16,
    pub(crate) page_image_number: usize,
    pub(crate) bbox: Option<PdfRect>,
    pub(crate) extractable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfLinkAnnotation {
    pub(crate) target: String,
    pub(crate) bbox: Option<PdfRect>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfLayoutDiagnostic {
    pub(crate) kind: PdfLayoutDiagnosticKind,
    pub(crate) page_number: Option<usize>,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PdfLayoutDiagnosticKind {
    NoTextLayer,
    SuspiciousTextLayer,
    ImageOmitted,
    ReadingOrderInferred,
    ReadingOrderUncertain,
    HeaderFooterClassified,
    HeadingInferred,
    LinkUnassociated,
    LayoutExtractionFailed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PdfRect {
    pub(crate) x0: f32,
    pub(crate) y0: f32,
    pub(crate) x1: f32,
    pub(crate) y1: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PdfFontFlags {
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) monospace: bool,
}

/// Reconstruct reading-order text from a page's positioned spans.
///
/// Returns the ordered text and whether a multi-column layout was detected and
/// applied. Returns `None` when there are no spans to work with, so the caller
/// keeps the existing content-stream-ordered text. Geometry alone is used (no
/// ML); the detection is deliberately conservative — when a page is not
/// confidently multi-column it is treated as a single column, which renders the
/// same top-to-bottom order the flat path already produces.
pub(crate) fn reading_order_text(page: &EnginePage) -> Option<(String, bool)> {
    if page.spans.is_empty() {
        return None;
    }
    let columns = detect_column_ranges(&page.spans, page.width);
    let multi_column = columns.len() > 1;

    let mut lines: Vec<Line> = Vec::new();
    for (lo, hi) in &columns {
        let mut col: Vec<&EngineSpan> = page
            .spans
            .iter()
            .filter(|span| {
                let center = (span.x0 + span.x1) / 2.0;
                if center.is_nan() {
                    // A NaN coordinate (degenerate/rotated transform) must not
                    // silently drop the span: keep it in the first column.
                    return *lo == f64::MIN;
                }
                center >= *lo && center < *hi
            })
            .collect();
        lines.extend(group_lines(&mut col));
    }

    let text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Some((text, multi_column))
}

struct Line {
    text: String,
}

/// Group a column's spans into visual lines (by baseline proximity), ordered
/// top-to-bottom, with each line's spans ordered left-to-right.
fn group_lines(spans: &mut [&EngineSpan]) -> Vec<Line> {
    spans.sort_by(|a, b| {
        a.y.partial_cmp(&b.y)
            .unwrap_or(Ordering::Equal)
            .then(a.x0.partial_cmp(&b.x0).unwrap_or(Ordering::Equal))
    });

    let mut lines: Vec<Line> = Vec::new();
    let mut current: Vec<&EngineSpan> = Vec::new();
    let mut current_y = 0.0_f64;
    for span in spans.iter() {
        let tol = span.font_size.max(1.0) * 0.6;
        if current.is_empty() {
            current_y = span.y;
            current.push(span);
        } else if (span.y - current_y).abs() <= tol {
            current.push(span);
        } else {
            lines.push(finish_line(&mut current));
            current_y = span.y;
            current.push(span);
        }
    }
    if !current.is_empty() {
        lines.push(finish_line(&mut current));
    }
    lines
}

fn finish_line(spans: &mut Vec<&EngineSpan>) -> Line {
    spans.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap_or(Ordering::Equal));
    let text = spans
        .iter()
        .map(|span| span.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    spans.clear();
    Line { text }
}

/// Detect column x-ranges. Returns a single full-width range unless a clear
/// central vertical gutter splits the page into two text-bearing columns.
fn detect_column_ranges(spans: &[EngineSpan], page_width: f64) -> Vec<(f64, f64)> {
    let single = vec![(f64::MIN, f64::MAX)];
    // Too few spans to trust a split; avoid false positives on sparse pages.
    if page_width <= 0.0 || spans.len() < 6 {
        return single;
    }

    let Some((gap_start, gap_end)) = central_gutter(spans, page_width) else {
        return single;
    };
    let gutter_width = gap_end - gap_start;
    let min_gutter = median_font_size(spans).max(page_width * 0.03);
    if gutter_width < min_gutter {
        return single;
    }

    // Require meaningful text fully on each side of the gutter.
    let left = spans.iter().filter(|s| s.x1 <= gap_start).count();
    let right = spans.iter().filter(|s| s.x0 >= gap_end).count();
    if left < 3 || right < 3 {
        return single;
    }

    let center = (gap_start + gap_end) / 2.0;
    vec![(f64::MIN, center), (center, f64::MAX)]
}

/// Widest horizontal gap, within the page's central band, that no span crosses.
fn central_gutter(spans: &[EngineSpan], page_width: f64) -> Option<(f64, f64)> {
    let mut intervals: Vec<(f64, f64)> = spans
        .iter()
        .filter(|s| s.x1 > s.x0)
        .map(|s| (s.x0, s.x1))
        .collect();
    intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));

    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (start, end) in intervals {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }

    let (lo, hi) = (page_width * 0.2, page_width * 0.8);
    let mut best: Option<(f64, f64)> = None;
    for pair in merged.windows(2) {
        let gap = (pair[0].1, pair[1].0);
        if gap.1 <= gap.0 {
            continue;
        }
        let center = (gap.0 + gap.1) / 2.0;
        if center < lo || center > hi {
            continue;
        }
        if best.is_none_or(|b| gap.1 - gap.0 > b.1 - b.0) {
            best = Some(gap);
        }
    }
    best
}

fn median_font_size(spans: &[EngineSpan]) -> f64 {
    let mut sizes: Vec<f64> = spans
        .iter()
        .map(|s| s.font_size)
        .filter(|f| *f > 0.0)
        .collect();
    if sizes.is_empty() {
        return 0.0;
    }
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    sizes[sizes.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::{EnginePage, EngineSpan, PdfBlockKind, PdfLayoutDocument, reading_order_text};
    use crate::parse::pdf_media::PageImage;

    fn span(text: &str, x0: f64, x1: f64, y: f64) -> EngineSpan {
        EngineSpan {
            text: text.to_string(),
            x0,
            x1,
            y,
            font_size: 10.0,
            link: None,
        }
    }

    #[test]
    fn page_text_and_images_seed_layout_skeleton() {
        let layout = PdfLayoutDocument::from_page_text_and_images(
            vec!["first".to_string(), "".to_string()],
            vec![
                vec![PageImage {
                    id: 3,
                    generation: 0,
                    extractable: true,
                }],
                Vec::new(),
            ],
        );

        assert_eq!(layout.pages.len(), 2);
        assert!(layout.has_text());
        assert!(layout.has_images());
        assert_eq!(layout.pages[0].number, 1);
        assert_eq!(layout.pages[0].text(), "first");
        assert_eq!(layout.pages[0].blocks[0].kind, PdfBlockKind::Paragraph);
        assert_eq!(layout.pages[0].images[0].id, 3);
        assert!(!layout.pages[1].has_text());
    }

    #[test]
    fn empty_page_yields_no_reading_order() {
        let page = EnginePage::default();
        assert!(reading_order_text(&page).is_none());
    }

    #[test]
    fn single_column_keeps_top_to_bottom_order() {
        // Plain stacked lines: not multi-column, ordered by y.
        let page = EnginePage {
            width: 600.0,
            height: 800.0,
            spans: vec![
                span("first line", 50.0, 200.0, 100.0),
                span("second line", 50.0, 210.0, 120.0),
                span("third line", 50.0, 190.0, 140.0),
            ],
            vector: Default::default(),
            links: Vec::new(),
        };
        let (text, multi_column) = reading_order_text(&page).expect("text");
        assert!(!multi_column);
        assert_eq!(text, "first line\nsecond line\nthird line");
    }

    #[test]
    fn two_columns_are_read_left_then_right_not_interleaved() {
        // Content-stream order interleaves the two columns row by row; geometric
        // reconstruction must emit the whole left column, then the whole right.
        let page = EnginePage {
            width: 600.0,
            height: 800.0,
            spans: vec![
                span("L1", 50.0, 250.0, 100.0),
                span("R1", 350.0, 550.0, 100.0),
                span("L2", 50.0, 250.0, 120.0),
                span("R2", 350.0, 550.0, 120.0),
                span("L3", 50.0, 250.0, 140.0),
                span("R3", 350.0, 550.0, 140.0),
            ],
            vector: Default::default(),
            links: Vec::new(),
        };
        let (text, multi_column) = reading_order_text(&page).expect("text");
        assert!(multi_column, "a clear central gutter must be detected");
        assert_eq!(text, "L1\nL2\nL3\nR1\nR2\nR3");
    }

    #[test]
    fn full_width_lines_are_not_split_into_columns() {
        // Lines that span the gutter (no clear vertical whitespace band) stay
        // single-column even though some text sits left and some right.
        let page = EnginePage {
            width: 600.0,
            height: 800.0,
            spans: vec![
                span(
                    "a wide heading spanning the whole page width",
                    50.0,
                    560.0,
                    100.0,
                ),
                span(
                    "another full width line of body text here",
                    50.0,
                    555.0,
                    120.0,
                ),
                span(
                    "and a third line that also spans across",
                    50.0,
                    540.0,
                    140.0,
                ),
                span(
                    "plus a fourth full width line of content",
                    50.0,
                    545.0,
                    160.0,
                ),
                span(
                    "with a fifth line to clear the span floor",
                    50.0,
                    550.0,
                    180.0,
                ),
                span("and a sixth line so detection can run", 50.0, 535.0, 200.0),
            ],
            vector: Default::default(),
            links: Vec::new(),
        };
        let (_, multi_column) = reading_order_text(&page).expect("text");
        assert!(!multi_column, "straddling lines must not be split");
    }

    #[test]
    fn nan_coordinate_span_is_not_silently_dropped() {
        // A span with NaN x-coords (e.g. from a rotated/degenerate text matrix)
        // must still appear in the reading-order output rather than vanishing
        // when its center fails both column-range comparisons.
        let page = EnginePage {
            width: 600.0,
            height: 800.0,
            spans: vec![
                span("normal", 50.0, 200.0, 100.0),
                EngineSpan {
                    text: "rotated".to_string(),
                    x0: f64::NAN,
                    x1: f64::NAN,
                    y: 120.0,
                    font_size: 10.0,
                    link: None,
                },
            ],
            vector: Default::default(),
            links: Vec::new(),
        };
        let (text, _) = reading_order_text(&page).expect("text");
        assert!(
            text.contains("rotated"),
            "NaN-coordinate span must not be dropped: {text:?}"
        );
        assert!(text.contains("normal"));
    }
}
