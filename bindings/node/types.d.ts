export interface ParseOptions {
  sourceName?: string;
  contentType?: string;
  format?: string;
  maxParseBytes?: number;
  /** XLSX only: restrict output to one sheet by name. */
  sheet?: string;
  /**
   * Inclusive 1-based `[first, last]` row range (Excel rows for XLSX, line
   * numbers for CSV). Mutually exclusive with `limit`/`offset`.
   */
  rows?: [number, number];
  /** Keep only these columns, by header name. */
  columns?: string[];
  /** Max data rows per table (default 100). */
  limit?: number;
  /** Skip this many data rows before applying `limit`. */
  offset?: number;
  /** PDF only: inclusive 1-based `[first, last]` page range to parse. */
  pages?: [number, number];
  /** Cooperative cap on in-parser work units (e.g. PDF operations) to bound CPU. */
  maxWorkUnits?: number;
  /**
   * Return output→source provenance: `"page"` for page-level (PDF), `"off"`
   * (default) for none. Output byte ranges in `provenance` index `markdown` as
   * UTF-8; slice with `Buffer.from(markdown).subarray(start, end)`.
   */
  provenance?: string;
  /**
   * PDF only: keep cross-page repeated headers/footers instead of
   * deduplicating them (default false — repeats are removed and a
   * `pdf_repeated_region_deduplicated` warning names what moved).
   */
  keepRepeatedRegions?: boolean;
}

export interface DocumentResult {
  source: string;
  format: string;
  markdown: string;
}

export interface TableResult {
  tables: Array<Record<string, unknown>>;
  serialized_bytes: number;
}

export type ParseContent =
  | { kind: 'document'; value: DocumentResult }
  | { kind: 'tables'; value: TableResult };

export type WarningLocation =
  | { kind: 'page'; number: number }
  | { kind: 'slide'; number: number };

export type WarningCode =
  | 'pdf_page_no_text_layer'
  | 'pdf_page_suspicious_text_layer'
  | 'pdf_multi_column_reading_order'
  | 'merged_table_structure_not_preserved'
  | 'embedded_visuals_omitted'
  | 'vector_graphics_omitted';

export interface SpoorWarning {
  code: WarningCode;
  message: string;
  location?: WarningLocation;
}

/** Half-open `[start, end)` byte range into the returned `markdown`. */
export interface TextRange {
  start: number;
  end: number;
}

/** Where a span of output came from. Currently page-oriented (PDF). */
export type SourceAnchor = { kind: 'page'; number: number };

export interface ProvenanceSpan {
  output: TextRange;
  source: SourceAnchor;
}

export interface Provenance {
  spans: ProvenanceSpan[];
}

export interface ParseResult {
  content: ParseContent;
  warnings: SpoorWarning[];
  stats: {
    input_bytes: number;
    output_bytes: number;
    format: string;
    /** Total pages for page-oriented formats (PDF); absent otherwise. */
    page_count?: number;
  };
  /** Output→source mapping when requested via `provenance`; absent otherwise. */
  provenance?: Provenance;
}

export type LocateMethod =
  | 'exact'
  | 'whitespace_insensitive'
  | 'table_anchor'
  | 'numeric_equivalence';

/** A grounded quote: where it sits in the markdown and what surrounds it. */
export interface LocatedQuote {
  /**
   * Half-open `[start, end)` range in UTF-16 code units (JS string indices),
   * so `markdown.slice(span.start, span.end)` is the raw hit — unlike
   * provenance ranges, which are UTF-8 byte offsets.
   */
  span: { start: number; end: number };
  /** Up to 30 chars of whitespace-collapsed context before the hit. */
  before: string;
  /** The matched text, whitespace-collapsed. */
  hit: string;
  /** Up to 30 chars of whitespace-collapsed context after the hit. */
  after: string;
  /** 1-based source page from spoor's `## Page N` markers; null without them. */
  page: number | null;
  method: LocateMethod;
}

export interface SpoorError extends Error {
  is_error: true;
  code: string;
  reason: string;
  hint: string;
  recoverable: boolean;
  stage?: string;
}

export function detectFormat(data: Buffer, sourceName?: string | null): string;
export function parseBytes(data: Buffer, options?: ParseOptions | null): ParseResult;
export function extractMedia(data: Buffer, resource: string, options?: ParseOptions | null): Buffer;
export function locateQuote(markdown: string, quote: string): LocatedQuote | null;
