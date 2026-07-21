use spoor_core::parse_document_result;
use spoor_core::{
    DocumentFilter, ErrorCode, Format, ParseContent, ParseLimits, ParseRequest, ProvenanceLevel,
    SourceAnchor, TableFilter, detect_format, extract_media, parse,
};
#[cfg(feature = "pdf")]
use spoor_core::{WarningCode, WarningLocation};

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
        assert_eq!(span.source, SourceAnchor::Page { number, bbox: None });
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
    assert_eq!(
        provenance.spans[0].source,
        SourceAnchor::Page {
            number: 2,
            bbox: None
        }
    );
}

#[test]
fn page_provenance_maps_linear_formats_to_one_input_span() {
    // Linear formats have no page model; page level gives one coarse
    // whole-document span anchored at the input byte range instead.
    let mut request = ParseRequest::new(b"hello\n");
    request.source_name = Some("note.txt");
    request.provenance = ProvenanceLevel::Page;
    let spans = parse(&request)
        .unwrap()
        .provenance
        .expect("linear provenance")
        .spans;
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].source, SourceAnchor::Input { start: 0, end: 6 });
}

#[test]
fn slide_provenance_tiles_output_with_slide_anchors() {
    use spoor_core::SourceAnchor;
    // 01_basic.pptx has 2 slides. Page and Block levels both mean "one span
    // per slide": a slide is a small enough citation unit that no finer
    // geometry is needed. Spans start at each `## Slide N` heading and stay
    // in ascending, non-overlapping order.
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/pptx/01_basic.pptx");
    for level in [ProvenanceLevel::Page, ProvenanceLevel::Block] {
        let mut request = ParseRequest::new(bytes);
        request.source_name = Some("deck.pptx");
        request.provenance = level;
        let result = parse(&request).unwrap();
        let ParseContent::Document(document) = &result.content else {
            panic!("expected document output");
        };
        let spans = &result.provenance.as_ref().expect("slide provenance").spans;
        assert_eq!(spans.len(), 2);
        let mut previous_end = 0usize;
        for (index, span) in spans.iter().enumerate() {
            let number = index + 1;
            assert_eq!(span.source, SourceAnchor::Slide { number });
            assert!(span.output.start >= previous_end);
            let text = &document.markdown[span.output.start..span.output.end];
            assert!(
                text.starts_with(&format!("## Slide {number}")),
                "span must start at its heading, got {text:?}"
            );
            previous_end = span.output.end;
        }
        assert_eq!(result.stats.page_count, Some(2));
    }
}

#[test]
fn slide_narrowing_follows_source_numbers_and_reports_full_count() {
    use spoor_core::SourceAnchor;
    // 05_ordering.pptx has 12 slides. A 2:3 slice keeps source numbering
    // (`## Slide 2`, `## Slide 3`) and stats still report the full deck, the
    // same contract as PDF's --pages.
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/pptx/05_ordering.pptx");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("deck.pptx");
    request.document_filter = DocumentFilter::build(Some((2, 3))).unwrap();
    request.provenance = ProvenanceLevel::Page;
    let result = parse(&request).unwrap();
    let ParseContent::Document(document) = &result.content else {
        panic!("expected document output");
    };
    assert!(document.markdown.contains("## Slide 2"));
    assert!(document.markdown.contains("## Slide 3"));
    assert!(!document.markdown.contains("## Slide 1"));
    assert!(!document.markdown.contains("## Slide 4"));
    assert_eq!(result.stats.page_count, Some(12));
    let spans = result.provenance.expect("slide provenance").spans;
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].source, SourceAnchor::Slide { number: 2 });
    assert_eq!(spans[1].source, SourceAnchor::Slide { number: 3 });

    // A slice starting past the deck is a structured error, mirroring PDF.
    let mut over = ParseRequest::new(bytes);
    over.source_name = Some("deck.pptx");
    over.document_filter = DocumentFilter::build(Some((99, 100))).unwrap();
    assert_eq!(parse(&over).unwrap_err().code, ErrorCode::ParseFailed);
}

#[test]
fn locate_quote_grounds_a_pptx_citation_to_its_slide() {
    use spoor_core::SourceAnchor;
    // End to end: an agent quotes bullet text; grounded locate resolves it to
    // the slide it came from, mechanically.
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/pptx/01_basic.pptx");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("deck.pptx");
    request.provenance = ProvenanceLevel::Block;
    let result = parse(&request).unwrap();
    let ParseContent::Document(document) = &result.content else {
        panic!("expected document output");
    };
    let spans = &result.provenance.as_ref().expect("provenance").spans;
    let grounded = spoor_core::locate_quote_grounded(&document.markdown, "Second bullet", spans)
        .expect("verbatim bullet located");
    assert_eq!(grounded.anchor, Some(SourceAnchor::Slide { number: 2 }));
}

#[test]
fn page_provenance_is_empty_for_reflowable_formats() {
    // Reflowable documents (DOCX etc.) still have no mapping; requesting one
    // yields none rather than a bogus anchor. HTML has no linear identity.
    let html = b"<html><body><p>hi</p></body></html>";
    let mut request = ParseRequest::new(html);
    request.source_name = Some("page.html");
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

#[test]
fn linear_block_provenance_tiles_identity_output_with_input_anchors() {
    let bytes = "第一行\nsecond line\n最后".as_bytes();
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("note.txt");
    request.provenance = ProvenanceLevel::Block;

    let result = spoor_core::parse(&request).expect("parse text");
    let spans = &result.provenance.expect("provenance").spans;

    // Identity output: spans tile the whole output, each mapping to the same
    // input byte range.
    assert_eq!(spans.len(), 3);
    let mut cursor = 0usize;
    for span in spans.iter() {
        assert_eq!(span.output.start, cursor);
        let SourceAnchor::Input { start, end } = &span.source else {
            panic!("expected input anchor: {:?}", span.source);
        };
        assert_eq!((*start, *end), (span.output.start, span.output.end));
        cursor = span.output.end;
    }
    assert_eq!(cursor, bytes.len());
}

#[test]
fn csv_block_provenance_anchors_cells_and_grounds_quotes() {
    let bytes = include_bytes!("../../spoor-cli/tests/fixtures/csv/01_basic.csv");
    let mut request = ParseRequest::new(bytes);
    request.source_name = Some("data.csv");
    request.provenance = ProvenanceLevel::Block;

    let result = spoor_core::parse_document_result(&request).expect("parse csv as document");
    let ParseContent::Document(document) = &result.content else {
        panic!("expected document result");
    };
    let spans = &result.provenance.as_ref().expect("provenance").spans;
    assert!(!spans.is_empty());

    // Every anchor is a cell of the rendered table; slicing the markdown by
    // the span gives the escaped cell text.
    let cell = spans
        .iter()
        .find_map(|span| match &span.source {
            SourceAnchor::Cell { row, column, .. } if *row == 1 => Some((span, column.clone())),
            _ => None,
        })
        .expect("first data row cell");
    let hit = &document.markdown[cell.0.output.start..cell.0.output.end];
    assert!(!hit.trim().is_empty());

    // Grounded locate on a cell value returns its Cell anchor.
    let grounded =
        spoor_core::locate_quote_grounded(&document.markdown, hit, spans).expect("locate");
    assert!(matches!(grounded.anchor, Some(SourceAnchor::Cell { .. })));
    assert_eq!(
        match &grounded.anchor {
            Some(SourceAnchor::Cell { column, .. }) => column.clone(),
            _ => String::new(),
        },
        cell.1
    );
}
