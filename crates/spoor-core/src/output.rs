//! Common helpers for extractors.

use crate::error::StructuredError;
use anyhow::Result;

pub struct MarkdownBuilder {
    buf: String,
    max_bytes: usize,
    exceeded: bool,
}

impl MarkdownBuilder {
    pub fn new() -> Self {
        Self::with_max_bytes(usize::MAX)
    }

    pub fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            buf: String::new(),
            max_bytes,
            exceeded: false,
        }
    }

    pub fn heading(&mut self, level: u8, text: &str) {
        let level = level.clamp(1, 6);
        self.ensure_blank_line();
        for _ in 0..level {
            self.push_char('#');
        }
        self.push_char(' ');
        self.push_str(text.trim());
        self.push_char('\n');
    }

    pub fn paragraph(&mut self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        self.ensure_blank_line();
        self.push_str(trimmed);
        self.push_char('\n');
    }

    pub fn raw(&mut self, s: &str) {
        self.push_str(s);
    }

    /// Append `rows` as a GFM table (first row = header), preceded by a blank
    /// line. No-op when there are no columns.
    pub fn table(&mut self, rows: &[Vec<String>]) {
        let rendered_size = gfm_table_size(rows);
        if rendered_size == 0 {
            return;
        }
        self.blank_line();
        if self.would_exceed(rendered_size) {
            self.exceeded = true;
            return;
        }
        self.push_str(&gfm_table(rows));
    }

    pub fn blank_line(&mut self) {
        self.ensure_blank_line();
    }

    fn ensure_blank_line(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        if !self.buf.ends_with('\n') {
            self.push_char('\n');
        }
        if !self.buf.ends_with("\n\n") {
            self.push_char('\n');
        }
    }

    pub fn build(mut self) -> Result<String> {
        if self.exceeded {
            return Err(
                StructuredError::parse_memory_limit(self.max_bytes, "Markdown rendering").into(),
            );
        }

        let trimmed_len = self.buf.trim_end().len();
        self.buf.truncate(trimmed_len);
        self.push_char('\n');
        if self.exceeded {
            return Err(
                StructuredError::parse_memory_limit(self.max_bytes, "Markdown rendering").into(),
            );
        }
        Ok(self.buf)
    }

    fn push_str(&mut self, value: &str) {
        if self.would_exceed(value.len()) {
            self.exceeded = true;
            return;
        }
        self.buf.push_str(value);
    }

    fn push_char(&mut self, value: char) {
        if self.would_exceed(value.len_utf8()) {
            self.exceeded = true;
            return;
        }
        self.buf.push(value);
    }

    fn would_exceed(&self, additional: usize) -> bool {
        self.exceeded || self.buf.len().saturating_add(additional) > self.max_bytes
    }
}

impl Default for MarkdownBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode bytes with charset detection (UTF-8 / GBK / GB18030 / Big5 / etc).
pub fn decode_text(bytes: &[u8]) -> String {
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(None, true);
    let (cow, _, _) = encoding.decode(bytes);
    cow.into_owned()
}

/// Escape one cell for a GFM table: escape pipes and collapse any newline /
/// carriage-return / tab to a single space so the row stays on one line.
pub fn escape_table_cell(cell: &str) -> String {
    cell.replace('|', "\\|").replace(['\n', '\r', '\t'], " ")
}

/// Render `rows` as a GFM table (first row = header). Ragged rows are padded
/// to the widest row, and every cell is escaped via [`escape_table_cell`].
/// Returns an empty string when there are no rows or no columns. Each line
/// ends with `\n`; callers add any surrounding blank lines.
pub fn gfm_table(rows: &[Vec<String>]) -> String {
    gfm_table_with_spans(rows).0
}

/// One rendered cell's byte range inside the string [`gfm_table_with_spans`]
/// returned. `row` is the 0-based row in `rows` (0 = header row), `column`
/// the 0-based column; the range covers the escaped cell text (empty cells
/// yield an empty range at their position and are skipped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedCell {
    pub row: usize,
    pub column: usize,
    pub start: usize,
    pub end: usize,
}

/// [`gfm_table`], additionally reporting where each non-empty cell landed —
/// the raw material for `Cell` provenance anchors.
pub fn gfm_table_with_spans(rows: &[Vec<String>]) -> (String, Vec<RenderedCell>) {
    if rows.is_empty() {
        return (String::new(), Vec::new());
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return (String::new(), Vec::new());
    }

    let mut out = String::new();
    let mut cells = Vec::new();
    push_table_row_with_spans(&mut out, &rows[0], cols, 0, &mut cells);
    out.push('|');
    for _ in 0..cols {
        out.push_str(" --- |");
    }
    out.push('\n');
    for (index, row) in rows[1..].iter().enumerate() {
        push_table_row_with_spans(&mut out, row, cols, index + 1, &mut cells);
    }
    (out, cells)
}

pub fn gfm_table_size(rows: &[Vec<String>]) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return 0;
    }

    let rows_size = rows.iter().fold(0usize, |total, row| {
        total.saturating_add(table_row_size(row, cols))
    });
    let separator_size = 1usize
        .saturating_add(cols.saturating_mul(" --- |".len()))
        .saturating_add(1);
    rows_size.saturating_add(separator_size)
}

fn table_row_size(row: &[String], cols: usize) -> usize {
    let cells_size = (0..cols).fold(0usize, |total, col| {
        total.saturating_add(escaped_table_cell_size(
            row.get(col).map_or("", String::as_str),
        ))
    });
    "| ".len()
        .saturating_add(cells_size)
        .saturating_add(cols.saturating_sub(1).saturating_mul(" | ".len()))
        .saturating_add(" |\n".len())
}

fn escaped_table_cell_size(cell: &str) -> usize {
    cell.chars().fold(0usize, |total, ch| {
        total.saturating_add(match ch {
            '|' => 2,
            '\n' | '\r' | '\t' => 1,
            _ => ch.len_utf8(),
        })
    })
}

fn push_table_row_with_spans(
    out: &mut String,
    row: &[String],
    cols: usize,
    row_index: usize,
    cells: &mut Vec<RenderedCell>,
) {
    out.push_str("| ");
    for col in 0..cols {
        if col > 0 {
            out.push_str(" | ");
        }
        let start = out.len();
        out.push_str(&escape_table_cell(row.get(col).map_or("", String::as_str)));
        if out.len() > start {
            cells.push(RenderedCell {
                row: row_index,
                column: col,
                start,
                end: out.len(),
            });
        }
    }
    out.push_str(" |\n");
}

#[cfg(test)]
mod tests {
    use super::{MarkdownBuilder, gfm_table, gfm_table_size};
    use crate::error::StructuredError;

    #[test]
    fn markdown_builder_stops_at_parse_budget() {
        let mut builder = MarkdownBuilder::with_max_bytes(8);
        builder.paragraph("12345678");

        let error = builder.build().unwrap_err();
        assert!(error.downcast_ref::<StructuredError>().is_some());
    }

    #[test]
    fn gfm_table_size_matches_rendered_bytes() {
        let rows = vec![
            vec!["a|b".to_string(), "c".to_string()],
            vec!["line\nbreak".to_string(), "值".to_string()],
        ];

        assert_eq!(gfm_table_size(&rows), gfm_table(&rows).len());
    }
}
