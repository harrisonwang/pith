use spoor_core::{
    DocumentFilter, ErrorCode, Format, ParseContent, ParseLimits, ParseRequest, TableFilter,
    detect_format, extract_media, parse,
};
#[cfg(feature = "pdf")]
use spoor_core::{WarningCode, WarningLocation, parse_document_result};

#[test]
fn bytes_only_document_api_returns_typed_result() {
    let mut request = ParseRequest::new(b"hello from core\n");
    request.source_name = Some("note.txt");

    assert_eq!(detect_format(&request).unwrap(), Format::PlainText);
    let result = parse(&request).unwrap();
    assert_eq!(result.stats.input_bytes, 16);
    match result.content {
        ParseContent::Document(document) => {
            assert_eq!(document.source, "note.txt");
            assert_eq!(document.markdown, "hello from core\n");
        }
        ParseContent::Tables(_) => panic!("expected document result"),
    }
}

#[test]
#[cfg(feature = "tables")]
fn bytes_only_table_api_returns_native_tables() {
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/csv/01_basic.csv");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("data.csv");

    let result = parse(&request).unwrap();
    match result.content {
        ParseContent::Tables(tables) => {
            assert_eq!(tables.tables.len(), 1);
            assert_eq!(tables.tables[0].format, "csv");
        }
        ParseContent::Document(_) => panic!("expected table result"),
    }
}

#[test]
#[cfg(feature = "tables")]
fn table_filter_narrows_rows_and_columns_through_parse() {
    // 01_basic.csv has 3 data rows: Alice(row 2), Bob(row 3), Carol(row 4),
    // columns Name/Score/Note. The filter all bindings now set must flow
    // through `parse()` and select the same slice the CLI's flags do.
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/csv/01_basic.csv");

    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("data.csv");
    request.table_filter =
        TableFilter::build(None, None, vec!["Name".to_string()], Some(1), Some(1)).unwrap();

    let ParseContent::Tables(tables) = parse(&request).unwrap().content else {
        panic!("expected table result");
    };
    let rows = &tables.tables[0].rows;
    assert_eq!(rows.len(), 1, "offset 1 + limit 1 keeps a single row");
    assert_eq!(rows[0]["Name"], "Bob");
    assert!(
        !rows[0].contains_key("Score"),
        "column filter drops unselected fields"
    );

    // Excel-style row range selects the same row by its 1-based number.
    let mut ranged = ParseRequest::new(bytes);
    ranged.source_name = Some("data.csv");
    ranged.table_filter = TableFilter::build(None, Some((3, 3)), Vec::new(), None, None).unwrap();
    let ParseContent::Tables(tables) = parse(&ranged).unwrap().content else {
        panic!("expected table result");
    };
    assert_eq!(tables.tables[0].rows.len(), 1);
    assert_eq!(tables.tables[0].rows[0]["Name"], "Bob");
}

#[test]
#[cfg(feature = "pdf")]
fn pdf_stats_report_total_page_count_even_when_sliced() {
    // 02_multipage.pdf has 3 pages. A one-page peek must still report the full
    // count, so a caller can learn the document size cheaply, then widen --pages.
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/pdf/02_multipage.pdf");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("doc.pdf");
    request.document_filter = DocumentFilter {
        page_range: Some((1, 1)),
        ..DocumentFilter::default()
    };

    let result = parse(&request).unwrap();
    assert_eq!(result.stats.page_count, Some(3));
}

#[test]
#[cfg(feature = "pdf")]
fn work_budget_aborts_parse_with_stable_error() {
    // A tiny work budget exhausts during PDF content-stream processing and
    // surfaces a stable, branchable error rather than running unbounded.
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/pdf/02_multipage.pdf");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("doc.pdf");
    request.limits.max_work_units = Some(1);

    let error = parse(&request).unwrap_err();
    assert_eq!(error.code, ErrorCode::WorkBudgetExceeded);

    // The same input parses fine without a budget — the abort is the budget,
    // not the document.
    let mut ok = ParseRequest::new(bytes);
    ok.source_name = Some("doc.pdf");
    assert!(parse(&ok).is_ok());
}

#[test]
fn non_paged_formats_report_no_page_count() {
    let mut request = ParseRequest::new(b"hello\n");
    request.source_name = Some("note.txt");
    assert_eq!(parse(&request).unwrap().stats.page_count, None);
}

#[test]
fn public_boundary_normalizes_unstructured_parser_errors() {
    let mut request = ParseRequest::new(br#"{"not":"a notebook"}"#);
    request.source_name = Some("bad.ipynb");
    request.format_hint = Some(Format::Ipynb);

    let error = parse(&request).unwrap_err();
    assert_eq!(error.code, ErrorCode::ParseFailed);
    assert_eq!(error.stage, Some(spoor_core::ParseStage::Parse));
}

#[test]
fn parse_budget_is_enforced_before_detection() {
    let mut request = ParseRequest::new(&[b'x'; 2048]);
    request.limits = ParseLimits {
        max_parse_bytes: 1024,
        max_work_units: None,
    };

    let error = parse(&request).unwrap_err();
    assert_eq!(error.code, ErrorCode::ParseBudgetExceeded);
}

#[test]
#[cfg(feature = "office")]
fn extract_media_uses_safe_format_specific_resource_uris() {
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/docx/16_image_placeholders.docx");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("images.docx");

    let image = extract_media(&request, "spoor://docx/part/word/media/image1.png").unwrap();
    assert_eq!(image, b"first-image");

    // The retired per-format scheme must no longer resolve.
    let error = extract_media(&request, "spoor-docx://word/media/image1.png").unwrap_err();
    assert_eq!(error.code, ErrorCode::ParseFailed);

    // Bare paths without the scheme are still rejected.
    let error = extract_media(&request, "word/media/image1.png").unwrap_err();
    assert_eq!(error.code, ErrorCode::ParseFailed);

    // Cross-container forgery: a DOCX is fed a PPTX-shaped URI.
    let error = extract_media(&request, "spoor://pptx/part/ppt/media/image1.png").unwrap_err();
    assert_eq!(error.code, ErrorCode::ParseFailed);

    // Same scheme but wrong opc-root for the detected format is rejected.
    let error = extract_media(&request, "spoor://docx/part/ppt/media/image1.png").unwrap_err();
    assert_eq!(error.code, ErrorCode::ParseFailed);
}

#[test]
#[cfg(feature = "pdf")]
fn document_result_api_preserves_structured_warning_locations() {
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/pdf/05_mixed_text_and_image.pdf");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("mixed.pdf");

    let result = parse_document_result(&request).unwrap();

    // Page 2 lacks a text layer and carries an image: a missing-text warning
    // followed by an embedded-visual warning, both page-located.
    assert_eq!(result.warnings.len(), 2);
    assert_eq!(result.warnings[0].code, WarningCode::PdfPageNoTextLayer);
    assert_eq!(result.warnings[1].code, WarningCode::EmbeddedVisualsOmitted);
    for warning in &result.warnings {
        assert_eq!(warning.location, Some(WarningLocation::Page { number: 2 }));
    }
    let serialized = serde_json::to_value(result).unwrap();
    assert_eq!(serialized["warnings"][0]["location"]["kind"], "page");
    assert_eq!(serialized["warnings"][0]["location"]["number"], 2);
}

#[test]
#[cfg(feature = "pdf")]
fn provenance_is_off_by_default_and_absent_from_the_wire() {
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/pdf/02_multipage.pdf");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("doc.pdf");

    let result = parse(&request).unwrap();
    assert!(result.provenance.is_none(), "default must not compute it");
    // Omitted from the serialized form entirely, so existing consumers see no
    // change in the JSON shape.
    let serialized = serde_json::to_value(&result).unwrap();
    assert!(serialized.get("provenance").is_none());
}

#[test]
#[cfg(feature = "pdf")]
fn page_provenance_maps_output_ranges_back_to_source_pages() {
    use spoor_core::{ProvenanceLevel, SourceAnchor};
    // 02_multipage.pdf has 3 pages; page-level provenance yields one span per
    // page, ordered, each output byte range covering that page's `## Page N`
    // block so a quote landing in it maps back to the right source page.
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/pdf/02_multipage.pdf");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("doc.pdf");
    request.provenance = ProvenanceLevel::Page;

    let result = parse(&request).unwrap();
    let ParseContent::Document(document) = &result.content else {
        panic!("expected document result");
    };
    let provenance = result.provenance.as_ref().expect("provenance requested");
    assert_eq!(provenance.spans.len(), 3);

    let mut previous_end = 0;
    for (index, span) in provenance.spans.iter().enumerate() {
        let number = index + 1;
        assert_eq!(span.source, SourceAnchor::Page { number });
        // Ordered and non-overlapping.
        assert!(span.output.start >= previous_end);
        assert!(span.output.end > span.output.start);
        previous_end = span.output.end;
        // The mapped slice is exactly this page's block.
        let slice = &document.markdown[span.output.start..span.output.end];
        assert!(slice.starts_with(&format!("## Page {number}")), "{slice:?}");
    }
    assert!(provenance.spans.last().unwrap().output.end <= document.markdown.len());
}

#[test]
#[cfg(feature = "pdf")]
fn page_provenance_follows_the_page_slice() {
    use spoor_core::{ProvenanceLevel, SourceAnchor};
    // With a 2:2 slice only page 2 is rendered, so provenance has a single span
    // still anchored to source page 2 (numbers track the source, not position).
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/pdf/02_multipage.pdf");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("doc.pdf");
    request.provenance = ProvenanceLevel::Page;
    request.document_filter = DocumentFilter {
        page_range: Some((2, 2)),
        ..DocumentFilter::default()
    };

    let result = parse(&request).unwrap();
    let provenance = result.provenance.as_ref().expect("provenance requested");
    assert_eq!(provenance.spans.len(), 1);
    assert_eq!(provenance.spans[0].source, SourceAnchor::Page { number: 2 });
}

#[test]
fn page_provenance_is_empty_for_non_paged_formats() {
    use spoor_core::ProvenanceLevel;
    // Requesting page provenance on a format with no page model yields no
    // mapping rather than a bogus one.
    let mut request = ParseRequest::new(b"hello\n");
    request.source_name = Some("note.txt");
    request.provenance = ProvenanceLevel::Page;
    assert!(parse(&request).unwrap().provenance.is_none());
}

#[test]
#[cfg(feature = "pdf")]
fn locate_quote_grounds_a_citation_in_parsed_output() {
    use spoor_core::{LocateMethod, locate_quote};
    // End to end: parse a real PDF, then ground quotes in the produced
    // Markdown the way an answer-verification layer would.
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/pdf/02_multipage.pdf");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("doc.pdf");
    let result = parse(&request).unwrap();
    let ParseContent::Document(document) = result.content else {
        panic!("expected document output");
    };

    // A quote lifted verbatim from page 2 lands on page 2.
    let verbatim = document
        .markdown
        .lines()
        .skip_while(|line| !line.starts_with("## Page 2"))
        .find(|line| !line.starts_with('#') && !line.trim().is_empty())
        .expect("page 2 has body text");
    let found = locate_quote(&document.markdown, verbatim).expect("verbatim quote located");
    assert_eq!(found.method, LocateMethod::Exact);
    assert_eq!(found.page, Some(2));
    assert_eq!(
        &document.markdown[found.span.start..found.span.end],
        verbatim
    );

    // A fabricated quote is not located; the claim it backs is unverifiable.
    assert!(locate_quote(&document.markdown, "这句话不在文档里").is_none());
}
