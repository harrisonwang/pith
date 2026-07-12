use crate::engine::TableFilter;
use crate::json_schema::{HeaderInfo, RowRange, TableEntry, a1_range, cells_to_values};
use crate::limits;
use crate::output::{decode_text, gfm_table, gfm_table_size};
use crate::source::Source;
use anyhow::{Result, anyhow};
use std::collections::BTreeMap;

const MARKDOWN_MAX_ROWS: usize = 1000;
const DEFAULT_PREVIEW_ROWS: usize = 100;

/// CSV → markdown table. Steps:
///   1. Decode bytes (handles GBK/GB18030/UTF-8 with chardetng).
///   2. Sniff delimiter from a sample (',' '\t' ';' '|').
///   3. Parse with `csv` crate, render as GFM table.
///   4. If file is huge, truncate to first N rows + a "(truncated)" line.
pub fn extract(source: &Source<'_>, max_parse_bytes: usize) -> Result<String> {
    let text = decode_text(source.bytes());
    let delimiter = sniff_delimiter(&text);

    let mut rdr = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(delimiter)
        .flexible(true)
        .from_reader(text.as_bytes());

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut truncated = false;

    for (i, rec) in rdr.records().enumerate() {
        if i >= MARKDOWN_MAX_ROWS {
            truncated = true;
            break;
        }
        let rec = rec?;
        rows.push(rec.iter().map(str::to_string).collect());
    }

    let marker_size = if truncated {
        format!("\n_(truncated at {MARKDOWN_MAX_ROWS} rows)_\n").len()
    } else {
        0
    };
    limits::ensure_parse_size(
        gfm_table_size(&rows).saturating_add(marker_size),
        max_parse_bytes,
        "CSV Markdown rendering",
    )?;

    let mut out = gfm_table(&rows);
    if truncated {
        out.push_str(&format!("\n_(truncated at {MARKDOWN_MAX_ROWS} rows)_\n"));
    }
    Ok(out)
}

pub fn extract_table_entries(
    source: &Source<'_>,
    source_label: &str,
    filter: &TableFilter,
    max_parse_bytes: usize,
) -> Result<Vec<TableEntry>> {
    let text = decode_text(source.bytes());
    let delimiter = sniff_delimiter(&text);
    let delimiter_str = (delimiter as char).to_string();

    let mut rdr = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(delimiter)
        .flexible(true)
        .from_reader(text.as_bytes());

    let mut headers: Vec<String> = Vec::new();
    let mut all_rows: Vec<(usize, Vec<String>)> = Vec::new();
    let mut row_count = 0usize;
    let mut column_count = 0usize;
    let mut retained_cell_bytes = 0usize;

    for rec in rdr.records() {
        let rec = rec?;
        let row: Vec<String> = rec.iter().map(normalize_cell).collect();
        row_count += 1;
        column_count = column_count.max(row.len());
        retained_cell_bytes = retained_cell_bytes.saturating_add(
            row.iter()
                .fold(0usize, |total, cell| total.saturating_add(cell.len())),
        );
        limits::ensure_parse_size(retained_cell_bytes, max_parse_bytes, "CSV retained cells")?;

        if row_count == 1 {
            headers = row;
        } else {
            all_rows.push((row_count, row));
        }
    }

    if row_count == 0 {
        return Ok(vec![empty_entry(source_label, &delimiter_str)]);
    }

    pad_row(&mut headers, column_count);
    for (_, row) in &mut all_rows {
        pad_row(row, column_count);
    }

    let data_row_count = all_rows.len();

    if !filter.columns.is_empty() {
        validate_columns(&filter.columns, &headers, source_label, None)?;
    }

    let selected: Vec<(usize, Vec<String>)> = apply_row_filters(&all_rows, filter);
    let selected_count = selected.len();

    let kept_columns = if filter.columns.is_empty() {
        None
    } else {
        Some(filter.columns.as_slice())
    };

    let headers_map: BTreeMap<String, HeaderInfo> = headers
        .iter()
        .enumerate()
        .filter(|(_, h)| kept_columns.is_none_or(|cols| cols.iter().any(|c| c == *h)))
        .map(|(idx, h)| (h.clone(), HeaderInfo::new(idx)))
        .collect();

    let table_rows: Vec<BTreeMap<String, String>> = selected
        .iter()
        .map(|(_, cells)| {
            let mut row = cells_to_values(cells, &headers);
            if let Some(cols) = kept_columns {
                row.retain(|k, _| cols.iter().any(|c| c == k));
            }
            row
        })
        .collect();

    let row_range = if let Some((first, last)) = selected.first().zip(selected.last()) {
        RowRange::new(first.0, last.0)
    } else if let Some((f, l)) = filter.row_range {
        RowRange::new(f, l)
    } else {
        RowRange::new(0, 0)
    };

    let truncated = selected_count < data_row_count;
    let warnings = build_warnings(
        selected_count,
        data_row_count,
        filter,
        all_rows.first().map(|(r, _)| *r),
        all_rows.last().map(|(r, _)| *r),
    );

    Ok(vec![TableEntry {
        source: source_label.to_string(),
        format: "csv".to_string(),
        sheet: None,
        workbook_sheets: None,
        delimiter: Some(delimiter_str),
        title: None,
        range: (column_count > 0).then(|| a1_range(1, 1, row_count, column_count)),
        column_count,
        header_row: None,
        headers: headers_map,
        preamble: None,
        rows: table_rows,
        row_range,
        truncated,
        warnings,
    }])
}

fn apply_row_filters(
    all_rows: &[(usize, Vec<String>)],
    filter: &TableFilter,
) -> Vec<(usize, Vec<String>)> {
    if let Some((first, last)) = filter.row_range {
        return all_rows
            .iter()
            .filter(|(row_num, _)| *row_num >= first && *row_num <= last)
            .cloned()
            .collect();
    }

    let offset = filter.offset.unwrap_or(0);
    let limit = filter.limit.unwrap_or(DEFAULT_PREVIEW_ROWS);
    all_rows.iter().skip(offset).take(limit).cloned().collect()
}

fn build_warnings(
    selected_count: usize,
    data_row_count: usize,
    filter: &TableFilter,
    data_first_row: Option<usize>,
    data_last_row: Option<usize>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if selected_count >= data_row_count {
        return warnings;
    }

    if let Some((first, last)) = filter.row_range {
        warnings.push(format!(
            "showing {} rows from --rows {}:{}; file contains {} data rows{}",
            selected_count,
            first,
            last,
            data_row_count,
            match (data_first_row, data_last_row) {
                (Some(f), Some(l)) => format!(" in range {f}..{l}"),
                _ => String::new(),
            }
        ));
    } else if filter.limit.is_some() || filter.offset.is_some() {
        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(DEFAULT_PREVIEW_ROWS);
        warnings.push(format!(
            "showing {selected_count} rows (--offset {offset} --limit {limit}); file contains {data_row_count} data rows"
        ));
    } else {
        warnings.push(format!(
            "preview limited to first {selected_count} data rows out of {data_row_count}"
        ));
    }

    warnings
}

pub(crate) fn validate_columns(
    requested: &[String],
    headers: &[String],
    source_label: &str,
    sheet: Option<&str>,
) -> Result<()> {
    let missing: Vec<&String> = requested
        .iter()
        .filter(|name| !headers.iter().any(|h| h == *name))
        .collect();

    if missing.is_empty() {
        return Ok(());
    }

    let available: Vec<&str> = headers
        .iter()
        .filter(|h| !h.trim().is_empty())
        .map(String::as_str)
        .collect();

    let location = match sheet {
        Some(s) => format!(" in sheet '{s}' of {source_label}"),
        None => format!(" in {source_label}"),
    };

    Err(anyhow!(
        "column(s) {:?} not found{}; available columns: {:?}",
        missing,
        location,
        available
    ))
}

fn empty_entry(source_label: &str, delimiter_str: &str) -> TableEntry {
    TableEntry {
        source: source_label.to_string(),
        format: "csv".to_string(),
        sheet: None,
        workbook_sheets: None,
        delimiter: Some(delimiter_str.to_string()),
        title: None,
        range: None,
        column_count: 0,
        header_row: None,
        headers: BTreeMap::new(),
        preamble: None,
        rows: Vec::new(),
        row_range: RowRange::new(0, 0),
        truncated: false,
        warnings: Vec::new(),
    }
}

fn sniff_delimiter(text: &str) -> u8 {
    let sample: String = text.lines().take(20).collect::<Vec<_>>().join("\n");
    let candidates = *b",\t;|";
    let mut best = (b',', 0usize);
    for &d in &candidates {
        let count = sample.bytes().filter(|&b| b == d).count();
        if count > best.1 {
            best = (d, count);
        }
    }
    best.0
}

fn normalize_cell(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn pad_row(row: &mut Vec<String>, column_count: usize) {
    while row.len() < column_count {
        row.push(String::new());
    }
}
