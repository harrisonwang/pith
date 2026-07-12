//! PDF integration tests.

mod common;
use common::{extract_fixture, extract_fixture_err, parse_fixture};
use insta::assert_snapshot;
use serde_json::json;
use spoor_core::{
    DocumentFilter, Format, ParseContent, ParseRequest, ProvenanceLevel, SourceAnchor, WarningCode,
    WarningLocation, parse_document, parse_document_result,
};

#[test]
fn basic_text_layer() {
    // Single page, plain text. pdf-extract gives us the text in
    // approximately reading order.
    let out = extract_fixture("pdf/01_basic.pdf", Format::Pdf);
    assert_snapshot!(out);
}

#[test]
fn multipage_has_page_boundaries() {
    let out = extract_fixture("pdf/02_multipage.pdf", Format::Pdf);
    assert_eq!(out.matches("## Page ").count(), 3);
    assert!(out.starts_with("## Page 1\n\nPage 1 content begins here."));
    assert!(out.contains("\n\n## Page 2\n\nPage 2 content begins here."));
    assert!(out.contains("\n\n## Page 3\n\nPage 3 content begins here."));
}

#[test]
fn ascii_baseline() {
    let out = extract_fixture("pdf/03_ascii_only.pdf", Format::Pdf);
    assert!(out.contains("ASCII only"));
}

#[test]
fn no_text_and_no_images_returns_structured_error() {
    // A vector-only page has no text layer and no images to hand off, so there
    // is genuinely nothing to extract — the structured error still fires.
    let error = extract_fixture_err("pdf/06_vector_only.pdf", Format::Pdf);
    let value: serde_json::Value = serde_json::from_str(&error).expect("structured JSON error");

    assert_eq!(
        value,
        json!({
            "is_error": true,
            "code": "pdf_no_extractable_content",
            "reason": "PDF 无可提取内容",
            "hint": "使用 VLM 处理。",
            "recoverable": true,
            "stage": "parse"
        })
    );
}

#[test]
fn image_only_pdf_is_surfaced_for_vision_instead_of_failing() {
    // A PDF with no text but with images must NOT hard-fail: it renders the page
    // skeleton plus image markers/handles so a vision-capable agent can read it.
    let markdown = extract_fixture("pdf/04_image_only.pdf", Format::Pdf);
    assert!(markdown.contains("## Page 1"), "{markdown}");
    assert!(markdown.contains("PDF image 1 (p1)"), "{markdown}");

    let result = parse_fixture("pdf/04_image_only.pdf", Format::Pdf);
    let codes: Vec<_> = result.warnings.iter().map(|warning| warning.code).collect();
    assert!(codes.contains(&WarningCode::PdfPageNoTextLayer));
    assert!(codes.contains(&WarningCode::EmbeddedVisualsOmitted));
}

#[test]
fn mixed_pdf_reports_page_level_missing_text_and_image() {
    let result = parse_fixture("pdf/05_mixed_text_and_image.pdf", Format::Pdf);

    // Page 2 has no text layer and carries an image, so it draws both a
    // missing-text warning and an embedded-visual warning, each page-located.
    assert_eq!(result.warnings.len(), 2);
    for warning in &result.warnings {
        assert_eq!(warning.location, Some(WarningLocation::Page { number: 2 }));
    }
    let codes: Vec<_> = result.warnings.iter().map(|warning| warning.code).collect();
    assert!(codes.contains(&WarningCode::PdfPageNoTextLayer));
    assert!(codes.contains(&WarningCode::EmbeddedVisualsOmitted));
}

#[test]
fn page_filter_limits_pdf_output_to_requested_pages() {
    let path = std::path::Path::new("tests/fixtures/pdf/02_multipage.pdf");
    let bytes = std::fs::read(path).expect("read fixture");
    let mut request = ParseRequest::new(&bytes);
    request.source_name = path.to_str();
    request.format_hint = Some(Format::Pdf);
    request.document_filter = DocumentFilter {
        page_range: Some((2, 2)),
        ..DocumentFilter::default()
    };

    let markdown = parse_document(&request)
        .expect("parse filtered PDF")
        .markdown;
    assert!(!markdown.contains("## Page 1"), "{markdown}");
    assert!(markdown.contains("## Page 2"), "{markdown}");
    assert!(!markdown.contains("## Page 3"), "{markdown}");
    assert!(
        markdown.contains("Page 2 content begins here."),
        "{markdown}"
    );
}

#[test]
fn uri_link_annotations_are_woven_into_markdown() {
    // 08_links.pdf carries three URI link annotations: one whose rect sits
    // exactly over "full guide", one javascript: action, and one over empty
    // page area. The anchored link wraps in place, the executable scheme is
    // dropped entirely, and the anchorless target survives as an autolink.
    let out = extract_fixture("pdf/08_links.pdf", Format::Pdf);
    assert!(
        out.contains("See the [full guide](https://example.com/guide) for details."),
        "anchored link must wrap its anchor in place:\n{out}"
    );
    assert!(
        !out.contains("javascript:"),
        "executable schemes must be dropped:\n{out}"
    );
    assert!(
        out.contains("Do not execute this."),
        "text under a dropped link keeps its plain form:\n{out}"
    );
    assert!(
        out.contains("<https://example.com/api>"),
        "anchorless target must survive as an autolink:\n{out}"
    );
}

#[test]
fn outline_titles_promote_matching_lines_to_headings() {
    // 09_outline.pdf carries a two-level outline: Introduction > Background on
    // page 1, Methods on page 2, plus a "Missing Section" entry whose title
    // appears nowhere. Outline depth maps below the `## Page N` blocks
    // (level 1 → ###), and an unmatched title must not fabricate a heading.
    let out = extract_fixture("pdf/09_outline.pdf", Format::Pdf);
    assert!(out.contains("\n### Introduction\n"), "{out}");
    assert!(out.contains("\n#### Background\n"), "{out}");
    assert!(out.contains("### Methods"), "{out}");
    assert!(
        !out.contains("Missing Section"),
        "an outline title absent from its page must not be fabricated:\n{out}"
    );
    // Prose lines stay prose.
    assert!(
        out.contains("Opening prose that follows the first heading."),
        "{out}"
    );
}

#[test]
fn repeated_headers_and_footers_deduplicate_with_warning() {
    // 10_header_footer.pdf repeats "ACME Corp Annual Report 2026" and
    // "Page N of 4" on all four pages. Dedup keeps each region's first
    // occurrence, removes the repeats, and names what moved in a stable
    // warning; body prose is untouched.
    let result = parse_fixture("pdf/10_header_footer.pdf", Format::Pdf);
    let ParseContent::Document(document) = &result.content else {
        panic!("expected document result");
    };
    let markdown = &document.markdown;

    assert_eq!(
        markdown.matches("ACME Corp Annual Report 2026").count(),
        1,
        "header must keep exactly its first occurrence:\n{markdown}"
    );
    assert!(markdown.contains("Page 1 of 4"), "{markdown}");
    assert!(!markdown.contains("Page 3 of 4"), "{markdown}");
    for body in [
        "Revenue grew steadily",
        "Costs were kept flat",
        "outlook section",
        "Appendix tables",
    ] {
        assert!(markdown.contains(body), "body prose must stay: {body}");
    }

    let furniture: Vec<_> = result
        .warnings
        .iter()
        .filter(|w| w.code == WarningCode::PdfRepeatedRegionDeduplicated)
        .collect();
    assert_eq!(furniture.len(), 2, "{:?}", result.warnings);
    assert!(furniture.iter().any(|w| w.message.contains("ACME Corp")));
}

#[test]
fn keep_repeated_regions_retains_verbatim_page_text() {
    let path = std::path::Path::new("tests/fixtures/pdf/10_header_footer.pdf");
    let bytes = std::fs::read(path).expect("read fixture");
    let mut request = ParseRequest::new(&bytes);
    request.source_name = path.to_str();
    request.format_hint = Some(Format::Pdf);
    request.document_filter = DocumentFilter {
        keep_repeated_regions: true,
        ..DocumentFilter::default()
    };

    let result = parse_document_result(&request).expect("parse with keep option");
    let ParseContent::Document(document) = &result.content else {
        panic!("expected document result");
    };

    assert_eq!(
        document
            .markdown
            .matches("ACME Corp Annual Report 2026")
            .count(),
        4,
        "keep option must retain every occurrence:\n{}",
        document.markdown
    );
    assert!(
        !result
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::PdfRepeatedRegionDeduplicated),
        "no dedup warning when nothing was removed"
    );
}

#[test]
fn line_end_hyphenation_rejoins_conservatively() {
    // 11_hyphenation.pdf breaks "dehyphenation" and "state-of-the-art" across
    // tightly-leaded lines, plus two guards that must stay split: a
    // free-standing minus and an uppercase acronym continuation.
    let out = extract_fixture("pdf/11_hyphenation.pdf", Format::Pdf);
    assert!(
        out.contains("conservative dehyphenation pass"),
        "broken word must rejoin without its hyphen:\n{out}"
    );
    assert!(
        out.contains("state-of-the-art extraction"),
        "compound broken at an inner hyphen must keep it:\n{out}"
    );
    assert!(
        out.contains("subtotal -\ndiscount"),
        "a free-standing minus is not a word break:\n{out}"
    );
    assert!(
        out.contains("UTF-\n8"),
        "digit/uppercase continuations stay split:\n{out}"
    );
}

#[test]
fn block_provenance_anchors_lines_with_boxes_and_refines_page_level() {
    let path = std::path::Path::new("tests/fixtures/pdf/09_outline.pdf");
    let bytes = std::fs::read(path).expect("read fixture");
    let mut request = ParseRequest::new(&bytes);
    request.source_name = path.to_str();
    request.format_hint = Some(Format::Pdf);
    request.provenance = ProvenanceLevel::Block;

    let result = parse_document_result(&request).expect("parse with block provenance");
    let ParseContent::Document(document) = &result.content else {
        panic!("expected document result");
    };
    let markdown = document.markdown.as_bytes();
    let spans = &result
        .provenance
        .as_ref()
        .expect("provenance present")
        .spans;

    // Ordered, non-overlapping, and every span page-anchored.
    let mut previous = 0usize;
    for span in spans.iter() {
        assert!(span.output.start >= previous, "spans must not overlap");
        assert!(span.output.end > span.output.start);
        previous = span.output.end;
        let SourceAnchor::Page { number, .. } = span.source;
        assert!((1..=2).contains(&number));
    }

    // The promoted heading line is anchored with a box that covers exactly
    // the line text (the "### " prefix stays in a page-anchored gap), and the
    // box is expressed in PDF-native user space (y-up, on a 792pt-high page).
    let heading = spans
        .iter()
        .find(|span| {
            std::str::from_utf8(&markdown[span.output.start..span.output.end])
                .is_ok_and(|text| text == "Introduction")
        })
        .expect("heading line span");
    let SourceAnchor::Page {
        number,
        bbox: Some(bbox),
    } = &heading.source
    else {
        panic!("heading line must carry a bbox: {:?}", heading.source);
    };
    assert_eq!(*number, 1);
    assert!((bbox.x0 - 72.0).abs() < 1.0, "x0 = {}", bbox.x0);
    assert!(
        bbox.y1 > bbox.y0 && bbox.y0 > 700.0 && bbox.y1 < 745.0,
        "{bbox:?}"
    );

    // Block level refines page level: some spans carry no box (headers,
    // separators) but still resolve to a page.
    assert!(
        spans
            .iter()
            .any(|span| matches!(span.source, SourceAnchor::Page { bbox: None, .. }))
    );
}

#[test]
fn two_column_pdf_is_read_left_column_then_right_with_warning() {
    // 07_two_column.pdf draws the two columns interleaved row-by-row in the
    // content stream, so flat extraction interleaves them. Geometric
    // reconstruction must emit the whole left column, then the whole right.
    let path = std::path::Path::new("tests/fixtures/pdf/07_two_column.pdf");
    let bytes = std::fs::read(path).expect("read fixture");
    let mut request = ParseRequest::new(&bytes);
    request.source_name = path.to_str();
    request.format_hint = Some(Format::Pdf);

    let result = parse_document_result(&request).expect("parse two-column PDF");
    let ParseContent::Document(document) = result.content else {
        panic!("expected document result");
    };
    let markdown = document.markdown;

    // The entire left column precedes the entire right column.
    let last_left = markdown
        .find("Left line four")
        .expect("left column present");
    let first_right = markdown
        .find("Right line one")
        .expect("right column present");
    assert!(
        last_left < first_right,
        "left column must be read before right column:\n{markdown}"
    );

    // The agent is told the page was reordered, located on page 1, so it can
    // fall back to raw order if needed.
    let warning = result
        .warnings
        .iter()
        .find(|w| w.code == WarningCode::PdfMultiColumnReadingOrder)
        .expect("multi-column warning");
    assert_eq!(warning.location, Some(WarningLocation::Page { number: 1 }));
}
