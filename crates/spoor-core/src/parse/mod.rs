use crate::detect::Format;
use crate::engine::{DocumentFilter, TableFilter};
use crate::json_schema::TableEntry;
use crate::result::{ProvenanceSpan, SpoorWarning};
use crate::source::Source;
use anyhow::{Result, anyhow};

#[cfg(feature = "tables")]
mod csv;
#[cfg(feature = "office")]
mod docx;
#[cfg(feature = "epub")]
mod epub;
#[cfg(feature = "html")]
mod html;
#[cfg(feature = "notebook")]
mod ipynb;
mod markdown;
#[cfg(feature = "pdf")]
mod pdf;
#[cfg(feature = "pdf")]
mod pdf_dehyphen;
#[cfg(feature = "pdf")]
mod pdf_furniture;
#[cfg(feature = "pdf")]
mod pdf_layout;
#[cfg(feature = "pdf")]
mod pdf_links;
#[cfg(feature = "pdf")]
mod pdf_outline;
#[cfg(feature = "pdf")]
mod pdf_tables;
#[cfg(feature = "pdf")]
#[rustfmt::skip]
pub(crate) mod pdf_engine;
#[cfg(feature = "pdf")]
pub(crate) mod pdf_media;
mod plain;
#[cfg(feature = "office")]
mod pptx;
#[cfg(feature = "html")]
mod url;
#[cfg(feature = "tables")]
mod xlsx;
#[cfg(any(feature = "office", feature = "epub"))]
mod xml;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtractedMarkdown {
    pub markdown: String,
    pub warnings: Vec<SpoorWarning>,
    /// Total document unit count for page-oriented formats (currently PDF pages),
    /// computed independently of any page-range slice so callers learn the whole
    /// size even from a one-page peek. `None` for formats without a page model.
    pub page_count: Option<usize>,
    /// Output→source spans (currently page-level for PDF). Always computed where
    /// cheap; the engine keeps or drops it per `ParseRequest.provenance`. Empty
    /// for formats that do not produce a mapping yet.
    pub provenance: Vec<ProvenanceSpan>,
}

impl ExtractedMarkdown {
    fn without_warnings(markdown: String) -> Self {
        Self {
            markdown,
            warnings: Vec::new(),
            page_count: None,
            provenance: Vec::new(),
        }
    }

    fn with_warnings(markdown: String, warnings: Vec<SpoorWarning>) -> Self {
        Self {
            markdown,
            warnings,
            page_count: None,
            provenance: Vec::new(),
        }
    }
}

pub fn extract(
    source: &Source<'_>,
    format: Format,
    document_filter: &DocumentFilter,
    max_parse_bytes: usize,
) -> Result<ExtractedMarkdown> {
    match format {
        Format::Url => extract_url(source, document_filter, max_parse_bytes),
        Format::Html => extract_html(source, document_filter, max_parse_bytes),
        Format::Markdown => markdown::extract(source).map(ExtractedMarkdown::without_warnings),
        Format::Pdf => extract_pdf(source, document_filter, max_parse_bytes),
        Format::Docx => extract_docx(source, document_filter, max_parse_bytes),
        Format::Xlsx => extract_xlsx(source, document_filter, max_parse_bytes),
        Format::Pptx => extract_pptx(source, document_filter, max_parse_bytes),
        Format::Csv => extract_csv(source, document_filter, max_parse_bytes),
        Format::Ipynb => extract_ipynb(source, document_filter, max_parse_bytes),
        Format::Epub => extract_epub(source, document_filter, max_parse_bytes),
        Format::PlainText => plain::extract(source).map(ExtractedMarkdown::without_warnings),
    }
}

pub fn extract_table_entries(
    source: &Source<'_>,
    format: Format,
    source_label: &str,
    filter: &TableFilter,
    max_parse_bytes: usize,
) -> Result<Vec<TableEntry>> {
    match format {
        Format::Csv => extract_csv_tables(source, source_label, filter, max_parse_bytes),
        Format::Xlsx => extract_xlsx_tables(source, source_label, filter, max_parse_bytes),
        _ => Err(anyhow!(
            "--mode json 仅支持 CSV 和 XLSX，当前格式为 {format}；请改用 --mode md。"
        )),
    }
}

macro_rules! format_extractor {
    ($name:ident, $feature:literal, $module:ident) => {
        #[cfg(feature = $feature)]
        fn $name(
            source: &Source<'_>,
            _document_filter: &DocumentFilter,
            max_parse_bytes: usize,
        ) -> Result<ExtractedMarkdown> {
            $module::extract(source, max_parse_bytes).map(ExtractedMarkdown::without_warnings)
        }

        #[cfg(not(feature = $feature))]
        fn $name(
            _source: &Source<'_>,
            _document_filter: &DocumentFilter,
            _max_parse_bytes: usize,
        ) -> Result<ExtractedMarkdown> {
            Err(anyhow!(concat!(
                "format disabled at compile time; enable feature ",
                $feature
            )))
        }
    };
}

format_extractor!(extract_url, "html", url);
format_extractor!(extract_html, "html", html);
format_extractor!(extract_xlsx, "tables", xlsx);
format_extractor!(extract_csv, "tables", csv);
format_extractor!(extract_ipynb, "notebook", ipynb);
format_extractor!(extract_epub, "epub", epub);

macro_rules! diagnostic_extractor {
    ($name:ident, $feature:literal, $module:ident) => {
        #[cfg(feature = $feature)]
        fn $name(
            source: &Source<'_>,
            _document_filter: &DocumentFilter,
            max_parse_bytes: usize,
        ) -> Result<ExtractedMarkdown> {
            $module::extract(source, max_parse_bytes)
        }

        #[cfg(not(feature = $feature))]
        fn $name(
            _source: &Source<'_>,
            _document_filter: &DocumentFilter,
            _max_parse_bytes: usize,
        ) -> Result<ExtractedMarkdown> {
            Err(anyhow!(concat!(
                "format disabled at compile time; enable feature ",
                $feature
            )))
        }
    };
}

#[cfg(feature = "pdf")]
fn extract_pdf(
    source: &Source<'_>,
    document_filter: &DocumentFilter,
    max_parse_bytes: usize,
) -> Result<ExtractedMarkdown> {
    pdf::extract(source, document_filter, max_parse_bytes)
}

#[cfg(not(feature = "pdf"))]
fn extract_pdf(
    _source: &Source<'_>,
    _document_filter: &DocumentFilter,
    _max_parse_bytes: usize,
) -> Result<ExtractedMarkdown> {
    Err(anyhow!(
        "format disabled at compile time; enable feature pdf"
    ))
}

diagnostic_extractor!(extract_docx, "office", docx);
diagnostic_extractor!(extract_pptx, "office", pptx);

#[cfg(feature = "tables")]
fn extract_csv_tables(
    source: &Source<'_>,
    source_label: &str,
    filter: &TableFilter,
    max_parse_bytes: usize,
) -> Result<Vec<TableEntry>> {
    csv::extract_table_entries(source, source_label, filter, max_parse_bytes)
}

#[cfg(not(feature = "tables"))]
fn extract_csv_tables(
    _source: &Source<'_>,
    _source_label: &str,
    _filter: &TableFilter,
    _max_parse_bytes: usize,
) -> Result<Vec<TableEntry>> {
    Err(anyhow!(
        "CSV support disabled at compile time; enable feature tables"
    ))
}

#[cfg(feature = "tables")]
fn extract_xlsx_tables(
    source: &Source<'_>,
    source_label: &str,
    filter: &TableFilter,
    max_parse_bytes: usize,
) -> Result<Vec<TableEntry>> {
    xlsx::extract_table_entries(source, source_label, filter, max_parse_bytes)
}

#[cfg(not(feature = "tables"))]
fn extract_xlsx_tables(
    _source: &Source<'_>,
    _source_label: &str,
    _filter: &TableFilter,
    _max_parse_bytes: usize,
) -> Result<Vec<TableEntry>> {
    Err(anyhow!(
        "XLSX support disabled at compile time; enable feature tables"
    ))
}
