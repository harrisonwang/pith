//! Deterministic, bytes-only document parsing engine.
//!
//! `spoor-core` performs no file, network, stdin/stdout, or process I/O.
//! Adapters supply bytes and metadata through [`ParseRequest`].

mod detect;
mod engine;
mod error;
mod json_schema;
mod limits;
mod locate;
mod output;
mod parse;
mod render;
mod result;
mod source;

pub use detect::Format;
pub use engine::{
    DocumentFilter, ExtractedDocument, ExtractedTables, ParseLimits, ParseRequest, ProvenanceLevel,
    SpoorResult, TableFilter, detect_format, extract_media, parse, parse_document,
    parse_document_result, parse_tables,
};
pub use error::{ErrorCode, ParseStage, SpoorError, StructuredError};
pub use json_schema::{
    HeaderInfo, JsonOutput, PreambleInfo, RowRange, TABLE_SCHEMA_VERSION, TABLE_USAGE, TableEntry,
    a1_range, cells_to_values,
};
pub use limits::{DEFAULT_MAX_PARSE_BYTES, MIN_MAX_PARSE_BYTES};
pub use locate::{LocateMethod, LocatedQuote, Locator, locate_quote, locate_quote_grounded};
pub use render::{
    DEFAULT_MAX_OUTPUT_BYTES, LimitedOutput, MIN_MAX_OUTPUT_BYTES, OutputMode, default_mode_for,
    limit_markdown_output, render_documents, render_json, render_json_limited,
};
pub use result::{
    DocumentResult, ParseContent, ParseResult, ParseStats, Provenance, ProvenanceSpan,
    SourceAnchor, SpoorWarning, TableResult, TextRange, WarningCode, WarningLocation,
};
