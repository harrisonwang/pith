//! DOCX integration tests.
//!
//! Each test is annotated with what we expect, and where we deliberately
//! diverge from Anthropic's extract-text behavior.

mod common;
use common::{extract_fixture, parse_fixture};
use insta::assert_snapshot;
use spoor_core::{Format, WarningCode};
use std::fs::File;
use zip::ZipArchive;

#[test]
fn basic_headings_and_inline_formatting() {
    let out = extract_fixture("docx/01_basic.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn list_via_pstyle_only() {
    // Anthropic's extract-text DOES NOT recognize lists declared via
    // pStyle="ListBullet" / "ListNumber" — it only checks <w:numPr>.
    // Word generates pStyle-only lists in many real documents, so we
    // SHOULD recognize them. This test guards that improvement.
    let out = extract_fixture("docx/02c_lists_pstyle_only.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn list_via_real_numpr() {
    // Lists declared via <w:numPr><w:numId> — both extract-text and we
    // should render these. We additionally choose to indent nested levels
    // (extract-text flattens them).
    let out = extract_fixture("docx/02b_lists_numpr.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn tables_render_as_gfm() {
    // Tables are rendered as GFM with a header separator row. Cells
    // containing '|' must be safe for the markdown table parser.
    // extract-text replaces '|' with U+2502 (BOX DRAWINGS LIGHT VERTICAL).
    // We choose `\|` (proper markdown escape) instead — same visual rendering
    // in markdown viewers, but round-trips through markdown parsers correctly.
    let out = extract_fixture("docx/03_tables.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn hyperlinks_use_rels_lookup() {
    // Hyperlink text is wrapped in markdown `[text](url)`, with the URL
    // resolved by joining `r:id` against word/_rels/document.xml.rels.
    let out = extract_fixture("docx/04_hyperlinks.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn footnotes_collected_and_appended() {
    // Footnote markers `[^N]` are emitted inline; the bodies are collected
    // and appended at the end of the document, separated by a blank line.
    let out = extract_fixture("docx/05_footnotes.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn unicode_passthrough() {
    // CJK, RTL, math, emoji, smart quotes — all should be transparent
    // (no transcoding, no smart-quote conversion).
    let out = extract_fixture("docx/06_unicode.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn custom_namespace_prefix() {
    // OOXML files don't have to use `w:` as the prefix — they only have
    // to bind the WordprocessingML namespace URI. Our parser resolves by
    // namespace, not prefix. extract-text handles this; we must too.
    let out = extract_fixture("docx/07_custom_prefix.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn empty_document() {
    // Empty docs produce a single newline. extract-text returns empty
    // string + final newline appended by the binary; we do the same.
    let out = extract_fixture("docx/08_empty.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn whitespace_only_paragraphs_skipped() {
    // Paragraphs that are entirely whitespace are skipped (no blank
    // pseudo-paragraph emitted).
    let out = extract_fixture("docx/09_whitespace.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn heading_levels_one_through_six() {
    // Heading 1..6 → '#'..'######'. Anything beyond 6 should fall back
    // to plain paragraph (markdown spec only defines 6 levels).
    let out = extract_fixture("docx/10_heading_levels.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn xml_space_preserve_runs() {
    // Runs with `xml:space="preserve"` keep leading/trailing spaces.
    // Without the attribute, default XML whitespace handling applies.
    let out = extract_fixture("docx/11_whitespace_runs.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn formatted_whitespace_only_runs_no_panic() {
    // Bold space, italic w:br, hyperlink-only space: md mode keeps raw whitespace,
    // does not emit ** / * / []() around invisible runs (see docs/DESIGN_NOTES.md).
    let out = extract_fixture("docx/13_formatted_whitespace_only_runs.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn tracked_changes_accept_inserts_drop_deletes() {
    // We accept all tracked changes by default, matching extract-text.
    // <w:ins> contents are kept, <w:del> contents are dropped.
    // (A `--show-changes` flag could be added later.)
    let out = extract_fixture("docx/12_tracked_changes.docx", Format::Docx);
    assert_snapshot!(out);
}

#[test]
fn merged_table_and_visual_omissions_are_explicit() {
    let merged = parse_fixture("docx/14_merged_table.docx", Format::Docx);
    let visual = parse_fixture("docx/15_embedded_visual.docx", Format::Docx);

    assert_eq!(
        merged.warnings[0].code,
        WarningCode::MergedTableStructureNotPreserved
    );
    assert_eq!(visual.warnings[0].code, WarningCode::EmbeddedVisualsOmitted);
}

#[test]
fn image_placeholders_follow_document_order_and_only_reference_safe_entries() {
    let out = extract_fixture("docx/16_image_placeholders.docx", Format::Docx);
    assert_snapshot!(out);

    assert_eq!(
        out.matches("spoor://docx/part/word/media/image1.png")
            .count(),
        2
    );
    assert_eq!(
        out.matches("spoor://docx/part/word/media/image2.png")
            .count(),
        1
    );
    assert_eq!(
        out.matches("spoor://docx/part/word/media/fallback-only.png")
            .count(),
        1
    );
    assert!(!out.contains("Fallback duplicate"));
    assert!(!out.contains("word/media/fallback.png"));
    assert!(!out.contains("image-hd.png"));
    assert!(!out.contains("external.png"));
    assert!(!out.contains("evil.png"));

    let fixture = "tests/fixtures/docx/16_image_placeholders.docx";
    let mut archive = ZipArchive::new(File::open(fixture).unwrap()).unwrap();
    assert!(archive.by_name("word/media/image1.png").is_ok());
    assert!(archive.by_name("word/media/image2.png").is_ok());
    assert!(archive.by_name("word/media/fallback-only.png").is_ok());

    let result = parse_fixture("docx/16_image_placeholders.docx", Format::Docx);
    assert_eq!(result.warnings[0].code, WarningCode::EmbeddedVisualsOmitted);
}
