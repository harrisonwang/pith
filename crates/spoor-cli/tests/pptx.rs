//! PPTX integration tests.

mod common;
use common::{extract_fixture, parse_fixture};
use insta::assert_snapshot;
use spoor_core::{Format, WarningCode, WarningLocation};

#[test]
fn basic_slides_with_titles_and_bullets() {
    // Each slide → "## Slide N" header.
    // Title and body text both extracted from <a:t> nodes.
    // <a:p> within a text frame separates text into lines.
    let out = extract_fixture("pptx/01_basic.pptx", Format::Pptx);
    assert_snapshot!(out);
}

#[test]
fn tables_in_slides() {
    // IMPORTANT: extract-text flattens tables in pptx — it just emits each
    // cell on its own line, no GFM table structure. This is a regression
    // from its docx behavior.
    //
    // We CHOOSE to emit GFM tables for pptx as well, matching docx.
    // It's a few extra lines of code and substantially more useful output.
    let out = extract_fixture("pptx/02_with_table.pptx", Format::Pptx);
    assert_snapshot!(out);
}

#[test]
fn speaker_notes_are_included() {
    // IMPORTANT: extract-text deliberately ignores ppt/notesSlides/*.xml.
    // Speaker notes often contain critical context (talking points, rationale,
    // citations) that are *more* valuable to an LLM than the slide bullets.
    //
    // We CHOOSE to include them, rendered under a "Notes:" sub-section.
    let out = extract_fixture("pptx/03_with_notes.pptx", Format::Pptx);
    assert_snapshot!(out);
}

#[test]
fn empty_deck_with_blank_slide() {
    let out = extract_fixture("pptx/04_empty.pptx", Format::Pptx);
    assert_snapshot!(out);
}

#[test]
fn slide_ordering_handles_double_digits() {
    // slide11.xml must come after slide2.xml. extract-text gets this
    // right by parsing the trailing digits and sorting numerically.
    // Test verifies this for slides 1..12.
    let out = extract_fixture("pptx/05_ordering.pptx", Format::Pptx);
    assert_snapshot!(out);
}

#[test]
fn slides_follow_presentation_order_not_filename_order() {
    // 09_reordered.pptx: parts are slide1=Alpha, slide2=Beta, slide3=Gamma,
    // but sldIdLst plays Gamma first. Slide numbers are 1-based positions in
    // deck order — the numbers PowerPoint shows — so the output must read
    // Slide 1: Gamma, Slide 2: Alpha, Slide 3: Beta.
    let out = extract_fixture("pptx/09_reordered.pptx", Format::Pptx);
    assert_snapshot!(out);
}

#[test]
fn hidden_slides_keep_their_number_and_surface_a_warning() {
    // Slide 2 ("Secret draft") carries show="0": the author retracted it from
    // the show, so its body is omitted — an agent must not cite content the
    // audience never saw — while the heading keeps its position so numbering
    // stays aligned with PowerPoint, warnings and anchors.
    let out = extract_fixture("pptx/10_hidden_slide.pptx", Format::Pptx);
    assert_snapshot!(out);
    assert!(!out.contains("Secret draft"));
    assert!(out.contains("## Slide 2"));
    assert!(out.contains("Visible three"));

    let parsed = parse_fixture("pptx/10_hidden_slide.pptx", Format::Pptx);
    let hidden: Vec<_> = parsed
        .warnings
        .iter()
        .filter(|w| w.code == WarningCode::HiddenSlideOmitted)
        .collect();
    assert_eq!(hidden.len(), 1);
    assert_eq!(
        hidden[0].location,
        Some(WarningLocation::Slide { number: 2 })
    );
}

#[test]
fn image_only_slides_carry_the_no_text_layer_posture() {
    // slide_no_text_layer is the "you got nothing" signal — VLM is mandatory,
    // not an enrichment — and its wording tells the agent whether speaker
    // notes still carried text out (slide 1 has notes, slide 2 does not).
    // The text control slide (3) draws no posture warning. Slide 4 is the
    // real-world pure-image shape (title + full-bleed screenshot): the title
    // is a label, not a text layer, so the posture fires there too — this is
    // what makes 纯图片页 distinguishable from 文字+图片页.
    let parsed = parse_fixture("pptx/11_image_only.pptx", Format::Pptx);
    let posture: Vec<_> = parsed
        .warnings
        .iter()
        .filter(|w| w.code == WarningCode::SlideNoTextLayer)
        .collect();
    assert_eq!(posture.len(), 3);
    assert_eq!(
        posture[0].location,
        Some(WarningLocation::Slide { number: 1 })
    );
    assert!(posture[0].message.contains("演讲者备注已提取"));
    assert_eq!(
        posture[1].location,
        Some(WarningLocation::Slide { number: 2 })
    );
    assert!(posture[1].message.contains("不经外部 VLM"));
    assert_eq!(
        posture[2].location,
        Some(WarningLocation::Slide { number: 4 })
    );
    // The recovery-handle warning still rides alongside for all image slides.
    let visuals = parsed
        .warnings
        .iter()
        .filter(|w| w.code == WarningCode::EmbeddedVisualsOmitted)
        .count();
    assert_eq!(visuals, 3);
}

#[test]
fn reading_order_follows_geometry_not_z_order() {
    // 12_reading_order.pptx adds text boxes bottom-first, so XML (z-) order
    // is Bottom row, Top right, Top left. The rendered order must be visual:
    // top-to-bottom, then left-to-right.
    let out = extract_fixture("pptx/12_reading_order.pptx", Format::Pptx);
    assert_snapshot!(out);
    let top_left = out.find("Top left").expect("top left");
    let top_right = out.find("Top right").expect("top right");
    let bottom = out.find("Bottom row").expect("bottom");
    assert!(top_left < top_right && top_right < bottom);
}

#[test]
fn bullet_levels_numbering_and_opt_out_are_preserved() {
    // Content-placeholder paragraphs are bulleted by PowerPoint's template
    // semantics; `lvl` nests them, buAutoNum numbers them, buNone opts out.
    let out = extract_fixture("pptx/13_bullets.pptx", Format::Pptx);
    assert_snapshot!(out);
    assert!(out.contains("- Level zero"));
    assert!(out.contains("    - Level one"));
    assert!(out.contains("        - Level two"));
    assert!(out.contains("1. Numbered item"));
    assert!(!out.contains("- No bullet here"));
    assert!(out.contains("No bullet here"));
}

#[test]
fn author_alt_text_rides_on_the_placeholder_sanitized() {
    // cNvPr@descr is author-provided image description — free routing signal
    // for the agent. Link-breaking characters must be neutralized so a
    // crafted descr cannot escape the `![alt](spoor://…)` syntax.
    let out = extract_fixture("pptx/14_alt_text.pptx", Format::Pptx);
    assert_snapshot!(out);
    assert!(out.contains("![PPTX image 1 (slide 1): Quarterly revenue chart evil x](spoor://pptx/part/ppt/media/image1.png)"));
    assert!(!out.contains("](evil)"));
}

#[test]
fn group_shapes_flatten_in_visual_order() {
    let out = extract_fixture("pptx/15_group_shapes.pptx", Format::Pptx);
    assert_snapshot!(out);
    let alpha = out.find("Grouped alpha").expect("alpha");
    let beta = out.find("Grouped beta").expect("beta");
    let after = out.find("After the group").expect("after");
    assert!(alpha < beta && beta < after);
}

#[test]
fn notes_page_number_furniture_does_not_leak() {
    // The notes slide carries a sldNum placeholder with the digit "12";
    // furniture placeholders are template chrome and must be filtered.
    let out = extract_fixture("pptx/16_notes_furniture.pptx", Format::Pptx);
    assert_snapshot!(out);
    assert!(out.contains("Real speaker notes."));
    assert!(!out.lines().any(|line| line.trim() == "12"));
}

#[test]
fn missing_presentation_part_falls_back_to_filename_order() {
    // 17_no_presentation.pptx is 09_reordered.pptx with presentation.xml
    // stripped: without a parseable deck order the parser degrades to
    // numeric filename order (slide1=Alpha, slide2=Beta, slide3=Gamma)
    // deterministically instead of erroring.
    let out = extract_fixture("pptx/17_no_presentation.pptx", Format::Pptx);
    assert_snapshot!(out);
    let alpha = out.find("Alpha").expect("alpha");
    let beta = out.find("Beta").expect("beta");
    let gamma = out.find("Gamma").expect("gamma");
    assert!(alpha < beta && beta < gamma);
}

#[test]
fn chart_data_is_extracted_as_a_table_with_no_incompleteness_warning() {
    // The numbers business decks put nowhere else: cached c:cat/c:val series
    // come out as a labeled GFM table at the chart's position. A fully
    // unpacked chart slide is complete — no embedded_visuals_omitted, and no
    // slide_no_text_layer (the agent got the data).
    let out = extract_fixture("pptx/18_chart.pptx", Format::Pptx);
    assert_snapshot!(out);
    assert!(out.contains("Chart: Quarterly performance"));
    assert!(out.contains("| Q2 | 120 | 90 |"));

    let parsed = parse_fixture("pptx/18_chart.pptx", Format::Pptx);
    assert!(
        parsed.warnings.is_empty(),
        "unpacked chart must not warn: {:?}",
        parsed.warnings
    );
}

#[test]
fn smartart_node_text_is_extracted_as_a_list() {
    // SmartArt text lives in ppt/diagrams/dataN.xml (dgm:t), which the whole
    // python-pptx tool family drops. Node labels come out in data-model
    // order; graph structure is deliberately not reconstructed.
    let out = extract_fixture("pptx/19_smartart.pptx", Format::Pptx);
    assert_snapshot!(out);
    assert!(out.contains("- Plan"));
    assert!(out.contains("- Build"));
    assert!(out.contains("- Ship"));

    let parsed = parse_fixture("pptx/19_smartart.pptx", Format::Pptx);
    assert!(
        parsed.warnings.is_empty(),
        "unpacked SmartArt must not warn: {:?}",
        parsed.warnings
    );
}

#[test]
fn merged_table_and_visual_omissions_are_located_by_slide() {
    let merged = parse_fixture("pptx/06_merged_table.pptx", Format::Pptx);
    let visual = parse_fixture("pptx/07_embedded_visual.pptx", Format::Pptx);

    assert_eq!(
        merged.warnings[0].code,
        WarningCode::MergedTableStructureNotPreserved
    );
    assert_eq!(
        merged.warnings[0].location,
        Some(WarningLocation::Slide { number: 1 })
    );
    // 07 is a title + picture slide — the real-world pure-image shape — so
    // the no-text-layer posture rides in front of the recovery-handle
    // warning; both locate to slide 1.
    assert_eq!(visual.warnings[0].code, WarningCode::SlideNoTextLayer);
    let handles = visual
        .warnings
        .iter()
        .find(|w| w.code == WarningCode::EmbeddedVisualsOmitted)
        .expect("recovery-handle warning still present");
    assert_eq!(handles.location, Some(WarningLocation::Slide { number: 1 }));
}

#[test]
fn image_placeholders_follow_slide_order_and_only_reference_safe_entries() {
    let out = extract_fixture("pptx/08_image_placeholders.pptx", Format::Pptx);
    assert_snapshot!(out);

    // image_number runs across slides: 1 on slide 1, 2 + 3 on slide 2, none
    // on slide 3. python-pptx dedups by content hash, so slide 1 and slide 2's
    // first image share `ppt/media/image1.png` — verifies that the same OPC
    // part referenced from two slides still gets distinct image numbers.
    // (": image.png" is python-pptx's default cNvPr@descr — the filename —
    // riding on the placeholder as sanitized alt text.)
    assert_eq!(
        out.matches("![PPTX image 1 (slide 1): image.png](spoor://pptx/part/ppt/media/image1.png)")
            .count(),
        1
    );
    assert_eq!(
        out.matches("![PPTX image 2 (slide 2): image.png](spoor://pptx/part/ppt/media/image1.png)")
            .count(),
        1
    );
    assert_eq!(
        out.matches("![PPTX image 3 (slide 2): image.png](spoor://pptx/part/ppt/media/image2.png)")
            .count(),
        1
    );
    // Slide 3 has no images: no `PPTX image 4` placeholder is emitted.
    assert!(!out.contains("PPTX image 4"));

    // Every emitted handle uses the unified scheme; nothing escapes the
    // `spoor://pptx/part/ppt/media/` sandbox.
    let total = out.matches("spoor://pptx/part/ppt/media/").count();
    assert_eq!(total, 3);
    assert!(!out.contains("spoor-pptx://"));
}

#[test]
fn slide_with_images_carries_extract_wording_in_warning() {
    let parsed = parse_fixture("pptx/08_image_placeholders.pptx", Format::Pptx);
    // Slide 1 and 2 carry visuals; slide 3 does not.
    let visual_warnings: Vec<_> = parsed
        .warnings
        .iter()
        .filter(|w| w.code == WarningCode::EmbeddedVisualsOmitted)
        .collect();
    assert_eq!(visual_warnings.len(), 2);
    for warning in visual_warnings {
        assert!(
            warning.message.contains("spoor://pptx/part/") && warning.message.contains("--extract"),
            "expected extract wording, got: {}",
            warning.message,
        );
    }
}
