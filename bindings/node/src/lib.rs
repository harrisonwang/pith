use napi::bindgen_prelude::{Buffer, Error, Result, Status};
use napi_derive::napi;
use spoor_core::{DocumentFilter, Format, ParseLimits, ParseRequest, ProvenanceLevel, TableFilter};
use std::str::FromStr;

#[derive(Default)]
#[napi(object)]
pub struct ParseOptions {
    pub source_name: Option<String>,
    pub content_type: Option<String>,
    pub format: Option<String>,
    pub max_parse_bytes: Option<i64>,
    /// XLSX only: restrict output to one sheet by name.
    pub sheet: Option<String>,
    /// Inclusive 1-based `[first, last]` row range (Excel rows for XLSX, line
    /// numbers for CSV). Mutually exclusive with `limit`/`offset`.
    pub rows: Option<Vec<u32>>,
    /// Keep only these columns, by header name.
    pub columns: Option<Vec<String>>,
    /// Max data rows per table (default 100).
    pub limit: Option<u32>,
    /// Skip this many data rows before applying `limit`.
    pub offset: Option<u32>,
    /// PDF only: inclusive 1-based `[first, last]` page range to parse.
    pub pages: Option<Vec<u32>>,
    /// Cooperative cap on in-parser work units (e.g. PDF operations) to bound
    /// CPU on pathological inputs. Omit to disable.
    pub max_work_units: Option<i64>,
    /// Return output→source provenance: `"page"` for page-level (PDF), `"off"`
    /// (default) for none. Output byte ranges in `provenance` index the returned
    /// `markdown` as UTF-8 bytes; slice with `Buffer.from(markdown).subarray(...)`.
    pub provenance: Option<String>,
    /// PDF only: keep cross-page repeated headers/footers instead of
    /// deduplicating them (default false — repeats are removed and a
    /// `pdf_repeated_region_deduplicated` warning names what moved).
    pub keep_repeated_regions: Option<bool>,
}

#[napi]
pub fn parse_bytes(data: Buffer, options: Option<ParseOptions>) -> Result<serde_json::Value> {
    let options = options.unwrap_or_default();
    let mut request = build_request(&data, &options)?;
    request.table_filter = table_filter(&options)?;
    request.document_filter =
        DocumentFilter::build_from_page_slice(options.pages.as_deref()).map_err(to_node_error)?;
    request.document_filter.keep_repeated_regions = options.keep_repeated_regions.unwrap_or(false);
    if let Some(level) = options.provenance.as_deref() {
        request.provenance = ProvenanceLevel::from_str(level).map_err(to_node_error)?;
    }
    let result = spoor_core::parse(&request).map_err(to_node_error)?;
    serde_json::to_value(result)
        .map_err(|error| Error::new(Status::GenericFailure, error.to_string()))
}

/// Extract one safe embedded media resource referenced by a URI emitted in the
/// parsed output (e.g. `spoor://docx/part/word/media/image1.png`,
/// `spoor://pptx/part/ppt/media/imageN.png`, or `spoor://pdf/obj/{id}/{gen}`).
/// Returns the raw resource bytes. spoor does not decode or interpret the bytes.
#[napi]
pub fn extract_media(
    data: Buffer,
    resource: String,
    options: Option<ParseOptions>,
) -> Result<Buffer> {
    let options = options.unwrap_or_default();
    let request = build_request(&data, &options)?;
    let bytes = spoor_core::extract_media(&request, &resource).map_err(to_node_error)?;
    Ok(Buffer::from(bytes))
}

/// Ground an LLM-cited quote in Markdown spoor produced. Tries four
/// deterministic tiers (exact, whitespace-insensitive, table-cell anchor,
/// numeric/unit equivalence) and returns `null` when none matches — treat the
/// claim the quote backs as unverifiable. `span` indexes `markdown` as a JS
/// string (UTF-16 code units), so `markdown.slice(span.start, span.end)` is
/// the raw hit; `page` comes from spoor's own `## Page N` markers when present.
#[napi]
pub fn locate_quote(
    markdown: String,
    quote: String,
    provenance_spans: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let spans: Vec<spoor_core::ProvenanceSpan> = match provenance_spans {
        Some(value) => serde_json::from_value(value).map_err(|error| {
            Error::new(
                Status::InvalidArg,
                format!("provenanceSpans 参数无效：{error}"),
            )
        })?,
        None => Vec::new(),
    };
    let Some(found) = spoor_core::locate_quote_grounded(&markdown, &quote, &spans) else {
        return Ok(serde_json::Value::Null);
    };
    // Core spans are UTF-8 byte offsets; convert to UTF-16 code units.
    let start = markdown[..found.span.start].encode_utf16().count();
    let end = start
        + markdown[found.span.start..found.span.end]
            .encode_utf16()
            .count();
    let mut value = serde_json::json!({
        "before": found.before,
        "hit": found.hit,
        "after": found.after,
        "span": {"start": start, "end": end},
        "page": found.page,
        "method": found.method.as_str(),
    });
    if let Some(anchor) = &found.anchor {
        value["anchor"] = serde_json::to_value(anchor)
            .map_err(|error| Error::new(Status::GenericFailure, error.to_string()))?;
    }
    Ok(value)
}

#[napi]
pub fn detect_format(data: Buffer, source_name: Option<String>) -> Result<String> {
    let mut request = ParseRequest::new(data.as_ref());
    request.source_name = source_name.as_deref();
    spoor_core::detect_format(&request)
        .map(|format| format.to_string())
        .map_err(to_node_error)
}

fn build_request<'a>(data: &'a Buffer, options: &'a ParseOptions) -> Result<ParseRequest<'a>> {
    let mut request = ParseRequest::new(data.as_ref());
    request.source_name = options.source_name.as_deref();
    request.content_type = options.content_type.as_deref();
    request.format_hint = options
        .format
        .as_deref()
        .map(Format::from_str)
        .transpose()
        .map_err(to_node_error)?;
    let max_parse_bytes = match options.max_parse_bytes {
        Some(value) => usize::try_from(value)
            .map_err(|_| Error::new(Status::InvalidArg, "max_parse_bytes must be positive"))?,
        None => request.limits.max_parse_bytes,
    };
    let max_work_units = match options.max_work_units {
        Some(value) => Some(
            usize::try_from(value)
                .map_err(|_| Error::new(Status::InvalidArg, "max_work_units must be positive"))?,
        ),
        None => None,
    };
    request.limits = ParseLimits {
        max_parse_bytes,
        max_work_units,
    };
    Ok(request)
}

fn table_filter(options: &ParseOptions) -> Result<TableFilter> {
    TableFilter::build_from_row_slice(
        options.sheet.clone(),
        options.rows.as_deref(),
        options.columns.clone().unwrap_or_default(),
        options.limit.map(|n| n as usize),
        options.offset.map(|n| n as usize),
    )
    .map_err(to_node_error)
}

fn to_node_error(error: spoor_core::SpoorError) -> Error {
    Error::new(Status::GenericFailure, error.to_json())
}
