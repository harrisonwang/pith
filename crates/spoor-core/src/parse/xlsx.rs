use crate::engine::{ProvenanceLevel, TableFilter};
use crate::json_schema::{
    HeaderInfo, PreambleInfo, RowRange, TableEntry, a1_range, cells_to_values,
};
use crate::limits;
use crate::output::{gfm_table_size, gfm_table_with_spans};
use crate::parse::ExtractedMarkdown;
use crate::parse::csv::validate_columns;
use crate::parse::table_provenance::cell_spans;
use crate::source::Source;
use anyhow::{Context, Result, anyhow};
use calamine::{Data, Reader, Xlsx, open_workbook_from_rs};
use std::collections::BTreeMap;
use std::io::Cursor;

const DEFAULT_PREVIEW_ROWS: usize = 100;

pub fn extract(
    source: &Source<'_>,
    max_parse_bytes: usize,
    provenance: ProvenanceLevel,
) -> Result<ExtractedMarkdown> {
    drop(limits::open_zip_archive(
        source.bytes(),
        "xlsx",
        max_parse_bytes,
    )?);
    let cursor = Cursor::new(source.bytes());
    let mut wb: Xlsx<_> = open_workbook_from_rs(cursor).context("failed to open xlsx")?;

    let mut out = String::new();
    let mut spans = Vec::new();
    let names = wb.sheet_names();

    for name in &names {
        let range = wb
            .worksheet_range(name)
            .with_context(|| format!("could not read sheet: {name}"))?;

        let prefix = format!(
            "{}## Sheet: {name}\n\n",
            if out.is_empty() { "" } else { "\n" }
        );

        let raw: Vec<Vec<String>> = range
            .rows()
            .map(|row| row.iter().map(format_cell_value).collect())
            .collect();

        let table_rows = match raw.split_first() {
            Some((header, data)) => {
                // Keep the header row; drop fully empty data rows.
                let mut table_rows = vec![header.clone()];
                table_rows.extend(
                    data.iter()
                        .filter(|row| !row.iter().all(String::is_empty))
                        .cloned(),
                );
                table_rows
            }
            None => Vec::new(),
        };
        limits::ensure_parse_size(
            out.len()
                .saturating_add(prefix.len())
                .saturating_add(gfm_table_size(&table_rows)),
            max_parse_bytes,
            "XLSX Markdown rendering",
        )?;
        out.push_str(&prefix);
        let (table, rendered_cells) = gfm_table_with_spans(&table_rows);
        if matches!(provenance, ProvenanceLevel::Block) {
            spans.extend(cell_spans(
                &table_rows,
                &rendered_cells,
                out.len(),
                Some(name.as_str()),
            ));
        }
        out.push_str(&table);
    }

    Ok(ExtractedMarkdown {
        markdown: out,
        warnings: Vec::new(),
        page_count: None,
        provenance: spans,
    })
}

pub fn extract_table_entries(
    source: &Source<'_>,
    source_label: &str,
    filter: &TableFilter,
    max_parse_bytes: usize,
) -> Result<Vec<TableEntry>> {
    drop(limits::open_zip_archive(
        source.bytes(),
        "xlsx",
        max_parse_bytes,
    )?);
    let cursor = Cursor::new(source.bytes());
    let mut wb: Xlsx<_> = open_workbook_from_rs(cursor).context("failed to open xlsx")?;

    let workbook_sheets: Vec<String> = wb.sheet_names();

    if let Some(name) = filter.sheet.as_deref()
        && !workbook_sheets.iter().any(|s| s == name)
    {
        return Err(anyhow!(
            "sheet {:?} not found in {}; available sheets: {:?}",
            name,
            source_label,
            workbook_sheets
        ));
    }

    let mut entries = Vec::new();
    for sheet_name in &workbook_sheets {
        if let Some(wanted) = filter.sheet.as_deref()
            && sheet_name != wanted
        {
            continue;
        }

        let range = wb
            .worksheet_range(sheet_name)
            .with_context(|| format!("could not read sheet: {sheet_name}"))?;

        if range.is_empty() || range.width() == 0 {
            // Empty sheet: skip from tables[], but the name still appears in
            // workbook_sheets so consumers know it exists.
            continue;
        }

        let entry = build_entry(source_label, sheet_name, &workbook_sheets, &range, filter)?;
        entries.push(entry);
    }

    Ok(entries)
}

fn build_entry(
    source_label: &str,
    sheet_name: &str,
    workbook_sheets: &[String],
    range: &calamine::Range<Data>,
    filter: &TableFilter,
) -> Result<TableEntry> {
    let start = range.start().unwrap_or((0, 0));
    let end = range.end().unwrap_or(start);
    let column_count = range.width();

    let mut rows: Vec<(usize, Vec<String>)> = range
        .rows()
        .enumerate()
        .map(|(idx, row)| {
            let mut cells = row.iter().map(format_cell_value).collect::<Vec<_>>();
            pad_row(&mut cells, column_count);
            (start.0 as usize + idx + 1, cells)
        })
        .collect();

    if rows.is_empty() {
        return Ok(empty_entry(source_label, sheet_name, workbook_sheets));
    }

    let header_index = detect_header_index(&rows);
    let header_row_num = rows[header_index].0;
    let header_list: Vec<String> = std::mem::take(&mut rows[header_index].1);

    if !filter.columns.is_empty() {
        validate_columns(
            &filter.columns,
            &header_list,
            source_label,
            Some(sheet_name),
        )?;
    }

    let kept_columns = if filter.columns.is_empty() {
        None
    } else {
        Some(filter.columns.as_slice())
    };

    let headers: BTreeMap<String, HeaderInfo> = header_list
        .iter()
        .enumerate()
        .filter(|(_, h)| kept_columns.is_none_or(|cols| cols.iter().any(|c| c == *h)))
        .map(|(idx, h)| (h.clone(), HeaderInfo::new(idx)))
        .collect();

    let title_index = detect_title_index(&rows[..header_index]);
    let title = title_index.and_then(|idx| single_non_empty_cell(&rows[idx].1));

    let preamble = if header_index > 1 {
        let search_range = if let Some(ti) = title_index {
            &rows[ti + 1..header_index]
        } else {
            &rows[..header_index]
        };

        search_range
            .iter()
            .find(|(_, cells)| has_content(cells))
            .map(|(row_num, cells)| {
                let mut content = cells_to_values(cells, &header_list);
                if let Some(cols) = kept_columns {
                    content.retain(|k, _| cols.iter().any(|c| c == k));
                }
                PreambleInfo {
                    row: *row_num,
                    content,
                }
            })
    } else {
        None
    };

    let data_cells: Vec<(usize, Vec<String>)> = rows[header_index + 1..]
        .iter()
        .filter(|(_, cells)| has_content(cells))
        .cloned()
        .collect();

    let data_row_count = data_cells.len();

    let selected = apply_row_filters(&data_cells, filter);
    let selected_count = selected.len();

    let table_rows: Vec<BTreeMap<String, String>> = selected
        .iter()
        .map(|(_, cells)| {
            let mut row = cells_to_values(cells, &header_list);
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
        data_cells.first().map(|(r, _)| *r),
        data_cells.last().map(|(r, _)| *r),
    );

    Ok(TableEntry {
        source: source_label.to_string(),
        format: "xlsx".to_string(),
        sheet: Some(sheet_name.to_string()),
        workbook_sheets: Some(workbook_sheets.to_vec()),
        delimiter: None,
        title,
        range: Some(a1_range(
            start.0 as usize + 1,
            start.1 as usize + 1,
            end.0 as usize + 1,
            end.1 as usize + 1,
        )),
        column_count,
        header_row: Some(header_row_num),
        headers,
        preamble,
        rows: table_rows,
        row_range,
        truncated,
        warnings,
    })
}

fn apply_row_filters(
    data_cells: &[(usize, Vec<String>)],
    filter: &TableFilter,
) -> Vec<(usize, Vec<String>)> {
    if let Some((first, last)) = filter.row_range {
        return data_cells
            .iter()
            .filter(|(row_num, _)| *row_num >= first && *row_num <= last)
            .cloned()
            .collect();
    }

    let offset = filter.offset.unwrap_or(0);
    let limit = filter.limit.unwrap_or(DEFAULT_PREVIEW_ROWS);
    data_cells
        .iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect()
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
            "showing {} rows from --rows {}:{}; sheet contains {} data rows{}",
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
            "showing {selected_count} rows (--offset {offset} --limit {limit}); sheet contains {data_row_count} data rows"
        ));
    } else {
        warnings.push(format!(
            "preview limited to first {selected_count} data rows out of {data_row_count}"
        ));
    }

    warnings
}

fn empty_entry(source_label: &str, sheet_name: &str, workbook_sheets: &[String]) -> TableEntry {
    TableEntry {
        source: source_label.to_string(),
        format: "xlsx".to_string(),
        sheet: Some(sheet_name.to_string()),
        workbook_sheets: Some(workbook_sheets.to_vec()),
        delimiter: None,
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

fn format_cell_value(cell: &Data) -> String {
    let raw = match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => (if *b { "TRUE" } else { "FALSE" }).to_string(),
        Data::DateTime(dt) => format_excel_datetime(dt.as_f64()),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => e.to_string(),
    };
    raw.replace("\r\n", "\n").replace('\r', "\n")
}

fn pad_row(row: &mut Vec<String>, column_count: usize) {
    while row.len() < column_count {
        row.push(String::new());
    }
}

fn detect_header_index(rows: &[(usize, Vec<String>)]) -> usize {
    let mut best = None::<(usize, i64)>;

    for (idx, (_, cells)) in rows.iter().enumerate().take(20) {
        let non_empty = cells.iter().filter(|cell| !cell.trim().is_empty()).count();
        if non_empty == 0 {
            continue;
        }

        let total_chars = cells
            .iter()
            .filter(|cell| !cell.trim().is_empty())
            .map(|cell| cell.chars().count())
            .sum::<usize>();
        let long_cells = cells
            .iter()
            .filter(|cell| cell.chars().count() > 40)
            .count();
        let next_non_empty = rows
            .iter()
            .skip(idx + 1)
            .find(|(_, row)| has_content(row))
            .map(|(_, row)| row.iter().filter(|cell| !cell.trim().is_empty()).count())
            .unwrap_or(0);

        let score = non_empty as i64 * 100 + next_non_empty.min(non_empty) as i64 * 10
            - long_cells as i64 * 80
            - total_chars as i64 / 10
            - idx as i64;

        if best.is_none_or(|(_, best_score)| score > best_score) {
            best = Some((idx, score));
        }
    }

    best.map(|(idx, _)| idx).unwrap_or(0)
}

fn detect_title_index(rows: &[(usize, Vec<String>)]) -> Option<usize> {
    rows.iter()
        .position(|(_, cells)| single_non_empty_cell(cells).is_some())
}

fn single_non_empty_cell(cells: &[String]) -> Option<String> {
    let mut values = cells.iter().filter(|cell| !cell.trim().is_empty());
    let first = values.next()?.trim().to_string();
    values.next().is_none().then_some(first)
}

fn has_content(cells: &[String]) -> bool {
    cells.iter().any(|cell| !cell.trim().is_empty())
}

fn format_excel_datetime(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return value.to_string();
    }

    let mut days = value.floor() as i64;
    let mut seconds = ((value - days as f64) * 86_400.0).round() as i64;
    if seconds >= 86_400 {
        days += 1;
        seconds -= 86_400;
    }

    if days == 0 {
        return format_time(seconds);
    }

    let (year, month, day) = civil_from_days(days - 25_569);
    if seconds == 0 {
        format!("{year:04}-{month:02}-{day:02}")
    } else {
        format!("{year:04}-{month:02}-{day:02}T{}", format_time(seconds))
    }
}

fn format_time(seconds: i64) -> String {
    let hour = seconds / 3600;
    let minute = (seconds % 3600) / 60;
    let second = seconds % 60;
    format!("{hour:02}:{minute:02}:{second:02}")
}

// Convert days since 1970-01-01 to Gregorian date.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year as i32, m as u32, d as u32)
}
