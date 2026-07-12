use crate::detect::{self, Format};
use crate::error::{ParseStage, SpoorError};
use crate::limits::{self, DEFAULT_MAX_PARSE_BYTES, ensure_parse_size};
use crate::parse as parsers;
use crate::result::{
    DocumentResult, ParseContent, ParseResult, ParseStats, Provenance, SpoorWarning, TableResult,
};
use crate::source::Source;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub type SpoorResult<T> = std::result::Result<T, SpoorError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseLimits {
    pub max_parse_bytes: usize,
    /// Cooperative cap on in-parser work units (e.g. PDF content-stream
    /// operations). `None` disables it. Bounds CPU on pathological inputs that
    /// the byte budget can't catch; true cancellation still needs host isolation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_work_units: Option<usize>,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_parse_bytes: DEFAULT_MAX_PARSE_BYTES,
            max_work_units: None,
        }
    }
}

/// How much output→source provenance to compute and return.
///
/// Off by default so existing callers pay nothing: emitting provenance for
/// every span across a host boundary (WASM/PyO3/napi) would serialize a large
/// object graph and erase the engine's speed advantage, so callers ask for it
/// — and how much — explicitly, the same principle as the page/table filters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProvenanceLevel {
    /// No provenance (default; output is byte-identical to before).
    #[default]
    Off,
    /// One mapping per source page (currently PDF). Coarse and small.
    Page,
}

impl FromStr for ProvenanceLevel {
    type Err = SpoorError;

    /// Parse a host-supplied level string (CLI flag / binding option) so every
    /// host rejects the same inputs with one structured error.
    fn from_str(value: &str) -> SpoorResult<Self> {
        match value {
            "off" => Ok(Self::Off),
            "page" => Ok(Self::Page),
            other => Err(SpoorError::parse_failed(
                format!("provenance 级别无效：{other}，支持 off 和 page。"),
                ParseStage::Parse,
            )),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableFilter {
    pub sheet: Option<String>,
    pub row_range: Option<(usize, usize)>,
    pub columns: Vec<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// Validate a 1-based inclusive range shared by table row ranges and document
/// page ranges. Centralizing the bounds contract keeps `--rows`, `--pages` and
/// every binding rejecting the same inputs with one structured error.
fn validated_inclusive_range(range: Option<(usize, usize)>) -> SpoorResult<Option<(usize, usize)>> {
    if let Some((first, last)) = range {
        if first == 0 || last == 0 {
            return Err(SpoorError::parse_failed(
                "区间端点必须 ≥ 1。",
                ParseStage::Parse,
            ));
        }
        if first > last {
            return Err(SpoorError::parse_failed(
                format!("区间起点 {first} 不能大于终点 {last}。"),
                ParseStage::Parse,
            ));
        }
    }
    Ok(range)
}

/// Convert a host-supplied slice (e.g. a JS array marshaled to `Vec<u32>`) into
/// an inclusive range pair, validating it has exactly two elements. Lets the
/// Node and WASM bindings forward `rows`/`pages` without each re-implementing
/// the length check, so the "must be a pair" failure is one structured error.
fn range_pair_from_slice(slice: Option<&[u32]>) -> SpoorResult<Option<(usize, usize)>> {
    match slice {
        None => Ok(None),
        Some([first, last]) => Ok(Some((*first as usize, *last as usize))),
        Some(_) => Err(SpoorError::parse_failed(
            "区间需要恰好两个值。",
            ParseStage::Parse,
        )),
    }
}

impl TableFilter {
    /// Assemble a validated table filter from host-agnostic narrowing inputs.
    ///
    /// Every adapter — CLI, Python, Node, WASM — funnels user-supplied table
    /// narrowing through this one place, so pagination, column selection and the
    /// row-range contract stay identical across hosts. `rows` is an inclusive,
    /// 1-based row range (Excel rows for XLSX, line numbers for CSV) and is
    /// mutually exclusive with `limit`/`offset`, mirroring the CLI's `--rows`.
    pub fn build(
        sheet: Option<String>,
        rows: Option<(usize, usize)>,
        columns: Vec<String>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> SpoorResult<Self> {
        let row_range = validated_inclusive_range(rows)?;
        if row_range.is_some() && (limit.is_some() || offset.is_some()) {
            return Err(SpoorError::parse_failed(
                "rows 与 limit/offset 互斥，只能二选一。",
                ParseStage::Parse,
            ));
        }
        Ok(Self {
            sheet,
            row_range,
            columns,
            limit,
            offset,
        })
    }

    /// Like [`TableFilter::build`], but takes the row range as a host-supplied
    /// slice (e.g. a JS array marshaled to `Vec<u32>`). Lets the Node and WASM
    /// bindings forward their `rows` without re-implementing the length check.
    pub fn build_from_row_slice(
        sheet: Option<String>,
        rows: Option<&[u32]>,
        columns: Vec<String>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> SpoorResult<Self> {
        Self::build(sheet, range_pair_from_slice(rows)?, columns, limit, offset)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentFilter {
    /// Inclusive 1-based page range for page-oriented document formats.
    /// Currently only PDF uses this to avoid extracting unrequested pages.
    pub page_range: Option<(usize, usize)>,
    /// Keep cross-page repeated headers/footers instead of deduplicating
    /// them (PDF). Off by default: repeated page furniture pollutes retrieval
    /// and chunking; consumers that need the verbatim page text (for example
    /// to ground quotes against unmodified output) opt out here.
    pub keep_repeated_regions: bool,
}

impl DocumentFilter {
    /// Assemble a validated document filter. `pages` is an inclusive, 1-based
    /// page range and shares the row-range bounds contract via the same
    /// validator, so the CLI's `--pages` and every binding reject the same
    /// inputs with the same structured error.
    pub fn build(pages: Option<(usize, usize)>) -> SpoorResult<Self> {
        Ok(Self {
            page_range: validated_inclusive_range(pages)?,
            keep_repeated_regions: false,
        })
    }

    /// Like [`DocumentFilter::build`], but takes the page range as a
    /// host-supplied slice (e.g. a JS array), validating it is a `[first, last]`
    /// pair. Lets the Node and WASM bindings forward `pages` uniformly.
    pub fn build_from_page_slice(pages: Option<&[u32]>) -> SpoorResult<Self> {
        Self::build(range_pair_from_slice(pages)?)
    }
}

#[derive(Debug, Clone)]
pub struct ParseRequest<'a> {
    pub bytes: &'a [u8],
    pub source_name: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub format_hint: Option<Format>,
    pub table_filter: TableFilter,
    pub document_filter: DocumentFilter,
    pub limits: ParseLimits,
    /// How much output→source provenance to return (default `Off`).
    pub provenance: ProvenanceLevel,
}

impl<'a> ParseRequest<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            source_name: None,
            content_type: None,
            format_hint: None,
            table_filter: TableFilter::default(),
            document_filter: DocumentFilter::default(),
            limits: ParseLimits::default(),
            provenance: ProvenanceLevel::default(),
        }
    }
}

pub fn detect_format(request: &ParseRequest<'_>) -> SpoorResult<Format> {
    catch_boundary(ParseStage::Detect, || detect_format_inner(request))
}

fn detect_format_inner(request: &ParseRequest<'_>) -> SpoorResult<Format> {
    ensure_parse_size(
        request.bytes.len(),
        request.limits.max_parse_bytes,
        "input bytes",
    )
    .map_err(|error| SpoorError::from_anyhow(error, ParseStage::Limits))?;
    if let Some(format) = request.format_hint {
        return Ok(format);
    }
    detect::detect(&source(request))
        .map_err(|error| SpoorError::from_anyhow(error, ParseStage::Detect))
}

pub fn parse(request: &ParseRequest<'_>) -> SpoorResult<ParseResult> {
    let _budget = limits::install_work_budget(request.limits.max_work_units);
    catch_boundary(ParseStage::Parse, || parse_inner(request))
}

fn parse_inner(request: &ParseRequest<'_>) -> SpoorResult<ParseResult> {
    let format = detect_format(request)?;
    if format.is_table() {
        let tables = parse_tables_with_format(request, format)?;
        let output_bytes = tables.serialized_bytes;
        Ok(ParseResult {
            content: ParseContent::Tables(tables),
            warnings: Vec::new(),
            stats: ParseStats::new(request.bytes.len(), output_bytes, format, None),
            provenance: None,
        })
    } else {
        let parsed = parse_document_with_format(request, format)?;
        let output_bytes = parsed.document.markdown.len();
        Ok(ParseResult {
            content: ParseContent::Document(parsed.document),
            warnings: parsed.warnings,
            stats: ParseStats::new(request.bytes.len(), output_bytes, format, parsed.page_count),
            provenance: parsed.provenance,
        })
    }
}

/// Parse a document and return its structured warnings.
///
/// Agents should prefer this function or [`parse`] over [`parse_document`],
/// because `parse_document` intentionally returns only the rendered document.
pub fn parse_document_result(request: &ParseRequest<'_>) -> SpoorResult<ParseResult> {
    let _budget = limits::install_work_budget(request.limits.max_work_units);
    catch_boundary(ParseStage::Parse, || {
        let format = detect_format(request)?;
        let parsed = parse_document_with_format(request, format)?;
        let output_bytes = parsed.document.markdown.len();
        Ok(ParseResult {
            content: ParseContent::Document(parsed.document),
            warnings: parsed.warnings,
            stats: ParseStats::new(request.bytes.len(), output_bytes, format, parsed.page_count),
            provenance: parsed.provenance,
        })
    })
}

/// Parse a document and return only its rendered Markdown.
///
/// This compatibility helper discards structured warnings. Agents should use
/// [`parse`] or [`parse_document_result`] instead.
pub fn parse_document(request: &ParseRequest<'_>) -> SpoorResult<DocumentResult> {
    parse_document_result(request).and_then(|result| match result.content {
        ParseContent::Document(document) => Ok(document),
        ParseContent::Tables(_) => Err(SpoorError::parse_failed(
            "文档格式不支持返回表格结果。",
            ParseStage::Parse,
        )),
    })
}

pub fn parse_tables(request: &ParseRequest<'_>) -> SpoorResult<TableResult> {
    let _budget = limits::install_work_budget(request.limits.max_work_units);
    catch_boundary(ParseStage::Parse, || {
        let format = detect_format(request)?;
        parse_tables_with_format(request, format)
    })
}

/// Extract one safe embedded media resource referenced by a URI emitted by spoor.
///
/// This is intentionally narrower than a general archive extraction API.
/// Supported resource schemes are dispatched by document format and apply the
/// same archive and parse-budget checks used during parsing. Currently emitted
/// shapes are `spoor://pdf/obj/{id}/{gen}` and
/// `spoor://{docx|pptx}/part/{opc-root}/media/{file}`.
pub fn extract_media(request: &ParseRequest<'_>, resource: &str) -> SpoorResult<Vec<u8>> {
    let _budget = limits::install_work_budget(request.limits.max_work_units);
    catch_boundary(ParseStage::Parse, || {
        let format = detect_format(request)?;
        match format {
            Format::Docx => extract_media_from_opc(request, Format::Docx, resource),
            Format::Pptx => extract_media_from_opc(request, Format::Pptx, resource),
            Format::Pdf => extract_media_from_pdf(request, resource),
            _ => Err(SpoorError::parse_failed(
                format!("--extract 仅支持 PDF、DOCX、PPTX 内嵌媒体，当前格式为 {format}。"),
                ParseStage::Parse,
            )),
        }
    })
}

/// Resolve a `spoor://pdf/obj/{id}/{gen}` image handle to its raw JPEG/JPEG2000
/// bytes. Unlike OPC media (already standalone files), only images whose PDF
/// stream is itself a usable file are returned; other encodings degrade to a
/// structured error and stay marked-only in the rendered text.
#[cfg(feature = "pdf")]
fn extract_media_from_pdf(request: &ParseRequest<'_>, resource: &str) -> SpoorResult<Vec<u8>> {
    // A whole-page render handle for a vector-drawn figure: return the page
    // (positioned text + vector shapes) as a self-contained SVG.
    if let Some(page) = safe_pdf_page_resource(resource) {
        let svg =
            crate::parse::pdf_engine::render_page_svg(request.bytes, page).map_err(|error| {
                SpoorError::parse_failed(
                    format!("渲染 PDF 第 {page} 页为 SVG 失败：{error}"),
                    ParseStage::Parse,
                )
            })?;
        ensure_parse_size(
            svg.len(),
            request.limits.max_parse_bytes,
            "rendered pdf page svg",
        )
        .map_err(|error| SpoorError::from_anyhow(error, ParseStage::Limits))?;
        return Ok(svg);
    }

    let (id, generation) = safe_pdf_image_resource(resource).ok_or_else(|| {
        SpoorError::parse_failed(
            "--extract 的 URI 格式无效，PDF 应为 spoor://pdf/obj/{id}/{gen} 或 spoor://pdf/page/{n}。",
            ParseStage::Parse,
        )
    })?;
    let bytes = crate::parse::pdf_media::extract_image(
        request.bytes,
        id,
        generation,
        request.limits.max_parse_bytes,
    )
    .map_err(|error| SpoorError::parse_failed(error.message(), ParseStage::Parse))?;
    ensure_parse_size(
        bytes.len(),
        request.limits.max_parse_bytes,
        "extracted pdf image",
    )
    .map_err(|error| SpoorError::from_anyhow(error, ParseStage::Limits))?;
    Ok(bytes)
}

#[cfg(not(feature = "pdf"))]
fn extract_media_from_pdf(_request: &ParseRequest<'_>, _resource: &str) -> SpoorResult<Vec<u8>> {
    Err(SpoorError::parse_failed(
        "PDF 支持未编译启用。",
        ParseStage::Parse,
    ))
}

/// Parse and validate a `spoor://pdf/obj/{id}/{gen}` handle into `(id, gen)`.
/// Mirrors `safe_opc_media_resource`: never trust the URI shape blindly.
#[cfg(feature = "pdf")]
fn safe_pdf_image_resource(resource: &str) -> Option<(u32, u16)> {
    let mut parts = resource.strip_prefix("spoor://pdf/obj/")?.split('/');
    let id = parts.next()?.parse::<u32>().ok()?;
    let generation = parts.next()?.parse::<u16>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((id, generation))
}

/// Parse and validate a `spoor://pdf/page/{n}` whole-page render handle into a
/// 1-based page number. Rejects 0 and any trailing segment.
#[cfg(feature = "pdf")]
fn safe_pdf_page_resource(resource: &str) -> Option<usize> {
    let page = resource
        .strip_prefix("spoor://pdf/page/")?
        .parse::<usize>()
        .ok()?;
    (page > 0).then_some(page)
}

/// Resolve a `spoor://{fmt}/part/{opc-root}/media/{file}` handle for an OPC
/// container (DOCX/PPTX/XLSX) into raw bytes. The validator below ensures the
/// `opc-root` segment matches the detected format, preventing cross-container
/// forgery (e.g. a `spoor://docx/part/ppt/media/...` handle is rejected).
fn extract_media_from_opc(
    request: &ParseRequest<'_>,
    fmt: Format,
    resource: &str,
) -> SpoorResult<Vec<u8>> {
    let path = safe_opc_media_resource(fmt, resource).ok_or_else(|| {
        SpoorError::parse_failed(
            format!(
                "--extract 的 URI 格式无效，{fmt} 应为 spoor://{fmt}/part/{root}/media/*。",
                root = opc_root_for(fmt).unwrap_or("?"),
            ),
            ParseStage::Parse,
        )
    })?;
    let archive_label = match fmt {
        Format::Docx => "docx",
        Format::Pptx => "pptx",
        Format::Xlsx => "xlsx",
        _ => "opc",
    };
    let mut zip =
        limits::open_zip_archive(request.bytes, archive_label, request.limits.max_parse_bytes)
            .map_err(|error| SpoorError::from_anyhow(error, ParseStage::Parse))?;
    limits::read_zip_bytes(&mut zip, path, request.limits.max_parse_bytes)
        .map_err(|error| SpoorError::from_anyhow(error, ParseStage::Parse))
}

/// The OPC root directory for an OOXML container format, or `None` when the
/// format does not use OPC part addressing.
fn opc_root_for(fmt: Format) -> Option<&'static str> {
    match fmt {
        Format::Docx => Some("word"),
        Format::Pptx => Some("ppt"),
        Format::Xlsx => Some("xl"),
        _ => None,
    }
}

/// Parse and validate a `spoor://{fmt}/part/{opc-root}/media/{file}` URI into
/// the underlying ZIP entry path. Rejects cross-container forgery: the
/// `opc-root` segment must equal `opc_root_for(fmt)`.
fn safe_opc_media_resource(fmt: Format, resource: &str) -> Option<&str> {
    let expected_root = opc_root_for(fmt)?;
    let prefix = match fmt {
        Format::Docx => "spoor://docx/part/",
        Format::Pptx => "spoor://pptx/part/",
        Format::Xlsx => "spoor://xlsx/part/",
        _ => return None,
    };
    let path = resource.strip_prefix(prefix)?;
    safe_opc_media_subpath(expected_root, path).then_some(path)
}

/// Whether `path` is a safe OPC media subpath of the form
/// `{root}/media/<safe component>+` — no traversal, no empty or odd-character
/// segments. Shared by the extract-time URI validator above and the parse-time
/// placeholder emitters (DOCX/PPTX), so a handle is emitted only when it will
/// also pass validation on `--extract`.
pub(crate) fn safe_opc_media_subpath(root: &str, path: &str) -> bool {
    let mut components = path.split('/');
    if components.next() != Some(root) || components.next() != Some("media") {
        return false;
    }
    let remaining = components.collect::<Vec<_>>();
    !remaining.is_empty() && remaining.iter().all(|c| is_safe_media_component(c))
}

pub(crate) fn is_safe_media_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// A parsed document plus the side channels `parse`/`parse_document_result`
/// hand back to the caller: completeness warnings, total page count, and the
/// optional output→source provenance mapping.
struct DocumentParse {
    document: DocumentResult,
    warnings: Vec<SpoorWarning>,
    page_count: Option<usize>,
    provenance: Option<Provenance>,
}

fn parse_document_with_format(
    request: &ParseRequest<'_>,
    format: Format,
) -> SpoorResult<DocumentParse> {
    let extracted = parsers::extract(
        &source(request),
        format,
        &request.document_filter,
        request.limits.max_parse_bytes,
    )
    .map_err(|error| SpoorError::from_anyhow(error, ParseStage::Parse))?;
    ensure_parse_size(
        extracted.markdown.len(),
        request.limits.max_parse_bytes,
        "extracted document text",
    )
    .map_err(|error| SpoorError::from_anyhow(error, ParseStage::Limits))?;

    // Parsers always compute the (cheap) page-level mapping; here we keep it only
    // when the caller asked, so `Off` stays byte-for-byte identical to before.
    let provenance = match request.provenance {
        ProvenanceLevel::Off => None,
        ProvenanceLevel::Page => (!extracted.provenance.is_empty()).then_some(Provenance {
            spans: extracted.provenance,
        }),
    };

    Ok(DocumentParse {
        document: DocumentResult {
            source: source_label(request).to_string(),
            format,
            markdown: extracted.markdown,
        },
        warnings: extracted.warnings,
        page_count: extracted.page_count,
        provenance,
    })
}

fn parse_tables_with_format(
    request: &ParseRequest<'_>,
    format: Format,
) -> SpoorResult<TableResult> {
    let entries = parsers::extract_table_entries(
        &source(request),
        format,
        source_label(request),
        &request.table_filter,
        request.limits.max_parse_bytes,
    )
    .map_err(|error| SpoorError::from_anyhow(error, ParseStage::Parse))?;
    let serialized_bytes = serialized_size(&entries)
        .map_err(|error| SpoorError::from_anyhow(error, ParseStage::Render))?;
    ensure_parse_size(
        serialized_bytes,
        request.limits.max_parse_bytes,
        "extracted table data",
    )
    .map_err(|error| SpoorError::from_anyhow(error, ParseStage::Limits))?;

    Ok(TableResult {
        tables: entries,
        serialized_bytes,
    })
}

fn source<'a>(request: &'a ParseRequest<'a>) -> Source<'a> {
    Source::new(request.bytes, request.source_name, request.content_type)
}

fn source_label<'a>(request: &'a ParseRequest<'a>) -> &'a str {
    request.source_name.unwrap_or("<bytes>")
}

fn serialized_size(value: &impl serde::Serialize) -> anyhow::Result<usize> {
    struct Counter(usize);

    impl std::io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len());
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

fn catch_boundary<T>(
    stage: ParseStage,
    operation: impl FnOnce() -> SpoorResult<T>,
) -> SpoorResult<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)).unwrap_or_else(|payload| {
        Err(SpoorError::parse_failed(
            format!("解析器内部异常：{}", panic_reason(payload.as_ref())),
            stage,
        ))
    })
}

fn panic_reason(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("未知 panic")
}

pub type ExtractedDocument = DocumentResult;
pub type ExtractedTables = TableResult;

#[cfg(test)]
mod tests {
    use super::{Format, ParseStage, TableFilter, catch_boundary, safe_opc_media_resource};
    use crate::ErrorCode;

    #[test]
    fn table_filter_build_accepts_valid_narrowing() {
        let filter = TableFilter::build(
            Some("L1".to_string()),
            Some((5, 104)),
            vec!["分类".to_string()],
            None,
            None,
        )
        .expect("valid filter");
        assert_eq!(filter.sheet.as_deref(), Some("L1"));
        assert_eq!(filter.row_range, Some((5, 104)));
        assert_eq!(filter.columns, vec!["分类".to_string()]);

        let paged = TableFilter::build(None, None, Vec::new(), Some(10), Some(2)).expect("paged");
        assert_eq!(paged.limit, Some(10));
        assert_eq!(paged.offset, Some(2));
    }

    #[test]
    fn table_filter_build_rejects_invalid_rows_and_conflicts() {
        // Mirrors the CLI's `--rows`/`parse_row_range` contract so every host
        // rejects the same inputs.
        for rows in [(0, 5), (5, 3)] {
            let error = TableFilter::build(None, Some(rows), Vec::new(), None, None)
                .expect_err("invalid row range");
            assert_eq!(error.code, ErrorCode::ParseFailed);
        }

        let error = TableFilter::build(None, Some((2, 4)), Vec::new(), Some(1), None)
            .expect_err("rows excludes limit");
        assert_eq!(error.code, ErrorCode::ParseFailed);

        let error = TableFilter::build(None, Some((2, 4)), Vec::new(), None, Some(1))
            .expect_err("rows excludes offset");
        assert_eq!(error.code, ErrorCode::ParseFailed);
    }

    #[test]
    fn table_filter_build_from_row_slice_requires_pair() {
        let filter =
            TableFilter::build_from_row_slice(None, Some(&[5, 104]), Vec::new(), None, None)
                .expect("valid pair");
        assert_eq!(filter.row_range, Some((5, 104)));

        assert_eq!(
            TableFilter::build_from_row_slice(None, None, Vec::new(), None, None)
                .expect("no range")
                .row_range,
            None
        );

        for bad in [vec![], vec![1u32], vec![1, 2, 3]] {
            let error = TableFilter::build_from_row_slice(None, Some(&bad), Vec::new(), None, None)
                .expect_err("non-pair slice");
            assert_eq!(error.code, ErrorCode::ParseFailed);
        }
    }

    #[test]
    fn document_filter_build_validates_page_range_like_rows() {
        use super::DocumentFilter;

        assert_eq!(
            DocumentFilter::build(Some((2, 5))).unwrap().page_range,
            Some((2, 5))
        );
        assert_eq!(DocumentFilter::build(None).unwrap().page_range, None);

        // Same 1-based inclusive bounds contract as table row ranges.
        for bad in [(0, 5), (5, 3)] {
            assert_eq!(
                DocumentFilter::build(Some(bad)).unwrap_err().code,
                ErrorCode::ParseFailed
            );
        }

        // Slice variant enforces the [first, last] pair shape.
        assert_eq!(
            DocumentFilter::build_from_page_slice(Some(&[2, 5]))
                .unwrap()
                .page_range,
            Some((2, 5))
        );
        for bad in [vec![], vec![1u32], vec![1, 2, 3]] {
            assert_eq!(
                DocumentFilter::build_from_page_slice(Some(&bad))
                    .unwrap_err()
                    .code,
                ErrorCode::ParseFailed
            );
        }
    }

    #[test]
    fn public_boundary_normalizes_parser_panics() {
        let error = catch_boundary::<()>(ParseStage::Parse, || {
            panic!("malformed parser input");
        })
        .expect_err("panic must become a structured error");

        assert_eq!(error.code, ErrorCode::ParseFailed);
        assert_eq!(error.stage, Some(ParseStage::Parse));
        assert!(error.reason.contains("malformed parser input"));
    }

    #[test]
    fn opc_media_resources_require_unified_spoor_uri() {
        // DOCX: prefix + root must both match.
        assert_eq!(
            safe_opc_media_resource(Format::Docx, "spoor://docx/part/word/media/image1.png"),
            Some("word/media/image1.png")
        );
        // Missing scheme entirely.
        assert_eq!(
            safe_opc_media_resource(Format::Docx, "word/media/image1.png"),
            None
        );
        // Path traversal inside the media filename is rejected by the sandbox.
        assert_eq!(
            safe_opc_media_resource(Format::Docx, "spoor://docx/part/word/media/../evil.png"),
            None
        );
        // Old per-format scheme is no longer accepted.
        assert_eq!(
            safe_opc_media_resource(Format::Docx, "spoor-docx://word/media/image1.png"),
            None
        );
        // Cross-container forgery: a `docx` URI must not address a `ppt/` root.
        assert_eq!(
            safe_opc_media_resource(Format::Docx, "spoor://docx/part/ppt/media/image1.png"),
            None
        );
        // A `pptx` validator must not accept a `word/` root.
        assert_eq!(
            safe_opc_media_resource(Format::Pptx, "spoor://pptx/part/word/media/image1.png"),
            None
        );
        // PPTX happy path.
        assert_eq!(
            safe_opc_media_resource(Format::Pptx, "spoor://pptx/part/ppt/media/image3.png"),
            Some("ppt/media/image3.png")
        );
        // Format mismatch: a DOCX-shaped URI is rejected by the PPTX validator.
        assert_eq!(
            safe_opc_media_resource(Format::Pptx, "spoor://docx/part/word/media/image1.png"),
            None
        );
        // XLSX validator is wired (no emitter yet) but accepts only `xl/`.
        assert_eq!(
            safe_opc_media_resource(Format::Xlsx, "spoor://xlsx/part/xl/media/image2.png"),
            Some("xl/media/image2.png")
        );
        assert_eq!(
            safe_opc_media_resource(Format::Xlsx, "spoor://xlsx/part/word/media/image2.png"),
            None
        );
        // Non-OPC format never validates.
        assert_eq!(
            safe_opc_media_resource(Format::Pdf, "spoor://pdf/part/word/media/image1.png"),
            None
        );
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_image_resources_require_obj_id_gen_uri() {
        use super::safe_pdf_image_resource;
        assert_eq!(
            safe_pdf_image_resource("spoor://pdf/obj/12/0"),
            Some((12, 0))
        );
        // Old per-format scheme is rejected.
        assert_eq!(safe_pdf_image_resource("spoor-pdf://obj/12/0"), None);
        // Wrong format segment in the unified scheme.
        assert_eq!(safe_pdf_image_resource("spoor://docx/obj/12/0"), None);
        // Missing or mis-shaped path segments.
        assert_eq!(safe_pdf_image_resource("spoor://pdf/12/0"), None);
        assert_eq!(safe_pdf_image_resource("spoor://pdf/obj/12"), None);
        assert_eq!(safe_pdf_image_resource("spoor://pdf/obj/12/0/extra"), None);
        assert_eq!(safe_pdf_image_resource("spoor://pdf/obj/x/0"), None);
    }
}
