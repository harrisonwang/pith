use crate::cli::Cli;
use crate::source::{ResolvedInput, is_url, resolve_input};
use anyhow::{Context, Result, anyhow};
use glob::{MatchOptions, glob_with};
use spoor_core::{
    DocumentFilter, Format, JsonOutput, OutputMode, ParseContent, ParseLimits, ParseRequest,
    ProvenanceLevel, SpoorError, SpoorWarning, TableFilter, default_mode_for, detect_format,
    extract_media, limit_markdown_output, parse_document_result, parse_tables, render_documents,
    render_json_limited,
};

const MAX_FAILURE_DIAGNOSTICS: usize = 20;

pub(crate) enum CommandOutput {
    Text(String),
    Binary(Vec<u8>),
}

pub(crate) fn run(cli: Cli) -> Result<CommandOutput> {
    validate_max_parse_mib(cli.max_parse_mib)?;
    if let Some(resource) = cli.extract.clone() {
        return extract_resource(&cli, &resource).map(CommandOutput::Binary);
    }
    validate_max_output_kib(cli.max_output_kib)?;
    if cli.provenance.is_some() {
        return run_provenance(cli).map(CommandOutput::Text);
    }
    run_parse(cli).map(CommandOutput::Text)
}

/// Single-input path for `--provenance`: parse one document and emit the whole
/// `ParseResult` as JSON (Markdown plus the output→source mapping). Provenance
/// is structured data, so it goes to stdout as JSON rather than polluting the
/// Markdown; offsets index one document's Markdown, so this takes a single
/// input rather than a concatenated batch.
fn run_provenance(cli: Cli) -> Result<String> {
    let level: ProvenanceLevel = cli
        .provenance
        .expect("run_provenance requires --provenance")
        .into();

    let inputs = expand_inputs(&cli.inputs)?;
    if inputs.len() != 1 {
        return Err(anyhow!("--provenance 目前仅支持单个输入"));
    }
    let input = resolve_input(&inputs[0], cli.max_parse_bytes())?;
    let mut request = request_for(
        &input,
        cli.format.map(Into::into),
        TableFilter::default(),
        build_document_filter(&cli)?,
        cli.max_parse_bytes(),
        cli.max_work_units,
    );
    request.provenance = level;

    let format = detect_format(&request)?;
    // Provenance exists for PDF (page/line anchors), linear formats (input
    // byte ranges) and tables (cell anchors, block level). Reflowable
    // documents (DOCX/PPTX/HTML/EPUB/IPYNB) have no mapping yet and get one
    // clear error instead of silently returning none.
    if !matches!(
        format,
        Format::Pdf | Format::PlainText | Format::Markdown | Format::Csv | Format::Xlsx
    ) {
        return Err(anyhow!(
            "--provenance 支持 PDF（页/行级）、纯文本/Markdown（输入区间）与 CSV/XLSX（单元格，仅 block）；当前格式为 {format}。"
        ));
    }

    let result = parse_document_result(&request)?;
    let json = serde_json::to_string_pretty(&result)
        .map_err(|error| anyhow!("序列化 provenance 结果失败：{error}"))?;
    // Keep the stdout byte cap a hard contract; truncating JSON would make it
    // invalid, so over-budget output is an error pointing at the way to shrink.
    if json.len() > cli.max_output_bytes() {
        return Err(anyhow!(
            "provenance JSON 约 {} 字节，超过 --max-output-kib 上限；用 --pages 缩小范围或调高上限。",
            json.len()
        ));
    }
    Ok(json)
}

fn extract_resource(cli: &Cli, resource: &str) -> Result<Vec<u8>> {
    let inputs = expand_inputs(&cli.inputs)?;
    if inputs.len() != 1 {
        return Err(anyhow!("--extract 仅支持单个输入"));
    }
    let input = resolve_input(&inputs[0], cli.max_parse_bytes())?;
    let request = request_for(
        &input,
        None,
        TableFilter::default(),
        DocumentFilter::default(),
        cli.max_parse_bytes(),
        cli.max_work_units,
    );
    extract_media(&request, resource).map_err(Into::into)
}

fn run_parse(cli: Cli) -> Result<String> {
    let inputs = expand_inputs(&cli.inputs)?;
    let format_hint = cli.format.map(Into::into);

    // Resolve each input independently: one unreadable file or failed fetch
    // must not abort the whole batch. Failures are collected and surfaced as
    // warnings (stderr, plus the JSON envelope in json mode) so the remaining
    // inputs still produce output. We only fail hard (exit 1) when *nothing*
    // succeeds.
    let mut resolved = Vec::with_capacity(inputs.len());
    let mut failures = Vec::new();
    let mut source_bytes = 0usize;
    for input in inputs {
        let remaining = cli.max_parse_bytes().saturating_sub(source_bytes);
        match resolve_input(&input, remaining) {
            Ok(input_data) => {
                let request = request_for(
                    &input_data,
                    format_hint,
                    TableFilter::default(),
                    build_document_filter(&cli)?,
                    remaining,
                    cli.max_work_units,
                );
                match detect_format(&request) {
                    Ok(format) => {
                        source_bytes += input_data.len();
                        resolved.push(PreparedInput {
                            input: input_data,
                            format,
                            max_parse_bytes: remaining,
                        });
                    }
                    Err(error) => failures.push(InputFailure::from_spoor(input, &error)),
                }
            }
            Err(e) => failures.push(InputFailure::from_error(input, &e)),
        }
    }

    let formats: Vec<_> = resolved.iter().map(|resolved| resolved.format).collect();

    // --pages is honored only by PDF (page-oriented). On any other format it is a
    // silent no-op, so warn — mirroring how md mode flags ignored table options,
    // so the agent never assumes a slice was applied when it was not.
    if cli.pages.is_some() && formats.iter().any(|format| !matches!(format, Format::Pdf)) {
        eprintln!("warning: --pages 仅对 PDF 按页生效，其他格式已忽略");
    }

    let mode = cli
        .mode
        .map(Into::into)
        .unwrap_or_else(|| default_mode_for(&formats));

    match mode {
        OutputMode::Md => {
            warn_unused_narrowing(&cli);
            let mut documents = Vec::with_capacity(resolved.len());
            let mut parse_warnings = Vec::new();
            let mut extracted_bytes = 0usize;
            let mut total_pdf_pages = 0usize;
            for resolved in &resolved {
                let request = request_for(
                    &resolved.input,
                    Some(resolved.format),
                    TableFilter::default(),
                    build_document_filter(&cli)?,
                    resolved.max_parse_bytes,
                    cli.max_work_units,
                );
                match parse_document_result(&request) {
                    Ok(result) => {
                        let page_count = result.stats.page_count;
                        let ParseContent::Document(document) = result.content else {
                            failures.push(InputFailure::from_error(
                                resolved.input.label.clone(),
                                &anyhow!("内部错误：Markdown 模式返回了表格结果"),
                            ));
                            continue;
                        };
                        if let Err(error) = retain_within_parse_budget(
                            &mut extracted_bytes,
                            document.markdown.len(),
                            cli.max_parse_bytes(),
                            "retained extracted documents",
                        ) {
                            failures.push(InputFailure::from_error(
                                resolved.input.label.clone(),
                                &error,
                            ));
                        } else {
                            parse_warnings.extend(result.warnings.into_iter().map(|warning| {
                                InputWarning {
                                    source: resolved.input.label.clone(),
                                    warning,
                                }
                            }));
                            total_pdf_pages += page_count.unwrap_or(0);
                            documents.push(document);
                        }
                    }
                    Err(error) => failures.push(InputFailure::from_spoor(
                        resolved.input.label.clone(),
                        &error,
                    )),
                }
            }
            if documents.is_empty() {
                return Err(all_failed_error(&failures));
            }
            report_skipped(&failures);
            report_parse_warnings(&parse_warnings);
            let markdown = render_documents(&documents, mode)?;
            // Reserve room for in-band diagnostics so the total remains within
            // --max-output-kib and agents do not lose warnings first.
            let diagnostics = [
                markdown_skipped_block(&failures),
                markdown_parse_warnings_block(&parse_warnings),
            ]
            .into_iter()
            .flatten()
            .collect::<String>();
            let limited_diagnostics = limit_markdown_output(diagnostics, cli.max_output_bytes());
            report_output_truncation(limited_diagnostics.warning.as_deref());
            let budget = cli
                .max_output_bytes()
                .saturating_sub(limited_diagnostics.content.len());
            let limited = limit_markdown_output(markdown, budget);
            report_output_truncation(limited.warning.as_deref());
            // Whenever the agent sees fewer pages than the document holds — either
            // truncated by the output cap or narrowed by --pages — surface the
            // total page count so it knows to fetch the rest with --pages instead
            // of assuming it has the whole document.
            if total_pdf_pages > 0 {
                let shown = limited.content.matches("## Page ").count();
                if limited.warning.is_some() {
                    eprintln!(
                        "warning: PDF 共 {total_pdf_pages} 页，本次输出截断到约 {shown} 页；用 --pages <start,end> 获取后续页面。"
                    );
                } else if shown < total_pdf_pages {
                    eprintln!(
                        "info: PDF 共 {total_pdf_pages} 页，本次为其中约 {shown} 页；用 --pages <start,end> 获取其他页面。"
                    );
                }
            }
            let mut content = limited.content;
            content.push_str(&limited_diagnostics.content);
            Ok(content)
        }
        OutputMode::Json => {
            let filter = build_filter(&cli)?;
            let mut tables = Vec::new();
            let mut successes = 0usize;
            let mut extracted_bytes = 0usize;
            for resolved in &resolved {
                let request = request_for(
                    &resolved.input,
                    Some(resolved.format),
                    filter.clone(),
                    DocumentFilter::default(),
                    resolved.max_parse_bytes,
                    cli.max_work_units,
                );
                match parse_tables(&request) {
                    Ok(extracted) => {
                        if let Err(error) = retain_within_parse_budget(
                            &mut extracted_bytes,
                            extracted.serialized_bytes,
                            cli.max_parse_bytes(),
                            "retained extracted tables",
                        ) {
                            failures.push(InputFailure::from_error(
                                resolved.input.label.clone(),
                                &error,
                            ));
                        } else {
                            successes += 1;
                            tables.extend(extracted.tables);
                        }
                    }
                    Err(error) => failures.push(InputFailure::from_spoor(
                        resolved.input.label.clone(),
                        &error,
                    )),
                }
            }
            if successes == 0 {
                return Err(all_failed_error(&failures));
            }
            report_skipped(&failures);
            let mut output = JsonOutput::new(tables);
            output.warnings = failures.iter().map(InputFailure::display).collect();
            let limited = render_json_limited(&output, cli.max_output_bytes());
            report_output_truncation(limited.warning.as_deref());
            Ok(limited.content)
        }
    }
}

#[derive(Debug)]
struct InputWarning {
    source: String,
    warning: SpoorWarning,
}

impl InputWarning {
    fn display(&self) -> String {
        let warning = serde_json::to_string(&self.warning)
            .unwrap_or_else(|_| format!("{}: {}", self.warning.code, self.warning.message));
        format!("{}: {warning}", self.source)
    }
}

fn validate_max_output_kib(max_output_kib: usize) -> Result<()> {
    if max_output_kib == 0 {
        return Err(anyhow!("--max-output-kib 不能小于 1"));
    }
    Ok(())
}

fn validate_max_parse_mib(max_parse_mib: usize) -> Result<()> {
    if max_parse_mib == 0 {
        return Err(anyhow!("--max-parse-mib 不能小于 1"));
    }
    Ok(())
}

fn retain_within_parse_budget(
    retained: &mut usize,
    additional: usize,
    max_parse_bytes: usize,
    stage: &str,
) -> Result<()> {
    let total = retained.checked_add(additional).unwrap_or(usize::MAX);
    if total > max_parse_bytes {
        return Err(SpoorError::parse_memory_limit(max_parse_bytes, stage).into());
    }
    *retained = total;
    Ok(())
}

fn report_output_truncation(warning: Option<&str>) {
    if let Some(warning) = warning {
        eprintln!("warning: {warning}");
    }
}

#[derive(Debug)]
struct InputFailure {
    source: String,
    message: String,
    structured: Option<SpoorError>,
}

impl InputFailure {
    fn from_error(source: String, error: &anyhow::Error) -> Self {
        let structured = error.downcast_ref::<SpoorError>().cloned();
        let message = structured
            .as_ref()
            .map(SpoorError::to_json)
            .unwrap_or_else(|| error.root_cause().to_string());

        Self {
            source,
            message,
            structured,
        }
    }

    fn from_spoor(source: String, error: &SpoorError) -> Self {
        Self {
            source,
            message: error.to_json(),
            structured: Some(error.clone()),
        }
    }

    fn display(&self) -> String {
        format!("{}: {}", self.source, self.message)
    }
}

/// Emit one stderr line per skipped input. In json mode the same messages
/// also land in the envelope's top-level `warnings[]`; stderr keeps a human
/// running the command in a terminal informed regardless of output mode.
fn report_skipped(failures: &[InputFailure]) {
    for failure in failures.iter().take(MAX_FAILURE_DIAGNOSTICS) {
        eprintln!("warning: 已跳过 {}", failure.display());
    }
    if failures.len() > MAX_FAILURE_DIAGNOSTICS {
        eprintln!(
            "warning: 另有 {} 条跳过警告未显示",
            failures.len() - MAX_FAILURE_DIAGNOSTICS
        );
    }
}

fn report_parse_warnings(warnings: &[InputWarning]) {
    for warning in warnings.iter().take(MAX_FAILURE_DIAGNOSTICS) {
        eprintln!("warning: 输出不完整 {}", warning.display());
    }
    if warnings.len() > MAX_FAILURE_DIAGNOSTICS {
        eprintln!(
            "warning: 另有 {} 条解析完整性警告未显示",
            warnings.len() - MAX_FAILURE_DIAGNOSTICS
        );
    }
}

/// In-band counterpart of `report_skipped` for markdown mode. Agents often
/// read only stdout (e.g. redirected to a file), so a partial-batch failure
/// must be visible there too — mirroring the JSON envelope's `warnings[]`.
fn markdown_skipped_block(failures: &[InputFailure]) -> Option<String> {
    if failures.is_empty() {
        return None;
    }

    let mut block = format!(
        "\n> [!WARNING]\n> {} 个输入解析失败，已跳过：\n",
        failures.len()
    );
    for failure in failures.iter().take(MAX_FAILURE_DIAGNOSTICS) {
        block.push_str(&format!(
            "> - {}\n",
            failure.display().replace(['\r', '\n'], " ")
        ));
    }
    if failures.len() > MAX_FAILURE_DIAGNOSTICS {
        block.push_str(&format!(
            "> - …… 另有 {} 个失败未列出\n",
            failures.len() - MAX_FAILURE_DIAGNOSTICS
        ));
    }

    Some(block)
}

fn markdown_parse_warnings_block(warnings: &[InputWarning]) -> Option<String> {
    if warnings.is_empty() {
        return None;
    }

    let mut block = format!(
        "\n> [!WARNING]\n> 以下 {} 条警告表示对应位置的输出可能不完整：\n",
        warnings.len()
    );
    for warning in warnings.iter().take(MAX_FAILURE_DIAGNOSTICS) {
        block.push_str(&format!(
            "> - {}\n",
            warning.display().replace(['\r', '\n'], " ")
        ));
    }
    if warnings.len() > MAX_FAILURE_DIAGNOSTICS {
        block.push_str(&format!(
            "> - …… 另有 {} 条解析完整性警告未列出\n",
            warnings.len() - MAX_FAILURE_DIAGNOSTICS
        ));
    }

    Some(block)
}

/// Error returned when every input failed (exit 1). A lone failure is
/// surfaced verbatim; multiple are aggregated into one message.
fn all_failed_error(failures: &[InputFailure]) -> anyhow::Error {
    match failures {
        [
            InputFailure {
                structured: Some(error),
                ..
            },
        ] => anyhow::Error::new(error.clone()),
        [only] => anyhow!(only.display()),
        _ => anyhow!(
            "全部 {} 个输入均失败：\n  - {}{}",
            failures.len(),
            failures
                .iter()
                .take(MAX_FAILURE_DIAGNOSTICS)
                .map(InputFailure::display)
                .collect::<Vec<_>>()
                .join("\n  - "),
            if failures.len() > MAX_FAILURE_DIAGNOSTICS {
                format!(
                    "\n  - …… 另有 {} 个失败未显示",
                    failures.len() - MAX_FAILURE_DIAGNOSTICS
                )
            } else {
                String::new()
            }
        ),
    }
}

fn build_filter(cli: &Cli) -> Result<TableFilter> {
    let rows = match &cli.rows {
        Some(s) => Some(parse_row_range(s)?),
        None => None,
    };
    // Funnel through the same validator the language bindings use, so the
    // row-range rules (>= 1, first <= last, rows ⟂ limit/offset) live in one
    // place. Surface a failure as a friendly CLI arg error (matching the
    // sibling `parse_row_range` shape errors) rather than the structured JSON
    // reserved for content/parse failures; clap also rejects the rows/limit
    // conflict at flag-parse time.
    TableFilter::build(
        cli.sheet.clone(),
        rows,
        cli.columns.clone(),
        cli.limit,
        cli.offset,
    )
    .map_err(|error| anyhow!("{}", error.reason))
}

/// Build a validated page filter from the CLI `--pages` flag. Like the row
/// filter, the page-range bounds live in the shared cross-host
/// `DocumentFilter::build`; a failure surfaces as a friendly CLI arg error
/// rather than the structured JSON reserved for content/parse failures.
fn build_document_filter(cli: &Cli) -> Result<DocumentFilter> {
    let pages = match &cli.pages {
        Some(s) => Some(parse_range_flag("--pages", s)?),
        None => None,
    };
    let mut filter = DocumentFilter::build(pages).map_err(|error| anyhow!("{}", error.reason))?;
    filter.keep_repeated_regions = cli.keep_repeated_regions;
    Ok(filter)
}

/// Parse a `<first>:<last>` range string into a 1-based pair. Bound validation
/// (>= 1, first <= last) lives in the shared cross-host `TableFilter::build` /
/// `DocumentFilter::build`, so it is not repeated here.
fn parse_row_range(s: &str) -> Result<(usize, usize)> {
    parse_range_flag("--rows", s)
}

fn parse_range_flag(flag: &str, s: &str) -> Result<(usize, usize)> {
    let (first, last) = s
        .split_once(':')
        .ok_or_else(|| anyhow!("{flag} expects <first>:<last>, got {s:?}"))?;
    let first: usize = first
        .trim()
        .parse()
        .map_err(|_| anyhow!("{flag}: invalid first number {first:?}"))?;
    let last: usize = last
        .trim()
        .parse()
        .map_err(|_| anyhow!("{flag}: invalid last number {last:?}"))?;
    Ok((first, last))
}

fn warn_unused_narrowing(cli: &Cli) {
    let used = cli.sheet.is_some()
        || cli.rows.is_some()
        || !cli.columns.is_empty()
        || cli.limit.is_some()
        || cli.offset.is_some();
    if used {
        eprintln!("warning: Markdown 模式会忽略 --sheet/--rows/--columns/--limit/--offset");
    }
}

fn expand_inputs(inputs: &[String]) -> Result<Vec<String>> {
    let mut expanded = Vec::new();

    for input in inputs {
        if is_url(input) || !has_glob_meta(input) {
            expanded.push(input.clone());
            continue;
        }

        expanded
            .extend(expand_glob(input).with_context(|| format!("failed to expand glob: {input}"))?);
    }

    Ok(expanded)
}

fn expand_glob(pattern: &str) -> Result<Vec<String>> {
    let options = MatchOptions {
        case_sensitive: !cfg!(windows),
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };

    let mut matches = glob_with(pattern, options)
        .with_context(|| format!("invalid glob pattern: {pattern}"))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to read glob matches: {pattern}"))?
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    matches.sort();
    matches.dedup();

    if matches.is_empty() {
        return Err(anyhow!("glob matched no files: {pattern}"));
    }

    Ok(matches)
}

fn has_glob_meta(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

struct PreparedInput {
    input: ResolvedInput,
    format: Format,
    max_parse_bytes: usize,
}

fn request_for<'a>(
    input: &'a ResolvedInput,
    format_hint: Option<Format>,
    table_filter: TableFilter,
    document_filter: DocumentFilter,
    max_parse_bytes: usize,
    max_work_units: Option<usize>,
) -> ParseRequest<'a> {
    ParseRequest {
        bytes: &input.bytes,
        source_name: Some(&input.label),
        content_type: input.content_type.as_deref(),
        format_hint,
        table_filter,
        document_filter,
        limits: ParseLimits {
            max_parse_bytes,
            max_work_units,
        },
        provenance: ProvenanceLevel::Off,
    }
}

#[cfg(test)]
struct TestDir {
    path: std::path::PathBuf,
}

#[cfg(test)]
impl TestDir {
    fn new(name: &str) -> Result<Self> {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("spoor-{name}-{unique}"));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_inputs_expand_and_sort() -> Result<()> {
        let dir = TestDir::new("glob_inputs_expand_and_sort")?;
        std::fs::write(dir.path().join("b.pdf"), b"")?;
        std::fs::write(dir.path().join("a.pdf"), b"")?;
        std::fs::write(dir.path().join("notes.txt"), b"")?;

        let pattern = dir.path().join("*.pdf").to_string_lossy().into_owned();
        let expanded = expand_inputs(&[pattern])?;
        let names = expanded
            .iter()
            .map(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();

        assert_eq!(names, ["a.pdf", "b.pdf"]);
        Ok(())
    }

    #[test]
    fn glob_without_matches_errors() -> Result<()> {
        let dir = TestDir::new("glob_without_matches_errors")?;
        let pattern = dir.path().join("*.pdf").to_string_lossy().into_owned();
        let err = expand_inputs(&[pattern]).unwrap_err();

        assert!(err.to_string().contains("failed to expand glob"));
        assert!(format!("{err:#}").contains("glob matched no files"));
        Ok(())
    }

    #[test]
    fn parse_row_range_accepts_valid_input() {
        assert_eq!(parse_row_range("5:104").unwrap(), (5, 104));
        assert_eq!(parse_row_range("1:1").unwrap(), (1, 1));
        assert_eq!(parse_row_range(" 5 : 104 ").unwrap(), (5, 104));
    }

    #[test]
    fn parse_row_range_rejects_malformed_input() {
        // parse_row_range only validates the string shape; out-of-bound values
        // (e.g. 0:10, 104:5) are rejected later by TableFilter::build, covered
        // in spoor-core's engine tests.
        assert!(parse_row_range("5").is_err());
        assert!(parse_row_range("a:b").is_err());
        assert!(parse_row_range(":5").is_err());
        assert!(parse_row_range("5:").is_err());
    }

    #[test]
    fn all_failed_error_caps_diagnostics() {
        let failures = (0..25)
            .map(|index| InputFailure {
                source: format!("input-{index}"),
                message: "failed".to_string(),
                structured: None,
            })
            .collect::<Vec<_>>();

        let message = all_failed_error(&failures).to_string();
        assert!(message.contains("全部 25 个输入均失败"));
        assert!(message.contains("input-19"));
        assert!(!message.contains("input-20"));
        assert!(message.contains("另有 5 个失败未显示"));
    }

    #[test]
    fn retained_results_share_parse_budget() {
        let mut retained = 600;
        let error = retain_within_parse_budget(&mut retained, 500, 1024, "test").unwrap_err();

        assert_eq!(retained, 600);
        assert!(error.downcast_ref::<SpoorError>().is_some());
    }
}
