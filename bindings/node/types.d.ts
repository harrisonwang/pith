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
  /** Page-oriented formats (PDF pages, PPTX slides): inclusive 1-based `[first, last]` range to parse. */
  pages?: [number, number];
  /** Cooperative cap on in-parser work units (e.g. PDF operations) to bound CPU. */
  maxWorkUnits?: number;
  /**
   * Return output→source provenance: `"page"` for page/slide-level
   * (PDF/PPTX), `"block"` for the finest available, `"off"` (default) for
   * none. Output byte ranges in `provenance` index `markdown` as UTF-8;
   * slice with `Buffer.from(markdown).subarray(start, end)`.
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
  | 'vector_graphics_omitted'
  | 'pdf_repeated_region_deduplicated'
  | 'slide_no_text_layer'
  | 'hidden_slide_omitted';

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

/** Approximate box in PDF-native user space (y-up, /MediaBox system). */
export interface AnchorRect { x0: number; y0: number; x1: number; y1: number }

/**
 * Where a span of output came from: a PDF page (block level adds an
 * approximate box), a PPTX slide (1-based deck-order number), a linear input
 * byte range (plain text / Markdown), or a table cell of the document-mode
 * CSV/XLSX rendering.
 */
export type SourceAnchor =
  | { kind: 'page'; number: number; bbox?: AnchorRect }
  | { kind: 'slide'; number: number }
  | { kind: 'input'; start: number; end: number }
  | { kind: 'cell'; row: number; column: string; sheet?: string };

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
    /** Total pages (PDF) or slides (PPTX); absent for other formats. */
    page_count?: number;
  };
  /** Output→source mapping when requested via `provenance`; absent otherwise. */
  provenance?: Provenance;
}

export type LocateMethod =
  | 'exact'
  | 'whitespace_insensitive'
  | 'fuzzy'
  | 'table_anchor'
  | 'numeric_equivalence';

/** A located text match or source candidate with its Markdown context. */
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
  /** Similarity of a `fuzzy` hit (1.0 = no edits); absent for other tiers. */
  score?: number;
  /**
   * How many places this tier could have matched (capped at 100); > 1 means
   * the returned location — and its page/anchor — is one of several
   * plausible ones.
   */
  occurrences: number;
  /**
   * `false` marks a data candidate accepted on value uniqueness alone
   * (numeric tier's synonym rescue), where a fabricated label is
   * indistinguishable from a synonym.
   */
  corroborated: boolean;
  /**
   * Source anchor of the provenance span overlapping the hit the most; only
   * present when `provenanceSpans` was passed. With block-level provenance
   * a PDF hit carries its approximate box, a table hit its cell.
   */
  anchor?: SourceAnchor;
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
export function locateQuote(
  markdown: string,
  quote: string,
  provenanceSpans?: Provenance['spans'] | null,
): LocatedQuote | null;
