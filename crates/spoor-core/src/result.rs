use crate::detect::Format;
use crate::json_schema::TableEntry;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentResult {
    pub source: String,
    pub format: Format,
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableResult {
    pub tables: Vec<TableEntry>,
    pub serialized_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ParseContent {
    Document(DocumentResult),
    Tables(TableResult),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpoorWarning {
    pub code: WarningCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<WarningLocation>,
}

impl SpoorWarning {
    pub fn new(code: WarningCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            location: None,
        }
    }

    pub fn at_page(code: WarningCode, message: impl Into<String>, number: usize) -> Self {
        Self {
            code,
            message: message.into(),
            location: Some(WarningLocation::Page { number }),
        }
    }

    pub fn at_slide(code: WarningCode, message: impl Into<String>, number: usize) -> Self {
        Self {
            code,
            message: message.into(),
            location: Some(WarningLocation::Slide { number }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    PdfPageNoTextLayer,
    PdfPageSuspiciousTextLayer,
    PdfMultiColumnReadingOrder,
    MergedTableStructureNotPreserved,
    EmbeddedVisualsOmitted,
    VectorGraphicsOmitted,
    PdfRepeatedRegionDeduplicated,
}

impl WarningCode {
    pub const ALL: [WarningCode; 7] = [
        WarningCode::PdfPageNoTextLayer,
        WarningCode::PdfPageSuspiciousTextLayer,
        WarningCode::PdfMultiColumnReadingOrder,
        WarningCode::MergedTableStructureNotPreserved,
        WarningCode::EmbeddedVisualsOmitted,
        WarningCode::VectorGraphicsOmitted,
        WarningCode::PdfRepeatedRegionDeduplicated,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PdfPageNoTextLayer => "pdf_page_no_text_layer",
            Self::PdfPageSuspiciousTextLayer => "pdf_page_suspicious_text_layer",
            Self::PdfMultiColumnReadingOrder => "pdf_multi_column_reading_order",
            Self::MergedTableStructureNotPreserved => "merged_table_structure_not_preserved",
            Self::EmbeddedVisualsOmitted => "embedded_visuals_omitted",
            Self::VectorGraphicsOmitted => "vector_graphics_omitted",
            Self::PdfRepeatedRegionDeduplicated => "pdf_repeated_region_deduplicated",
        }
    }
}

impl fmt::Display for WarningCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WarningLocation {
    Page { number: usize },
    Slide { number: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseStats {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub format: Format,
    /// Total page count for page-oriented formats (currently PDF), regardless of
    /// any page-range slice. Lets a caller learn the whole document size from a
    /// cheap one-page read, then decide whether to request a wider range.
    /// `None` for formats without a page model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_count: Option<usize>,
}

impl ParseStats {
    pub(crate) fn new(
        input_bytes: usize,
        output_bytes: usize,
        format: Format,
        page_count: Option<usize>,
    ) -> Self {
        Self {
            input_bytes,
            output_bytes,
            format,
            page_count,
        }
    }
}

/// Maps spans of the produced output back to where they came from in the
/// source, so a caller can ground an LLM's quote in an exact location instead
/// of trusting the model's self-citation. Opt-in via `ParseRequest.provenance`;
/// absent (and not serialized) when provenance was not requested.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Output→source mappings, ordered by `output.start` and non-overlapping.
    pub spans: Vec<ProvenanceSpan>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceSpan {
    /// Where this run sits in the returned Markdown.
    pub output: TextRange,
    /// Where it came from in the source document.
    pub source: SourceAnchor,
}

/// A half-open byte range `[start, end)` into the returned Markdown string.
/// Byte offsets (not chars) keep the contract unambiguous across hosts and
/// aligned with `stats.output_bytes`; bindings document how to slice per
/// language (UTF-8 strings in Rust, `bytes` in Python, `Buffer` in Node/WASM).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

/// An axis-aligned rectangle in PDF user space (same coordinate system as the
/// page's `/MediaBox`): y grows upward, `y0` is the bottom edge, `y1` the top.
/// Values are PDF points. Consumers rendering with PDF.js can convert with
/// `viewport.convertToViewportRectangle([x0, y0, x1, y1])` directly. The box
/// approximates the glyph extent from baseline geometry (ascent ≈ font size,
/// descent ≈ 0.25 × font size); it is a highlight target, not a typographic
/// measurement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Where a span of output came from in the source. A tagged enum (like
/// [`WarningLocation`]) so more anchor kinds (input byte ranges, table cells)
/// can be added without breaking consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceAnchor {
    /// Page-oriented formats (currently PDF): the 1-based source page number,
    /// plus — at block-level provenance, for born-digital lines whose
    /// geometry is trustworthy — the approximate bounding box on that page.
    /// Absent `bbox` means "somewhere on this page" (page-level spans, and
    /// block-level gaps whose text was rewritten or has no usable geometry).
    Page {
        number: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bbox: Option<Rect>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParseResult {
    pub content: ParseContent,
    pub warnings: Vec<SpoorWarning>,
    pub stats: ParseStats,
    /// Output→source mapping when requested via `ParseRequest.provenance`;
    /// omitted entirely otherwise, so existing callers see no change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

#[cfg(test)]
mod tests {
    use super::WarningCode;

    #[test]
    fn warning_code_display_matches_wire_format() {
        for code in WarningCode::ALL {
            assert_eq!(
                serde_json::to_string(&code).unwrap(),
                format!("\"{}\"", code.as_str())
            );
            assert_eq!(code.to_string(), code.as_str());
        }
    }
}
