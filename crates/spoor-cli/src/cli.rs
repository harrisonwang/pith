use clap::{ArgAction, Parser, ValueEnum};
use spoor_core::{Format, OutputMode, ProvenanceLevel};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum FormatArg {
    Pdf,
    Docx,
    Xlsx,
    Csv,
    Pptx,
    Epub,
    Ipynb,
    Html,
    Markdown,
    Text,
}

impl From<FormatArg> for Format {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Pdf => Format::Pdf,
            FormatArg::Docx => Format::Docx,
            FormatArg::Xlsx => Format::Xlsx,
            FormatArg::Csv => Format::Csv,
            FormatArg::Pptx => Format::Pptx,
            FormatArg::Epub => Format::Epub,
            FormatArg::Ipynb => Format::Ipynb,
            FormatArg::Html => Format::Html,
            FormatArg::Markdown => Format::Markdown,
            FormatArg::Text => Format::PlainText,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ModeArg {
    Md,
    Json,
}

impl From<ModeArg> for OutputMode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Md => OutputMode::Md,
            ModeArg::Json => OutputMode::Json,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ProvenanceArg {
    /// One mapping per source page (PDF page-level).
    Page,
}

impl From<ProvenanceArg> for ProvenanceLevel {
    fn from(value: ProvenanceArg) -> Self {
        match value {
            ProvenanceArg::Page => ProvenanceLevel::Page,
        }
    }
}

const HELP_TEMPLATE: &str = "\
{about}

Usage:
  spoor [OPTIONS] <input>...

Arguments:
{positionals}

Options:
{options}

Common Patterns

  Tables (CSV/XLSX)
    spoor data.xlsx                         查看结构
    spoor data.xlsx --sheet Sheet1 --rows 5:104   按行切片
    spoor data.xlsx --columns 名称,数量        按列筛选

  Documents (DOCX/PDF)
    spoor report.docx                       提取文本
    spoor report.pdf --pages 1:3            仅提取前 3 页

  Pipes
    cat data.csv | spoor --format csv -      从 stdin 读取
    spoor report.pdf | llm \"总结\"            对接 LLM

  Media Extraction
    spoor doc.docx --extract spoor://docx/part/word/media/img.png > img.png

Defaults
  - 每表默认 100 行（--limit 翻页，--rows 定区间）
  - stdout 上限 256 KiB（--max-output-kib 调高）
  - 解析内存上限 64 MiB（--max-parse-mib 调高）

Examples
  spoor data.csv | jq '.tables[]'
  spoor https://example.com/report
  spoor \"*.pdf\"
  spoor report.pdf --provenance page
  spoor report.pdf --pages 1:3 --mode json
  spoor data.xlsx --sheet Sheet1 --limit 50 --offset 100
";

#[derive(Parser, Debug)]
#[command(
    name = "spoor",
    version,
    about = "将文档（DOCX/PDF）、表格（XLSX/CSV）、网页和幻灯片（PPTX）转成 LLM 可直接消费的文本",
    long_about = None,
    override_usage = "spoor [OPTIONS] <input>...",
    help_template = HELP_TEMPLATE,
    disable_help_flag = true,
    disable_version_flag = true
)]
pub(crate) struct Cli {
    /// 文件路径、URL、glob，或 - 表示 stdin。可传多个。URL 与 - 不参与 glob 展开。
    #[arg(value_name = "input", required = true, num_args = 1..)]
    pub(crate) inputs: Vec<String>,

    /// 手动指定输入格式。默认自动检测。
    #[arg(long, value_enum, value_name = "format", hide_possible_values = true)]
    pub(crate) format: Option<FormatArg>,

    /// 输出模式。表格默认 json、文档默认 md；json 仅表格可用。
    #[arg(
        long,
        short = 'm',
        value_enum,
        value_name = "mode",
        hide_possible_values = true,
        hide_default_value = true
    )]
    pub(crate) mode: Option<ModeArg>,

    /// 仅提取 PDF 指定页，如 `1:3`。
    #[arg(long, value_name = "first:last")]
    pub(crate) pages: Option<String>,

    /// 保留 PDF 跨页重复的页眉/页脚（默认去重，仅保留首次出现）。
    #[arg(long)]
    pub(crate) keep_repeated_regions: bool,

    /// 指定 XLSX 工作表名。CSV 忽略此选项。
    #[arg(long, value_name = "name")]
    pub(crate) sheet: Option<String>,

    /// 指定行号范围，如 `5:104`。与 --limit/--offset 互斥。
    #[arg(long, value_name = "first:last", conflicts_with_all = ["limit", "offset"])]
    pub(crate) rows: Option<String>,

    /// 按列名筛选，逗号分隔。
    #[arg(long, value_name = "columns", value_delimiter = ',')]
    pub(crate) columns: Vec<String>,

    /// 每表最多返回行数，默认 100。
    #[arg(long, value_name = "n")]
    pub(crate) limit: Option<usize>,

    /// 跳过前 N 行，默认 0。
    #[arg(long, value_name = "n")]
    pub(crate) offset: Option<usize>,

    /// stdout 上限，默认 256 KiB。
    #[arg(
        long,
        value_name = "kib",
        default_value_t = spoor_core::DEFAULT_MAX_OUTPUT_BYTES >> 10,
        hide_default_value = true
    )]
    pub(crate) max_output_kib: usize,

    /// 解析内存上限，默认 64 MiB。
    #[arg(
        long,
        value_name = "mib",
        default_value_t = spoor_core::DEFAULT_MAX_PARSE_BYTES >> 20,
        hide_default_value = true
    )]
    pub(crate) max_parse_mib: usize,

    /// 运算量上限，默认不限。
    #[arg(long, value_name = "n")]
    pub(crate) max_work_units: Option<usize>,

    /// 输出原文定位映射。当前支持 page（PDF 页级）。仅限单文件输入。
    /// 输出为 JSON，包含 markdown 与 provenance。
    #[arg(
        long,
        value_enum,
        value_name = "level",
        conflicts_with = "mode",
        hide_possible_values = true
    )]
    pub(crate) provenance: Option<ProvenanceArg>,

    /// 提取内嵌媒体到 stdout。接受 spoor://... URI。
    #[arg(
        long,
        value_name = "uri",
        conflicts_with_all = [
            "format",
            "mode",
            "pages",
            "sheet",
            "rows",
            "columns",
            "limit",
            "offset",
            "max_output_kib",
            "provenance"
        ]
    )]
    pub(crate) extract: Option<String>,

    /// 显示帮助。
    #[arg(short = 'h', long = "help", action = ArgAction::Help)]
    help: Option<bool>,

    /// 显示版本。
    #[arg(short = 'V', long = "version", action = ArgAction::Version)]
    version: Option<bool>,
}

impl Cli {
    /// `--max-parse-mib` 是面向用户的 MiB 单位；core 仍按字节计，这里换算并防溢出。
    pub(crate) fn max_parse_bytes(&self) -> usize {
        self.max_parse_mib.saturating_mul(1024 * 1024)
    }

    /// `--max-output-kib` 是面向用户的 KiB 单位；换算成字节交给 core。
    pub(crate) fn max_output_bytes(&self) -> usize {
        self.max_output_kib.saturating_mul(1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_without_flags_still_parses() {
        let cli = Cli::try_parse_from(["spoor", "report.pdf"]).unwrap();

        assert_eq!(cli.inputs, ["report.pdf"]);
        assert!(cli.mode.is_none());
        assert!(cli.format.is_none());
        assert!(cli.sheet.is_none());
        assert!(cli.pages.is_none());
        assert!(cli.rows.is_none());
        assert!(cli.columns.is_empty());
        assert!(cli.limit.is_none());
        assert!(cli.offset.is_none());
        assert_eq!(
            cli.max_output_kib,
            spoor_core::DEFAULT_MAX_OUTPUT_BYTES >> 10
        );
        assert_eq!(cli.max_parse_mib, spoor_core::DEFAULT_MAX_PARSE_BYTES >> 20);
        assert!(cli.extract.is_none());
    }

    #[test]
    fn multiple_inputs_parse() {
        let cli = Cli::try_parse_from(["spoor", "a.pdf", "b.pdf"]).unwrap();

        assert_eq!(cli.inputs, ["a.pdf", "b.pdf"]);
    }

    #[test]
    fn explicit_mode_parses() {
        let cli = Cli::try_parse_from(["spoor", "-m", "json", "data.csv"]).unwrap();
        assert_eq!(cli.mode, Some(ModeArg::Json));

        let cli = Cli::try_parse_from(["spoor", "-m", "md", "data.xlsx"]).unwrap();
        assert_eq!(cli.mode, Some(ModeArg::Md));
    }

    #[test]
    fn narrowing_flags_parse() {
        let cli = Cli::try_parse_from([
            "spoor",
            "data.xlsx",
            "--sheet",
            "L1",
            "--pages",
            "1:3",
            "--rows",
            "5:104",
            "--columns",
            "分类,技能",
        ])
        .unwrap();

        assert_eq!(cli.sheet, Some("L1".to_string()));
        assert_eq!(cli.pages, Some("1:3".to_string()));
        assert_eq!(cli.rows, Some("5:104".to_string()));
        assert_eq!(cli.columns, vec!["分类".to_string(), "技能".to_string()]);
    }

    #[test]
    fn rows_conflicts_with_limit() {
        let err = Cli::try_parse_from(["spoor", "data.xlsx", "--rows", "5:104", "--limit", "10"])
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn rows_conflicts_with_offset() {
        let err = Cli::try_parse_from(["spoor", "data.xlsx", "--rows", "5:104", "--offset", "2"])
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn extract_parses_and_conflicts_with_text_output_options() {
        let cli = Cli::try_parse_from([
            "spoor",
            "document.docx",
            "--extract",
            "spoor://docx/part/word/media/image1.png",
        ])
        .unwrap();
        assert_eq!(
            cli.extract.as_deref(),
            Some("spoor://docx/part/word/media/image1.png")
        );

        let err = Cli::try_parse_from([
            "spoor",
            "document.docx",
            "--extract",
            "spoor://docx/part/word/media/image1.png",
            "--mode",
            "md",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn help_shows_common_patterns_and_all_options() {
        let err = Cli::try_parse_from(["spoor", "-h"]).unwrap_err();

        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let help = err.to_string();
        assert!(help.contains(
            "将文档（DOCX/PDF）、表格（XLSX/CSV）、网页和幻灯片（PPTX）转成 LLM 可直接消费的文本"
        ));
        assert!(help.contains("Usage:"));
        assert!(help.contains("spoor [OPTIONS] <input>..."));
        assert!(help.contains("Arguments:"));
        assert!(help.contains("Options:"));
        assert!(help.contains("--format <format>"));
        assert!(help.contains("--mode <mode>"));
        assert!(help.contains("--sheet <name>"));
        assert!(help.contains("--pages <first:last>"));
        assert!(help.contains("--rows <first:last>"));
        assert!(help.contains("--columns <columns>"));
        assert!(help.contains("--limit <n>"));
        assert!(help.contains("--offset <n>"));
        assert!(help.contains("--max-output-kib <kib>"));
        assert!(help.contains("--max-parse-mib <mib>"));
        assert!(help.contains("--extract <uri>"));
        assert!(help.contains("Common Patterns"));
        assert!(help.contains("Defaults"));
        assert!(help.contains("Examples"));
        assert!(help.contains("spoor \"*.pdf\""));
        assert!(help.contains("spoor report.pdf --pages 1:3"));
        assert!(help.contains("总结"));
        assert!(help.contains("--sheet Sheet1 --rows 5:104"));
        assert!(help.contains("256 KiB"));
        assert!(help.contains("显示帮助。"));
    }

    /// The JSON envelope's `usage` hint must stay in sync with the real CLI
    /// flags. We derive the narrowing flags from the clap definition (every
    /// long flag except the non-narrowing ones) and require each to appear in
    /// `TABLE_USAGE`, so renaming or adding a narrowing flag without updating
    /// the hint fails CI instead of silently lying to consumers.
    #[test]
    fn table_usage_lists_every_narrowing_flag() {
        use clap::CommandFactory;

        let not_narrowing = [
            "format",
            "mode",
            "pages",
            "keep-repeated-regions",
            "max-output-kib",
            "max-parse-mib",
            "max-work-units",
            "provenance",
            "extract",
            "help",
            "version",
        ];

        for arg in Cli::command().get_arguments() {
            let Some(long) = arg.get_long() else {
                continue;
            };
            if not_narrowing.contains(&long) {
                continue;
            }
            assert!(
                spoor_core::TABLE_USAGE.contains(&format!("--{long}")),
                "narrowing flag --{long} is missing from TABLE_USAGE; the JSON \
                 `usage` hint would no longer match the real CLI"
            );
        }
    }
}
