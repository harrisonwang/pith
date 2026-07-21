use spoor_core::{DocumentFilter, Format, ParseLimits, ParseRequest, ProvenanceLevel, TableFilter};
use std::str::FromStr;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn detect_format(
    bytes: &[u8],
    source_name: Option<String>,
    content_type: Option<String>,
) -> Result<String, JsValue> {
    let request = request(
        bytes,
        source_name.as_deref(),
        content_type.as_deref(),
        None,
        None,
        None,
    )?;
    spoor_core::detect_format(&request)
        .map(|format| format.to_string())
        .map_err(error_value)
}

/// Parse document/table bytes into a typed `ParseResult`.
///
/// For table formats (CSV/XLSX) the trailing options mirror the CLI and the
/// other bindings: `sheet` (XLSX only), `rows` as an inclusive 1-based
/// `[first, last]` pair (mutually exclusive with `limit`/`offset`), `columns`
/// to keep, and `limit`/`offset` for pagination. For page-oriented formats
/// (PDF pages, PPTX slides), `pages` is an inclusive 1-based `[first, last]`
/// range. `provenance` accepts `"page"`, `"block"`, or `"off"` (default).
/// `keep_repeated_regions` keeps PDF cross-page repeated headers/footers
/// instead of deduplicating them. Each is ignored by formats it does not
/// apply to, and all are optional, so existing 5-argument calls keep working.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn parse_bytes(
    bytes: &[u8],
    source_name: Option<String>,
    content_type: Option<String>,
    format: Option<String>,
    max_parse_bytes: Option<usize>,
    sheet: Option<String>,
    rows: Option<Vec<u32>>,
    columns: Option<Vec<String>>,
    limit: Option<usize>,
    offset: Option<usize>,
    pages: Option<Vec<u32>>,
    max_work_units: Option<usize>,
    provenance: Option<String>,
    keep_repeated_regions: Option<bool>,
) -> Result<JsValue, JsValue> {
    let mut request = request(
        bytes,
        source_name.as_deref(),
        content_type.as_deref(),
        format.as_deref(),
        max_parse_bytes,
        max_work_units,
    )?;
    request.table_filter = TableFilter::build_from_row_slice(
        sheet,
        rows.as_deref(),
        columns.unwrap_or_default(),
        limit,
        offset,
    )
    .map_err(error_value)?;
    request.document_filter =
        DocumentFilter::build_from_page_slice(pages.as_deref()).map_err(error_value)?;
    request.document_filter.keep_repeated_regions = keep_repeated_regions.unwrap_or(false);
    if let Some(level) = provenance.as_deref() {
        request.provenance = ProvenanceLevel::from_str(level).map_err(error_value)?;
    }
    let result = spoor_core::parse(&request).map_err(error_value)?;
    serde_wasm_bindgen::to_value(&result).map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Extract one safe embedded media resource referenced by a URI emitted in the
/// parsed output (`spoor://docx/part/word/media/*`,
/// `spoor://pptx/part/ppt/media/*`, or `spoor://pdf/obj/{id}/{gen}`).
/// Returns the raw resource bytes as a `Uint8Array`. Lets browser and edge
/// callers resolve image placeholders without filesystem access. spoor does not
/// decode or interpret the bytes.
#[wasm_bindgen]
pub fn extract_media(
    bytes: &[u8],
    resource: String,
    source_name: Option<String>,
    content_type: Option<String>,
    format: Option<String>,
    max_parse_bytes: Option<usize>,
) -> Result<Vec<u8>, JsValue> {
    let request = request(
        bytes,
        source_name.as_deref(),
        content_type.as_deref(),
        format.as_deref(),
        max_parse_bytes,
        None,
    )?;
    spoor_core::extract_media(&request, &resource).map_err(error_value)
}

/// Locate LLM-cited text or data in Markdown spoor produced. Exact and
/// whitespace-insensitive matches are textual; table/numeric matches are
/// source candidates. `null` only means no tier matched this Markdown; scans,
/// visuals, or parse omissions may still contain the content, and no result
/// establishes factual truth. `span` uses JS string indices, so
/// `markdown.slice(span.start, span.end)` is the raw hit.
#[wasm_bindgen]
pub fn locate_quote(
    markdown: &str,
    quote: &str,
    provenance_spans: JsValue,
) -> Result<JsValue, JsValue> {
    #[derive(serde::Serialize)]
    struct Span {
        start: usize,
        end: usize,
    }
    #[derive(serde::Serialize)]
    struct Located<'a> {
        before: &'a str,
        hit: &'a str,
        after: &'a str,
        span: Span,
        page: Option<usize>,
        method: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        score: Option<f64>,
        occurrences: usize,
        corroborated: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        anchor: &'a Option<spoor_core::SourceAnchor>,
    }

    // Pass the same `provenance.spans` array a parse returned (byte offsets
    // as-is) to also get the hit's source anchor back.
    let spans: Vec<spoor_core::ProvenanceSpan> =
        if provenance_spans.is_undefined() || provenance_spans.is_null() {
            Vec::new()
        } else {
            serde_wasm_bindgen::from_value(provenance_spans)
                .map_err(|error| JsValue::from_str(&format!("provenance 参数无效:{error}")))?
        };

    let Some(found) = spoor_core::locate_quote_grounded(markdown, quote, &spans) else {
        return Ok(JsValue::NULL);
    };
    // Core spans are UTF-8 byte offsets; convert to UTF-16 code units.
    let start = markdown[..found.span.start].encode_utf16().count();
    let end = start
        + markdown[found.span.start..found.span.end]
            .encode_utf16()
            .count();
    let located = Located {
        before: &found.before,
        hit: &found.hit,
        after: &found.after,
        span: Span { start, end },
        page: found.page,
        method: found.method.as_str(),
        score: found.score,
        occurrences: found.occurrences,
        corroborated: found.corroborated,
        anchor: &found.anchor,
    };
    serde_wasm_bindgen::to_value(&located).map_err(|error| JsValue::from_str(&error.to_string()))
}

fn request<'a>(
    bytes: &'a [u8],
    source_name: Option<&'a str>,
    content_type: Option<&'a str>,
    format: Option<&str>,
    max_parse_bytes: Option<usize>,
    max_work_units: Option<usize>,
) -> Result<ParseRequest<'a>, JsValue> {
    let mut request = ParseRequest::new(bytes);
    request.source_name = source_name;
    request.content_type = content_type;
    request.format_hint = format
        .map(Format::from_str)
        .transpose()
        .map_err(error_value)?;
    request.limits = ParseLimits {
        max_parse_bytes: max_parse_bytes.unwrap_or(request.limits.max_parse_bytes),
        max_work_units,
    };
    Ok(request)
}

fn error_value(error: spoor_core::SpoorError) -> JsValue {
    serde_wasm_bindgen::to_value(&error).unwrap_or_else(|_| JsValue::from_str(&error.to_json()))
}
