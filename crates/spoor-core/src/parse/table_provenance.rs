//! Cell provenance for the document-mode (Markdown) rendering of tables.
//!
//! `gfm_table_with_spans` reports where each rendered cell landed; this
//! module turns data-row cells into `Cell { sheet, row, column }` anchors.
//! `row` is the 1-based data row of the *rendered* table — the table the
//! agent actually reads — and `column` is the header cell's original text.
//! Header cells themselves are labels, not addressable data, so they carry
//! no anchor and fall outside the spans (provenance need not tile tables).

use crate::output::RenderedCell;
use crate::result::{ProvenanceSpan, SourceAnchor, TextRange};

/// Turn rendered data cells into provenance spans. `offset` shifts the cell
/// ranges into the final document (XLSX prepends sheet headings); `sheet`
/// names the worksheet, `None` for CSV.
pub(crate) fn cell_spans(
    rows: &[Vec<String>],
    rendered: &[RenderedCell],
    offset: usize,
    sheet: Option<&str>,
) -> Vec<ProvenanceSpan> {
    let Some(header) = rows.first() else {
        return Vec::new();
    };
    rendered
        .iter()
        .filter(|cell| cell.row > 0)
        .map(|cell| ProvenanceSpan {
            output: TextRange {
                start: offset + cell.start,
                end: offset + cell.end,
            },
            source: SourceAnchor::Cell {
                sheet: sheet.map(str::to_string),
                row: cell.row,
                column: header
                    .get(cell.column)
                    .cloned()
                    .unwrap_or_else(|| format!("column {}", cell.column + 1)),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::cell_spans;
    use crate::output::gfm_table_with_spans;
    use crate::result::SourceAnchor;

    #[test]
    fn data_cells_anchor_with_rendered_row_and_header_name() {
        let rows = vec![
            vec!["名称".to_string(), "金额".to_string()],
            vec!["营收".to_string(), "777102".to_string()],
            vec!["净利".to_string(), "53128".to_string()],
        ];
        let (markdown, rendered) = gfm_table_with_spans(&rows);
        let spans = cell_spans(&rows, &rendered, 0, Some("Sheet1"));

        // Four data cells, none for the header row.
        assert_eq!(spans.len(), 4);
        let amount = spans
            .iter()
            .find(|span| &markdown[span.output.start..span.output.end] == "53128")
            .expect("cell span for 53128");
        let SourceAnchor::Cell { sheet, row, column } = &amount.source else {
            panic!("expected cell anchor: {:?}", amount.source);
        };
        assert_eq!(sheet.as_deref(), Some("Sheet1"));
        assert_eq!(*row, 2);
        assert_eq!(column, "金额");
    }

    #[test]
    fn offset_shifts_ranges_into_the_document() {
        let rows = vec![vec!["h".to_string()], vec!["v".to_string()]];
        let (markdown, rendered) = gfm_table_with_spans(&rows);
        let spans = cell_spans(&rows, &rendered, 100, None);
        assert_eq!(spans.len(), 1);
        assert_eq!(
            &markdown[spans[0].output.start - 100..spans[0].output.end - 100],
            "v"
        );
    }

    #[test]
    fn empty_table_yields_no_spans() {
        assert!(cell_spans(&[], &[], 0, None).is_empty());
    }
}
