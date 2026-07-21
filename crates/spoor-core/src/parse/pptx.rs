use crate::engine::{DocumentFilter, ProvenanceLevel};
use crate::error::{ParseStage, StructuredError};
use crate::limits;
use crate::output::MarkdownBuilder;
use crate::parse::ExtractedMarkdown;
use crate::parse::xml::attr;
use crate::result::{ProvenanceSpan, SourceAnchor, SpoorWarning, TextRange, WarningCode};
use crate::source::Source;
use anyhow::{Result, anyhow};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path};

pub fn extract(
    source: &Source<'_>,
    document_filter: &DocumentFilter,
    max_parse_bytes: usize,
    _provenance: ProvenanceLevel,
) -> Result<ExtractedMarkdown> {
    let mut zip = limits::open_zip_archive(source.bytes(), "pptx", max_parse_bytes)?;

    // Deck order comes from presentation.xml's sldIdLst — the order PowerPoint
    // shows — with a deterministic fallback to filename-number order for
    // packages without a parseable presentation part. Slide numbers are
    // 1-based positions in this order; hidden slides keep their position so
    // numbering stays aligned with PowerPoint, warnings and anchors.
    let slides = deck_slide_order(&mut zip, max_parse_bytes)?;
    let page_count = slides.len();

    // Same 1-based inclusive slice contract as PDF's --pages: numbers follow
    // source positions (a 2:2 slice still renders "## Slide 2"), and a slice
    // starting past the deck is a clear caller error rather than empty output.
    let page_range = document_filter.page_range;
    if let Some((first, _)) = page_range {
        if first > page_count {
            return Err(StructuredError::parse_failed(
                format!("请求的页码超出文档范围：起始页 {first} 超过总页数 {page_count}。"),
                ParseStage::Parse,
            )
            .into());
        }
    }
    let selected =
        |number: usize| page_range.is_none_or(|(first, last)| number >= first && number <= last);

    let mut md = MarkdownBuilder::with_max_bytes(max_parse_bytes);
    let mut warnings = Vec::new();
    // Slide-level provenance is free to compute while concatenating (the PDF
    // M1 pattern). A slide is a small enough citation unit that Page and
    // Block levels both mean "one span per slide"; the engine drops the spans
    // when provenance is Off.
    let mut spans: Vec<ProvenanceSpan> = Vec::new();
    let mut image_number: usize = 0;
    for (index, name) in slides.iter().enumerate() {
        let number = index + 1;
        if !selected(number) {
            continue;
        }
        md.blank_line();
        let start = md.len();
        md.heading(2, &format!("Slide {number}"));
        let xml = limits::read_zip_text(&mut zip, name, max_parse_bytes)?;
        if slide_is_hidden(&xml)? {
            // The author retracted this slide from the show; extracting its
            // body would let an agent cite content the audience never saw.
            // The heading (and the warning) keep the omission visible.
            warnings.push(SpoorWarning::at_slide(
                WarningCode::HiddenSlideOmitted,
                format!(
                    "第 {number} 张幻灯片被作者标记为隐藏，正文未提取；页码保留以对齐放映顺序。"
                ),
                number,
            ));
        } else {
            let rels = slide_rel_targets(&mut zip, name, max_parse_bytes)?;
            let mut node_budget = NodeBudget::new(max_parse_bytes);
            let (background_blips, shapes) = parse_slide(&xml, &mut node_budget)?;
            // Chart data and SmartArt text live in separate package parts;
            // resolve them up front so rendering can put the recovered data
            // at the shape's position in reading order.
            let graphics = load_graphics(&mut zip, &shapes, &rels.parts, max_parse_bytes);
            let mut emitted = SlideImageEmission::default();
            let mut tally = GraphicTally::default();
            let body = render_slide(
                background_blips,
                shapes,
                number,
                &rels.images,
                &graphics,
                &mut image_number,
                &mut emitted,
                &mut tally,
                &mut md,
            );
            let features = scan_slide_features(&xml)?;
            let notes_rendered =
                if let Some(notes_name) = notes_slide_for(&mut zip, name, max_parse_bytes)? {
                    let notes_xml = limits::read_zip_text(&mut zip, &notes_name, max_parse_bytes)?;
                    render_notes(&notes_xml, max_parse_bytes, &mut md)?
                } else {
                    false
                };
            if !body.saw_body_text && (emitted.total_blips > 0 || tally.opaque > 0) {
                warnings.push(no_text_layer_warning(number, notes_rendered));
            }
            warnings.extend(feature_warnings(number, features, emitted, tally));
        }
        spans.push(ProvenanceSpan {
            output: TextRange {
                start,
                end: md.len(),
            },
            source: SourceAnchor::Slide { number },
        });
    }

    let markdown = md.build()?;
    // build() normalizes trailing whitespace to a single newline; clamp the
    // final block so no span dangles past the built string.
    for span in &mut spans {
        span.output.end = span.output.end.min(markdown.len());
    }
    spans.retain(|span| span.output.start < span.output.end);

    Ok(ExtractedMarkdown {
        markdown,
        warnings,
        page_count: Some(page_count),
        provenance: spans,
    })
}

/// Slide part names in deck (presentation) order. Prefers `sldIdLst` resolved
/// through the presentation rels; falls back to filename-number order when the
/// presentation part is missing, lists no slides, or references a part absent
/// from the archive — a deterministic degradation for hand-built packages,
/// not a guess.
fn deck_slide_order<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    max_parse_bytes: usize,
) -> Result<Vec<String>> {
    let names: HashSet<String> = zip.file_names().map(str::to_string).collect();
    // A corrupt presentation part (ill-formed XML, non-UTF-8, over-budget) is
    // the same caller story as a missing one — the slide parts themselves are
    // intact — so an Err here degrades to filename order instead of failing
    // the whole parse.
    if let Ok(Some(order)) = presentation_slide_order(zip, max_parse_bytes) {
        // A crafted sldIdLst can reference the same slide part twice, which
        // would render (and number) it once per reference — bounded output
        // amplification. A duplicate is the same class of malformed
        // presentation part as a dangling reference: distrust the whole list.
        let unique: HashSet<&String> = order.iter().collect();
        if !order.is_empty()
            && unique.len() == order.len()
            && order.iter().all(|name| names.contains(name))
        {
            return Ok(order);
        }
    }
    // Covered by 17_no_presentation.pptx: a package without a parseable
    // presentation part degrades to filename-number order. Full-tuple sort:
    // two entries can parse to the same number (slide1.xml vs slide01.xml),
    // and the name tiebreak keeps the order deterministic across runs where
    // the HashSet's iteration order is not.
    let mut slides: Vec<(u32, String)> = names
        .into_iter()
        .filter_map(|name| slide_number(&name).map(|n| (n, name)))
        .collect();
    slides.sort();
    Ok(slides.into_iter().map(|(_, name)| name).collect())
}

/// The deck order declared by `ppt/presentation.xml`, or `None` when any part
/// of the chain (presentation part, its rels, an `r:id` reference) is missing
/// so the caller can fall back.
fn presentation_slide_order<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    max_parse_bytes: usize,
) -> Result<Option<Vec<String>>> {
    let Some(presentation) =
        limits::read_zip_text_optional(zip, "ppt/presentation.xml", max_parse_bytes)?
    else {
        return Ok(None);
    };
    let Some(rels_xml) =
        limits::read_zip_text_optional(zip, "ppt/_rels/presentation.xml.rels", max_parse_bytes)?
    else {
        return Ok(None);
    };
    let targets = parse_slide_rel_targets(&rels_xml);
    let ids = sld_id_list(&presentation)?;
    if ids.is_empty() {
        return Ok(None);
    }
    let mut order = Vec::with_capacity(ids.len());
    for id in ids {
        match targets.get(&id) {
            Some(target) => order.push(target.clone()),
            None => return Ok(None),
        }
    }
    Ok(Some(order))
}

/// The `r:id` values of `<p:sldId>` entries in document order. Returns an
/// empty list (→ fallback) when any entry lacks a relationship id. Entries
/// inside an `mc:Fallback` branch are skipped — collecting both Choice and
/// Fallback would double every wrapped slide reference.
fn sld_id_list(xml: &str) -> Result<Vec<String>> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut in_list = false;
    let mut fallback_depth = 0usize;
    let mut ids = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"sldIdLst" => in_list = true,
            Ok(Event::End(e)) if e.local_name().as_ref() == b"sldIdLst" => break,
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"Fallback" => fallback_depth += 1,
            Ok(Event::End(e)) if e.local_name().as_ref() == b"Fallback" => {
                fallback_depth = fallback_depth.saturating_sub(1)
            }
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if in_list && fallback_depth == 0 && e.local_name().as_ref() == b"sldId" =>
            {
                match relationship_id_attr(&e) {
                    Some(id) => ids.push(id),
                    None => return Ok(Vec::new()),
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(anyhow!("XML parse error: {error}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(ids)
}

/// The `r:id` attribute of a `<p:sldId>` / `<c:chart>`. `xml::attr` matches by
/// local name, which is ambiguous here: `<p:sldId>` carries both `id="256"`
/// and `r:id="rId2"`, whose local names are both `id`.
fn relationship_id_attr(e: &quick_xml::events::BytesStart<'_>) -> Option<String> {
    prefixed_attr(e, b"id")
}

/// Find an attribute by local name among *prefixed* attributes only. The
/// conventional relationships prefix `r` wins outright, so a foreign
/// `foo:id` cannot shadow `r:id`; a lone prefixed match (renamed
/// relationships prefix) is still accepted.
fn prefixed_attr(e: &quick_xml::events::BytesStart<'_>, local: &[u8]) -> Option<String> {
    let mut fallback: Option<String> = None;
    for a in e.attributes().filter_map(|a| a.ok()) {
        if a.key.local_name().as_ref() != local {
            continue;
        }
        let Some(prefix) = a.key.prefix() else {
            continue;
        };
        let value = String::from_utf8(a.value.into_owned()).ok();
        if prefix.as_ref() == b"r" {
            return value;
        }
        if fallback.is_none() {
            fallback = value;
        }
    }
    fallback
}

/// `rId → ppt/slides/slideN.xml` from the presentation's rels, keeping only
/// `…/slide` relationship types (`/slideMaster` and `/slideLayout` end
/// differently, so the suffix match cannot confuse them) and normalizing
/// targets against the `ppt/` base.
fn parse_slide_rel_targets(xml: &str) -> HashMap<String, String> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut map = HashMap::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if e.local_name().as_ref() == b"Relationship" =>
            {
                let rel_type = attr(&e, b"Type").unwrap_or_default();
                if !rel_type.ends_with("/slide") {
                    buf.clear();
                    continue;
                }
                // An external-mode target is a URL, not a package part; drop
                // it here so this map never carries a non-archive name even
                // if a future caller skips the existence check.
                if attr(&e, b"TargetMode").as_deref() == Some("External") {
                    buf.clear();
                    continue;
                }
                if let (Some(id), Some(target)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                    map.insert(id, normalize_zip_path(Path::new("ppt").join(target)));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    map
}

/// Whether the slide's root `<p:sld>` carries `show="0"`/`show="false"` — the
/// author hid it from the show.
fn slide_is_hidden(xml: &str) -> Result<bool> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                return Ok(e.local_name().as_ref() == b"sld"
                    && matches!(attr(&e, b"show").as_deref(), Some("0") | Some("false")));
            }
            Ok(Event::Eof) => return Ok(false),
            Err(error) => return Err(anyhow!("XML parse error: {error}")),
            _ => {}
        }
        buf.clear();
    }
}

/// The "you got nothing" posture signal: the slide rendered no body text —
/// a title alone is a label, not content — while visual objects are present.
/// Unlike `EmbeddedVisualsOmitted` ("what you got is incomplete") the slide
/// is unreadable without routing its visuals to a VLM. Wording tells the
/// agent whether speaker notes still carried text out.
fn no_text_layer_warning(slide: usize, notes_rendered: bool) -> SpoorWarning {
    let message = if notes_rendered {
        format!(
            "第 {slide} 张幻灯片除标题外无正文文本，内容全部在图像或图形对象里；演讲者备注已提取，页面视觉仍需交外部 VLM。"
        )
    } else {
        format!(
            "第 {slide} 张幻灯片除标题外无正文文本，内容全部在图像或图形对象里；不经外部 VLM 此页信息不可用。"
        )
    };
    SpoorWarning::at_slide(WarningCode::SlideNoTextLayer, message, slide)
}

/// Per-slide tally of `<a:blip>` references the renderer saw and what it could
/// resolve to a safe `ppt/media/*` part name. Feeds the warning so the agent
/// knows whether every visual was marked, only some were, or none.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SlideImageEmission {
    total_blips: usize,
    emitted_handles: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SlideFeatures {
    merged_table: bool,
    embedded_visuals: bool,
}

/// The graphicFrame payload type spoor extracts natively; every other
/// `graphicData` uri (chart, SmartArt diagram, OLE, future payloads) is a
/// visual object the text output cannot represent.
const TABLE_GRAPHIC_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/table";

fn scan_slide_features(xml: &str) -> Result<SlideFeatures> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut features = SlideFeatures::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                features.merged_table |= [
                    b"gridSpan".as_slice(),
                    b"rowSpan".as_slice(),
                    b"hMerge".as_slice(),
                    b"vMerge".as_slice(),
                ]
                .iter()
                .any(|name| attr(&e, name).is_some());
                // Beyond bitmaps/charts/OLE, any non-table graphicData payload
                // (SmartArt diagrams most commonly) is a visual the output
                // omits — without this, a SmartArt-only slide would render as
                // a bare heading with no posture signal at all.
                features.embedded_visuals |= matches!(
                    e.local_name().as_ref(),
                    b"pic" | b"blip" | b"chart" | b"oleObj"
                ) || (e.local_name().as_ref() == b"graphicData"
                    && attr(&e, b"uri").is_some_and(|uri| uri != TABLE_GRAPHIC_URI));
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(anyhow!("XML parse error: {error}")),
            _ => {}
        }
        buf.clear();
    }

    Ok(features)
}

fn feature_warnings(
    slide: usize,
    features: SlideFeatures,
    emission: SlideImageEmission,
    tally: GraphicTally,
) -> Vec<SpoorWarning> {
    let mut warnings = Vec::new();
    if features.merged_table {
        warnings.push(SpoorWarning::at_slide(
            WarningCode::MergedTableStructureNotPreserved,
            format!("第 {slide} 张幻灯片表格含合并单元格，Markdown 降级后跨行/跨列信息已丢失。"),
            slide,
        ));
    }
    // Mirror pdf.rs's three-branch wording for bitmaps: did every visual get
    // a handle, none, or only some? Agents key off the wording to decide
    // whether the slide is fully recoverable via `--extract` or still needs
    // external VLM rendering. A slide whose only graphics were fully
    // unpacked in-line (charts→tables, SmartArt→text) is no longer
    // incomplete and draws no warning at all.
    let message = if emission.total_blips > 0 {
        Some(if emission.emitted_handles == emission.total_blips {
            format!(
                "第 {slide} 张幻灯片有 {n} 张图片，已用 spoor://pptx/part/ 标注；可用 --extract 取出交 VLM。",
                n = emission.emitted_handles,
            )
        } else if emission.emitted_handles == 0 {
            format!(
                "第 {slide} 张幻灯片有 {n} 张图片，引用未能解析；输出可能不完整。",
                n = emission.total_blips,
            )
        } else {
            format!(
                "第 {slide} 张幻灯片有 {total} 张图片：{ok} 张可用 --extract 取出，其余未能解析。",
                total = emission.total_blips,
                ok = emission.emitted_handles,
            )
        })
    } else if tally.opaque > 0 || (features.embedded_visuals && tally.rendered == 0) {
        Some(format!(
            "第 {slide} 张幻灯片含未能解构的图形对象（图表/SmartArt/OLE，无位图），仅提取了文本；输出不完整。"
        ))
    } else {
        None
    };
    if let Some(message) = message {
        warnings.push(SpoorWarning::at_slide(
            WarningCode::EmbeddedVisualsOmitted,
            message,
            slide,
        ));
    }
    warnings
}

fn slide_number(name: &str) -> Option<u32> {
    name.strip_prefix("ppt/slides/slide")?
        .strip_suffix(".xml")?
        .parse::<u32>()
        .ok()
}

/// What the slide body yielded: whether any actual text (as opposed to image
/// placeholders spoor itself inserted) was rendered. Feeds the no-text-layer
/// posture warning.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SlideBody {
    saw_body_text: bool,
}

/// Nesting cap for group shapes: far beyond anything a real deck produces, it
/// bounds parser recursion on a crafted file. A deeper group degrades to a
/// transparent container — its children still parse, flattened into the
/// containing level — so content survives, only nesting structure is lost.
const MAX_GROUP_DEPTH: usize = 32;

/// A shape parsed from the slide's shape tree — the structural minimum the
/// renderer needs for reading order and semantics. `top`/`left` are the
/// shape's own `a:off` EMU when present; placeholders inheriting geometry
/// from the layout/master carry none (spoor does not resolve that chain) and
/// sort to the front, which is where layout placeholders sit in practice.
#[derive(Debug, Default)]
struct Shape {
    top: Option<i64>,
    left: Option<i64>,
    body: ShapeBody,
    /// Every `a:blip` seen inside the shape, in XML order; an entry is the
    /// `r:embed` rId or empty when the blip has none (still counted so the
    /// visuals warning reports the gap). For a Picture the first entry is the
    /// image itself; everything else (fills, OLE fallbacks) renders after the
    /// shape's own content.
    blips: Vec<String>,
}

#[derive(Debug, Default)]
enum ShapeBody {
    #[default]
    Empty,
    Text {
        ph: Option<String>,
        paras: Vec<Para>,
    },
    Table(Vec<Vec<String>>),
    Picture {
        descr: Option<String>,
    },
    Group(Vec<Shape>),
    /// A chart frame; `rid` points at its data part (`ppt/charts/chartN.xml`).
    Chart {
        rid: Option<String>,
    },
    /// A SmartArt frame; `rid` points at its data-model part
    /// (`ppt/diagrams/dataN.xml`, via `dgm:relIds@r:dm`).
    Diagram {
        rid: Option<String>,
    },
    /// A graphicFrame payload spoor cannot unpack (OLE object, unknown
    /// graphicData); counts toward the visuals-omitted posture.
    Opaque,
}

#[derive(Debug, Default)]
struct Para {
    text: String,
    lvl: u8,
    bullet: Bullet,
}

/// Explicit bullet property on a paragraph. `Inherit` means the slide XML
/// says nothing — the effective value lives in the layout/master style chain,
/// which spoor does not read; the renderer falls back to the placeholder
/// convention (content placeholders are bulleted, text boxes are prose).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Bullet {
    #[default]
    Inherit,
    None,
    Char,
    AutoNum,
}

/// Byte-derived cap on parsed shape-tree nodes (shapes, paragraphs, blips).
/// The tree is the one place the pptx parser materializes per-node structs
/// out of streamed XML; without a cap, a small crafted file (megabytes of
/// empty `<p:sp/>`) amplifies into gigabytes of resident structs. Charged
/// against `max_parse_bytes / 256` so the tree stays a fraction of the parse
/// budget; exceeding it is the same caller story as any other budget hit — a
/// structured error, not silent truncation.
struct NodeBudget {
    remaining: usize,
    max_parse_bytes: usize,
}

impl NodeBudget {
    fn new(max_parse_bytes: usize) -> Self {
        Self {
            remaining: (max_parse_bytes / 256).max(4096),
            max_parse_bytes,
        }
    }

    fn charge(&mut self) -> Result<()> {
        if self.remaining == 0 {
            return Err(StructuredError::parse_memory_limit(
                self.max_parse_bytes,
                "PPTX shape tree",
            )
            .into());
        }
        self.remaining -= 1;
        Ok(())
    }
}

/// Unescape a text event, degrading to the raw bytes on failure: an unknown
/// entity (`&nbsp;` from HTML-minded generators) must not silently drop the
/// whole chunk — that loses legitimate text and can fake a no-text-layer
/// posture.
fn text_of(t: &quick_xml::events::BytesText<'_>) -> String {
    t.unescape()
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| String::from_utf8_lossy(t.as_ref()).into_owned())
}

/// Parse the slide's shape tree, plus any stray blips *before* it (the slide
/// background), leaving render order to the caller. `mc:Fallback` branches
/// are skipped — the same policy as `sld_id_list` — so AlternateContent
/// (equations, chartex, WordArt) renders once, not once per branch. Also
/// used for notes slides, whose `cSld/spTree` structure is identical.
fn parse_slide(xml: &str, budget: &mut NodeBudget) -> Result<(Vec<String>, Vec<Shape>)> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut background_blips = Vec::new();
    let mut shapes = Vec::new();
    let mut in_shape_tree = false;
    let mut seen_shape_tree = false;
    let mut fallback_depth = 0usize;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if fallback_depth > 0 => {
                if e.local_name().as_ref() == b"Fallback" {
                    fallback_depth += 1;
                }
            }
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"Fallback" => fallback_depth += 1,
                b"spTree" => {
                    in_shape_tree = true;
                    seen_shape_tree = true;
                }
                b"sp" | b"cxnSp" if in_shape_tree => {
                    shapes.push(parse_text_shape(&mut reader, budget)?)
                }
                b"pic" if in_shape_tree => shapes.push(parse_picture(&mut reader, budget)?),
                b"graphicFrame" if in_shape_tree => {
                    shapes.push(parse_graphic_frame(&mut reader, budget)?)
                }
                b"grpSp" if in_shape_tree => shapes.push(parse_group(&mut reader, 1, budget)?),
                // Only pre-spTree strays are the slide background; blips after
                // the tree (ActiveX control previews in p:controls) are not
                // content and must not lead the slide.
                b"blip" if !seen_shape_tree => {
                    budget.charge()?;
                    background_blips.push(attr(&e, b"embed").unwrap_or_default());
                }
                _ => {}
            },
            Ok(Event::Empty(e))
                if fallback_depth == 0
                    && e.local_name().as_ref() == b"blip"
                    && !seen_shape_tree =>
            {
                budget.charge()?;
                background_blips.push(attr(&e, b"embed").unwrap_or_default());
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"Fallback" => fallback_depth = fallback_depth.saturating_sub(1),
                b"spTree" => in_shape_tree = false,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => return Err(anyhow!("XML parse error: {error}")),
            _ => {}
        }
        buf.clear();
    }
    Ok((background_blips, shapes))
}

/// The placeholder type, with the schema default `obj` (a content
/// placeholder) when the attribute is absent.
fn placeholder_type(e: &quick_xml::events::BytesStart<'_>) -> String {
    attr(e, b"type").unwrap_or_else(|| "obj".to_string())
}

/// Record the shape's own offset from its first `a:off`. Later `a:off`
/// elements (nested drawing internals) never override it.
fn record_offset(e: &quick_xml::events::BytesStart<'_>, shape: &mut Shape) {
    if shape.top.is_some() {
        return;
    }
    if let (Some(y), Some(x)) = (
        attr(e, b"y").and_then(|v| v.parse::<i64>().ok()),
        attr(e, b"x").and_then(|v| v.parse::<i64>().ok()),
    ) {
        shape.top = Some(y);
        shape.left = Some(x);
    }
}

/// Parse a `<p:sp>` / `<p:cxnSp>` subtree (the Start event is already
/// consumed) into a text shape: placeholder type, offset, paragraphs with
/// level and bullet props, and any fill blips. Paragraph text is taken only
/// from inside `<a:t>` runs — inter-element whitespace in pretty-printed XML
/// must not end up in headings or list items — and `<a:br/>` becomes a space
/// so runs on either side of a soft break do not glue together.
fn parse_text_shape(reader: &mut Reader<&[u8]>, budget: &mut NodeBudget) -> Result<Shape> {
    budget.charge()?;
    let mut shape = Shape::default();
    let mut ph: Option<String> = None;
    let mut paras: Vec<Para> = Vec::new();
    let mut current: Option<Para> = None;
    let mut in_tx_body = false;
    let mut in_text_run = false;
    let mut depth = 1usize;
    let mut buf = Vec::new();

    fn dispatch(
        e: &quick_xml::events::BytesStart<'_>,
        shape: &mut Shape,
        ph: &mut Option<String>,
        current: &mut Option<Para>,
        in_tx_body: bool,
        budget: &mut NodeBudget,
    ) -> Result<()> {
        match e.local_name().as_ref() {
            b"ph" => *ph = Some(placeholder_type(e)),
            b"off" => record_offset(e, shape),
            b"blip" => {
                budget.charge()?;
                shape.blips.push(attr(e, b"embed").unwrap_or_default());
            }
            b"br" => {
                if let Some(para) = current {
                    para.text.push(' ');
                }
            }
            b"pPr" if in_tx_body => {
                if let Some(para) = current {
                    para.lvl = attr(e, b"lvl").and_then(|v| v.parse().ok()).unwrap_or(0);
                }
            }
            b"buNone" => {
                if let Some(para) = current {
                    para.bullet = Bullet::None;
                }
            }
            b"buChar" | b"buBlip" => {
                if let Some(para) = current {
                    para.bullet = Bullet::Char;
                }
            }
            b"buAutoNum" => {
                if let Some(para) = current {
                    para.bullet = Bullet::AutoNum;
                }
            }
            _ => {}
        }
        Ok(())
    }

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                match e.local_name().as_ref() {
                    b"txBody" => in_tx_body = true,
                    b"p" if in_tx_body => {
                        budget.charge()?;
                        current = Some(Para::default());
                    }
                    b"t" if in_tx_body => in_text_run = true,
                    _ => dispatch(&e, &mut shape, &mut ph, &mut current, in_tx_body, budget)?,
                }
                depth += 1;
            }
            Ok(Event::Empty(e)) => {
                dispatch(&e, &mut shape, &mut ph, &mut current, in_tx_body, budget)?
            }
            Ok(Event::Text(t)) if in_text_run => {
                if let Some(para) = &mut current {
                    para.text.push_str(&text_of(&t));
                }
            }
            // CDATA is legal OOXML even though PowerPoint never writes it;
            // third-party generators do, and dropping it would both lose text
            // and fire a false no-text-layer posture.
            Ok(Event::CData(t)) if in_text_run => {
                if let Some(para) = &mut current {
                    para.text
                        .push_str(&String::from_utf8_lossy(&t.into_inner()));
                }
            }
            Ok(Event::End(e)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                match e.local_name().as_ref() {
                    b"txBody" => in_tx_body = false,
                    b"t" => in_text_run = false,
                    b"p" => {
                        if let Some(para) = current.take() {
                            paras.push(para);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(anyhow!("XML parse error: {error}")),
            _ => {}
        }
        buf.clear();
    }
    shape.body = ShapeBody::Text { ph, paras };
    Ok(shape)
}

/// Parse a `<p:pic>` subtree: offset, alt text (`cNvPr@descr`) and its blips
/// (the first is the picture itself).
fn parse_picture(reader: &mut Reader<&[u8]>, budget: &mut NodeBudget) -> Result<Shape> {
    budget.charge()?;
    let mut shape = Shape::default();
    let mut descr: Option<String> = None;
    let mut depth = 1usize;
    let mut buf = Vec::new();
    fn dispatch(
        e: &quick_xml::events::BytesStart<'_>,
        shape: &mut Shape,
        descr: &mut Option<String>,
        budget: &mut NodeBudget,
    ) -> Result<()> {
        match e.local_name().as_ref() {
            b"cNvPr" if descr.is_none() => {
                *descr = attr(e, b"descr").filter(|d| !d.trim().is_empty());
            }
            b"off" => record_offset(e, shape),
            b"blip" => {
                budget.charge()?;
                shape.blips.push(attr(e, b"embed").unwrap_or_default());
            }
            _ => {}
        }
        Ok(())
    }

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                dispatch(&e, &mut shape, &mut descr, budget)?;
                depth += 1;
            }
            Ok(Event::Empty(e)) => dispatch(&e, &mut shape, &mut descr, budget)?,
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(anyhow!("XML parse error: {error}")),
            _ => {}
        }
        buf.clear();
    }
    shape.body = ShapeBody::Picture { descr };
    Ok(shape)
}

/// Parse a `<p:graphicFrame>` subtree: offset, its payload (table rows,
/// chart reference, SmartArt data reference, or an opaque object), and any
/// blips (chart fills, OLE preview images) as extras.
fn parse_graphic_frame(reader: &mut Reader<&[u8]>, budget: &mut NodeBudget) -> Result<Shape> {
    budget.charge()?;
    let mut shape = Shape::default();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Option<Vec<String>> = None;
    let mut current_cell = String::new();
    let mut in_cell = false;
    let mut chart_rid: Option<String> = None;
    let mut diagram_rid: Option<String> = None;
    let mut non_table_payload = false;
    let mut depth = 1usize;
    let mut buf = Vec::new();

    fn classify(
        e: &quick_xml::events::BytesStart<'_>,
        shape: &mut Shape,
        chart_rid: &mut Option<String>,
        diagram_rid: &mut Option<String>,
        non_table_payload: &mut bool,
    ) {
        match e.local_name().as_ref() {
            b"off" => record_offset(e, shape),
            b"blip" => shape.blips.push(attr(e, b"embed").unwrap_or_default()),
            b"graphicData" => {
                *non_table_payload |= attr(e, b"uri").is_some_and(|uri| uri != TABLE_GRAPHIC_URI);
            }
            b"chart" if chart_rid.is_none() => *chart_rid = relationship_id_attr(e),
            // SmartArt: <dgm:relIds r:dm="…"/> — dm names the data-model part.
            b"relIds" if diagram_rid.is_none() => *diagram_rid = prefixed_attr(e, b"dm"),
            _ => {}
        }
    }

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                match e.local_name().as_ref() {
                    b"tr" => {
                        budget.charge()?;
                        current_row = Some(Vec::new());
                    }
                    b"tc" => {
                        budget.charge()?;
                        in_cell = true;
                        current_cell.clear();
                    }
                    _ => classify(
                        &e,
                        &mut shape,
                        &mut chart_rid,
                        &mut diagram_rid,
                        &mut non_table_payload,
                    ),
                }
                depth += 1;
            }
            Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"br" && in_cell {
                    current_cell.push(' ');
                } else {
                    classify(
                        &e,
                        &mut shape,
                        &mut chart_rid,
                        &mut diagram_rid,
                        &mut non_table_payload,
                    )
                }
            }
            Ok(Event::Text(t)) if in_cell => {
                current_cell.push_str(&text_of(&t));
            }
            Ok(Event::CData(t)) if in_cell => {
                current_cell.push_str(&String::from_utf8_lossy(&t.into_inner()));
            }
            Ok(Event::End(e)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                match e.local_name().as_ref() {
                    b"p" if in_cell => current_cell.push(' '),
                    b"tc" => {
                        if let Some(row) = &mut current_row {
                            row.push(current_cell.trim().to_string());
                        }
                        current_cell.clear();
                        in_cell = false;
                    }
                    b"tr" => {
                        if let Some(row) = current_row.take() {
                            if !row.is_empty() {
                                rows.push(row);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(anyhow!("XML parse error: {error}")),
            _ => {}
        }
        buf.clear();
    }
    shape.body = if !rows.is_empty() {
        ShapeBody::Table(rows)
    } else if chart_rid.is_some() {
        ShapeBody::Chart { rid: chart_rid }
    } else if diagram_rid.is_some() {
        ShapeBody::Diagram { rid: diagram_rid }
    } else if non_table_payload {
        ShapeBody::Opaque
    } else {
        ShapeBody::Empty
    };
    Ok(shape)
}

/// Parse a `<p:grpSp>` subtree: the group's own offset (its `grpSpPr` xfrm)
/// and its children. Children keep their group-local coordinates — a group
/// renders as one visual unit sorted by its own position, and within it the
/// children sort in their shared local space, which translation and positive
/// scaling cannot reorder.
fn parse_group(
    reader: &mut Reader<&[u8]>,
    group_depth: usize,
    budget: &mut NodeBudget,
) -> Result<Shape> {
    budget.charge()?;
    let mut shape = Shape::default();
    let mut children: Vec<Shape> = Vec::new();
    let mut depth = 1usize;
    let mut fallback_depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if fallback_depth > 0 => {
                if e.local_name().as_ref() == b"Fallback" {
                    fallback_depth += 1;
                }
                depth += 1;
            }
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"Fallback" => {
                    fallback_depth += 1;
                    depth += 1;
                }
                b"sp" | b"cxnSp" => children.push(parse_text_shape(reader, budget)?),
                b"pic" => children.push(parse_picture(reader, budget)?),
                b"graphicFrame" => children.push(parse_graphic_frame(reader, budget)?),
                b"grpSp" if group_depth < MAX_GROUP_DEPTH => {
                    children.push(parse_group(reader, group_depth + 1, budget)?)
                }
                b"off" => {
                    record_offset(&e, &mut shape);
                    depth += 1;
                }
                b"blip" => {
                    budget.charge()?;
                    shape.blips.push(attr(&e, b"embed").unwrap_or_default());
                    depth += 1;
                }
                _ => depth += 1,
            },
            Ok(Event::Empty(e)) if fallback_depth == 0 => match e.local_name().as_ref() {
                b"off" => record_offset(&e, &mut shape),
                b"blip" => {
                    budget.charge()?;
                    shape.blips.push(attr(&e, b"embed").unwrap_or_default());
                }
                _ => {}
            },
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == b"Fallback" {
                    fallback_depth = fallback_depth.saturating_sub(1);
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(anyhow!("XML parse error: {error}")),
            _ => {}
        }
        buf.clear();
    }
    shape.body = ShapeBody::Group(children);
    Ok(shape)
}

/// Per-slide tally of graphicFrame payloads: how many were unpacked into the
/// output (chart tables, SmartArt text) versus left opaque (OLE, unresolved
/// references). Drives the visuals warning: a fully-unpacked slide no longer
/// claims its output is incomplete.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct GraphicTally {
    rendered: usize,
    opaque: usize,
}

struct RenderCtx<'a> {
    slide_number: usize,
    rels: &'a HashMap<String, String>,
    graphics: &'a HashMap<String, GraphicContent>,
    image_number: &'a mut usize,
    emission: &'a mut SlideImageEmission,
    tally: &'a mut GraphicTally,
    saw_body_text: bool,
}

/// Render a parsed shape tree in reading order — title placeholders first,
/// remaining shapes top-to-bottom then left-to-right (stable, so shapes
/// without an offset keep authoring order at the front). Replaces the flat
/// XML-order text dump, which interleaved multi-column layouts and lost
/// title/bullet semantics.
#[allow(clippy::too_many_arguments)]
fn render_slide(
    background_blips: Vec<String>,
    shapes: Vec<Shape>,
    slide_number: usize,
    rels: &HashMap<String, String>,
    graphics: &HashMap<String, GraphicContent>,
    image_number: &mut usize,
    emission: &mut SlideImageEmission,
    tally: &mut GraphicTally,
    md: &mut MarkdownBuilder,
) -> SlideBody {
    let mut ctx = RenderCtx {
        slide_number,
        rels,
        graphics,
        image_number,
        emission,
        tally,
        saw_body_text: false,
    };
    for rid in &background_blips {
        render_blip(rid, None, &mut ctx, md);
    }
    render_shapes(shapes, true, &mut ctx, md);
    SlideBody {
        saw_body_text: ctx.saw_body_text,
    }
}

fn render_shapes(
    shapes: Vec<Shape>,
    top_level: bool,
    ctx: &mut RenderCtx<'_>,
    md: &mut MarkdownBuilder,
) {
    let (titles, rest): (Vec<Shape>, Vec<Shape>) = if top_level {
        shapes.into_iter().partition(is_title_shape)
    } else {
        (Vec::new(), shapes)
    };
    for shape in titles.into_iter().chain(reading_order(rest)) {
        render_shape(shape, ctx, md);
    }
}

fn is_title_shape(shape: &Shape) -> bool {
    matches!(
        &shape.body,
        ShapeBody::Text { ph: Some(t), .. } if t == "title" || t == "ctrTitle"
    )
}

/// Deterministic visual order: (top, left) with a stable sort, shapes without
/// an offset keyed to the front. Plain two-key ordering can interleave true
/// multi-column layouts (a known, documented limit — row banding is a future
/// refinement); it is still strictly better than z-order for every stacked
/// layout.
fn reading_order(mut shapes: Vec<Shape>) -> Vec<Shape> {
    // Sort on the Options directly: `None < Some(_)` puts offset-less shapes
    // (layout-inherited placeholders — the main content) genuinely first,
    // where `unwrap_or(0)` would let a negative-coordinate shape (an author's
    // off-canvas scratch box) cut in front of them.
    shapes.sort_by_key(|s| (s.top, s.left));
    shapes
}

fn render_shape(shape: Shape, ctx: &mut RenderCtx<'_>, md: &mut MarkdownBuilder) {
    let Shape { body, blips, .. } = shape;
    match body {
        ShapeBody::Empty => {}
        ShapeBody::Text { ph, paras } => render_text_paragraphs(ph, paras, ctx, md),
        ShapeBody::Table(rows) => {
            if rows.iter().any(|row| row.iter().any(|c| !c.is_empty())) {
                ctx.saw_body_text = true;
            }
            md.table(&rows);
        }
        ShapeBody::Picture { descr } => {
            let mut iter = blips.iter();
            if let Some(primary) = iter.next() {
                render_blip(primary, descr.as_deref(), ctx, md);
            }
            for rid in iter {
                render_blip(rid, None, ctx, md);
            }
            return;
        }
        ShapeBody::Group(children) => render_shapes(children, false, ctx, md),
        ShapeBody::Chart { rid } => match rid.as_deref().and_then(|r| ctx.graphics.get(r)) {
            Some(GraphicContent::Chart { title, table, note }) => {
                let label = match title {
                    Some(t) => format!("Chart: {t}"),
                    None => "Chart:".to_string(),
                };
                md.paragraph(&label);
                md.table(table);
                if let Some(note) = note {
                    md.paragraph(note);
                }
                if table.iter().any(|row| row.iter().any(|c| !c.is_empty())) {
                    ctx.saw_body_text = true;
                }
                ctx.tally.rendered += 1;
            }
            _ => ctx.tally.opaque += 1,
        },
        ShapeBody::Diagram { rid } => match rid.as_deref().and_then(|r| ctx.graphics.get(r)) {
            Some(GraphicContent::Diagram { texts }) if !texts.is_empty() => {
                md.paragraph("Diagram:");
                let mut items: Vec<String> = texts.iter().map(|t| format!("- {t}")).collect();
                flush_list(&mut items, md);
                ctx.saw_body_text = true;
                ctx.tally.rendered += 1;
            }
            _ => ctx.tally.opaque += 1,
        },
        ShapeBody::Opaque => ctx.tally.opaque += 1,
    }
    for rid in &blips {
        render_blip(rid, None, ctx, md);
    }
}

fn render_text_paragraphs(
    ph: Option<String>,
    paras: Vec<Para>,
    ctx: &mut RenderCtx<'_>,
    md: &mut MarkdownBuilder,
) {
    let ph = ph.as_deref();
    if matches!(ph, Some("title") | Some("ctrTitle")) {
        // Whitespace-normalize so a stray newline inside a run cannot break
        // the heading line apart.
        let text = paras
            .iter()
            .flat_map(|p| p.text.split_whitespace())
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            md.heading(3, &text);
            // Deliberately NOT counted as body text: a title is the slide's
            // label, not its content. Real-world image-only slides almost
            // always carry a title placeholder ("系统架构图") — counting it
            // would silence the no-text-layer posture on exactly the pages
            // that must be routed to a VLM.
        }
        return;
    }
    // Content placeholders (`body`, and `obj` — the schema default) are
    // bulleted lists in PowerPoint's own template semantics; an explicit
    // buNone opts a paragraph out. Text boxes, subtitles and furniture
    // placeholders default to prose.
    let bullet_default = matches!(ph, Some("body") | Some("obj"));
    let mut list: Vec<String> = Vec::new();
    for para in paras {
        let text = para.text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        ctx.saw_body_text = true;
        let marker = match para.bullet {
            Bullet::Char => Some("- "),
            Bullet::AutoNum => Some("1. "),
            Bullet::None => None,
            Bullet::Inherit => bullet_default.then_some("- "),
        };
        match marker {
            // Four spaces per level: CommonMark requires a child item to
            // reach the parent's content column, which is 3 for `1. ` — two
            // spaces would silently flatten (or renumber) nesting under
            // ordered parents.
            Some(marker) => list.push(format!(
                "{}{marker}{text}",
                "    ".repeat(para.lvl as usize)
            )),
            None => {
                flush_list(&mut list, md);
                md.paragraph(&text);
            }
        }
    }
    flush_list(&mut list, md);
}

/// Emit buffered list items as one block, via `raw` so nested-item leading
/// indentation survives (`paragraph` would trim it).
fn flush_list(list: &mut Vec<String>, md: &mut MarkdownBuilder) {
    if !list.is_empty() {
        md.blank_line();
        md.raw(&list.join("\n"));
        md.raw("\n");
    }
    list.clear();
}

/// Emit a `![PPTX image N (slide S)](spoor://pptx/part/ppt/media/...)`
/// placeholder paragraph. An empty/unresolvable rId still counts toward
/// `total_blips` (never `emitted_handles`) so the visuals warning surfaces
/// the gap. Author alt text rides after the stable label when present.
fn render_blip(rid: &str, descr: Option<&str>, ctx: &mut RenderCtx<'_>, md: &mut MarkdownBuilder) {
    ctx.emission.total_blips += 1;
    if rid.is_empty() {
        return;
    }
    let Some(target) = ctx.rels.get(rid) else {
        return;
    };
    *ctx.image_number += 1;
    ctx.emission.emitted_handles += 1;
    let number = *ctx.image_number;
    let slide = ctx.slide_number;
    let alt = match descr.and_then(sanitize_alt) {
        Some(d) => format!("PPTX image {number} (slide {slide}): {d}"),
        None => format!("PPTX image {number} (slide {slide})"),
    };
    md.paragraph(&format!("![{alt}](spoor://pptx/part/{target})"));
}

/// Author-provided alt text (`cNvPr@descr`), neutralized for the Markdown
/// image-alt position: link-breaking and control characters are dropped,
/// whitespace collapsed, and the result capped so a novel-length descr cannot
/// bloat agent-facing output. `None` when nothing readable is left.
fn sanitize_alt(descr: &str) -> Option<String> {
    let mut cleaned = String::new();
    let mut last_space = true;
    for ch in descr.chars() {
        let ch = match ch {
            '[' | ']' | '(' | ')' | '\\' | '`' => ' ',
            c if c.is_control() => ' ',
            // Invisible format characters (zero-width, bidi overrides, BOM,
            // soft hyphen): an RTL override in alt text would visually
            // reorder what the agent's user reads.
            '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{00AD}'
            | '\u{061C}' => ' ',
            c => c,
        };
        if ch.is_whitespace() {
            if !last_space {
                cleaned.push(' ');
                last_space = true;
            }
        } else {
            cleaned.push(ch);
            last_space = false;
        }
    }
    let capped: String = cleaned.trim().chars().take(120).collect();
    let capped = capped.trim_end();
    if capped.is_empty() {
        None
    } else {
        Some(capped.to_string())
    }
}

/// Data recovered from a graphicFrame's separate package part.
#[derive(Debug)]
enum GraphicContent {
    /// Chart plot data rebuilt as a table: header row = series names, data
    /// rows = categories.
    Chart {
        title: Option<String>,
        table: Vec<Vec<String>>,
        note: Option<String>,
    },
    /// SmartArt node labels in data-model order.
    Diagram { texts: Vec<String> },
}

/// Token-economy caps for chart tables: real business charts fit comfortably;
/// a data-dense or crafted chart truncates with an in-band note instead of
/// flooding the output the agent pays for.
const MAX_CHART_SERIES: usize = 12;
const MAX_CHART_POINTS: usize = 100;

/// Walk the parsed shapes and load each referenced chart / diagram part.
/// Any failure (missing part, over budget, ill-formed XML, empty data) simply
/// leaves the reference unresolved — the render pass counts it opaque and the
/// visuals warning tells the agent the slide is incomplete.
fn load_graphics<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    shapes: &[Shape],
    parts: &HashMap<String, String>,
    max_parse_bytes: usize,
) -> HashMap<String, GraphicContent> {
    let mut rids: Vec<(String, bool)> = Vec::new();
    collect_graphic_rids(shapes, &mut rids);
    let mut out = HashMap::new();
    for (rid, is_chart) in rids {
        if out.contains_key(&rid) {
            continue;
        }
        let Some(path) = parts.get(&rid) else {
            continue;
        };
        let Ok(Some(xml)) = limits::read_zip_text_optional(zip, path, max_parse_bytes) else {
            continue;
        };
        let content = if is_chart {
            parse_chart(&xml)
        } else {
            parse_diagram(&xml)
        };
        if let Some(content) = content {
            out.insert(rid, content);
        }
    }
    out
}

/// `(rid, is_chart)` for every chart / diagram frame, groups flattened.
fn collect_graphic_rids(shapes: &[Shape], sink: &mut Vec<(String, bool)>) {
    for shape in shapes {
        match &shape.body {
            ShapeBody::Chart { rid: Some(rid) } => sink.push((rid.clone(), true)),
            ShapeBody::Diagram { rid: Some(rid) } => sink.push((rid.clone(), false)),
            ShapeBody::Group(children) => collect_graphic_rids(children, sink),
            _ => {}
        }
    }
}

/// Rebuild a chart part's cached plot data (`c:cat`/`c:val` per `c:ser`) as a
/// small table — the numbers business decks put nowhere else. Reads only the
/// caches PowerPoint always writes: no formula evaluation, no styling.
/// Scatter charts map `xVal`→category, `yVal`→value. `None` when no series
/// carries values (the frame then stays opaque and draws the warning).
fn parse_chart(xml: &str) -> Option<GraphicContent> {
    #[derive(Default)]
    struct Series {
        name: Option<String>,
        cats: BTreeMap<usize, String>,
        vals: BTreeMap<usize, String>,
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Section {
        None,
        Name,
        Cat,
        Val,
    }
    // Subtrees that nest their own text/value elements but are chrome, not
    // plot data: reading them would corrupt series names and values.
    const SKIP: &[&[u8]] = &[b"dLbls", b"trendline", b"errBars", b"extLst"];

    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut title = String::new();
    let mut in_title = false;
    let mut title_done = false;
    let mut series: Vec<Series> = Vec::new();
    let mut current: Option<Series> = None;
    let mut section = Section::None;
    let mut point_idx: Option<usize> = None;
    let mut in_value = false;
    let mut value = String::new();
    let mut skip_depth = 0usize;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.local_name();
                let local = name.as_ref();
                if SKIP.contains(&local) {
                    skip_depth += 1;
                } else if skip_depth == 0 {
                    match local {
                        b"title" if !title_done => in_title = true,
                        b"ser" => current = Some(Series::default()),
                        b"tx" if current.is_some() && section == Section::None => {
                            section = Section::Name
                        }
                        b"cat" | b"xVal" if current.is_some() => section = Section::Cat,
                        b"val" | b"yVal" if current.is_some() => section = Section::Val,
                        b"pt" => point_idx = attr(&e, b"idx").and_then(|v| v.parse().ok()),
                        b"v" => {
                            in_value = true;
                            value.clear();
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                if skip_depth == 0 && e.local_name().as_ref() == b"pt" {
                    point_idx = attr(&e, b"idx").and_then(|v| v.parse().ok());
                }
            }
            Ok(Event::Text(t)) if skip_depth == 0 => {
                if in_value {
                    value.push_str(&t.unescape().map(|c| c.into_owned()).unwrap_or_default());
                } else if in_title {
                    title.push_str(&t.unescape().map(|c| c.into_owned()).unwrap_or_default());
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name();
                let local = name.as_ref();
                if SKIP.contains(&local) {
                    skip_depth = skip_depth.saturating_sub(1);
                } else if skip_depth == 0 {
                    match local {
                        b"title" => {
                            in_title = false;
                            title_done = true;
                        }
                        b"v" => {
                            in_value = false;
                            if let Some(ser) = &mut current {
                                let text = value.trim().to_string();
                                match section {
                                    Section::Name => {
                                        if ser.name.is_none() && !text.is_empty() {
                                            ser.name = Some(text);
                                        }
                                    }
                                    Section::Cat => {
                                        if let Some(idx) = point_idx {
                                            ser.cats.insert(idx, text);
                                        }
                                    }
                                    Section::Val => {
                                        if let Some(idx) = point_idx {
                                            ser.vals.insert(idx, text);
                                        }
                                    }
                                    Section::None => {}
                                }
                            }
                        }
                        b"tx" | b"cat" | b"val" | b"xVal" | b"yVal" => section = Section::None,
                        b"ser" => {
                            if let Some(ser) = current.take() {
                                series.push(ser);
                            }
                            section = Section::None;
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }

    if !series.iter().any(|s| !s.vals.is_empty()) {
        return None;
    }
    let total_series = series.len();
    series.truncate(MAX_CHART_SERIES);
    let mut idxs: BTreeSet<usize> = BTreeSet::new();
    for ser in &series {
        idxs.extend(ser.vals.keys().copied());
        idxs.extend(ser.cats.keys().copied());
    }
    let total_points = idxs.len();
    let idxs: Vec<usize> = idxs.into_iter().take(MAX_CHART_POINTS).collect();

    let mut header = vec![String::new()];
    for (i, ser) in series.iter().enumerate() {
        header.push(
            ser.name
                .clone()
                .unwrap_or_else(|| format!("Series {}", i + 1)),
        );
    }
    let mut table = vec![header];
    for idx in &idxs {
        let label = series
            .iter()
            .find_map(|s| s.cats.get(idx))
            .cloned()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| (idx + 1).to_string());
        let mut row = vec![label];
        for ser in &series {
            row.push(ser.vals.get(idx).cloned().unwrap_or_default());
        }
        table.push(row);
    }

    let mut clipped = Vec::new();
    if total_points > MAX_CHART_POINTS {
        clipped.push(format!("{MAX_CHART_POINTS} of {total_points} data points"));
    }
    if total_series > MAX_CHART_SERIES {
        clipped.push(format!("{MAX_CHART_SERIES} of {total_series} series"));
    }
    let note = (!clipped.is_empty())
        .then(|| format!("(chart table truncated: showing {})", clipped.join(", ")));
    let title = {
        let trimmed = title.split_whitespace().collect::<Vec<_>>().join(" ");
        (!trimmed.is_empty()).then_some(trimmed)
    };
    Some(GraphicContent::Chart { title, table, note })
}

/// SmartArt node labels from the diagram data model (`dgm:pt`/`dgm:t`), in
/// document order. The graph structure (hierarchy, connectors) is
/// deliberately not reconstructed — the labels carry the content an agent
/// needs; relationships would be layout interpretation.
fn parse_diagram(xml: &str) -> Option<GraphicContent> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    // dgm:t and the a:t runs inside it share the local name `t`; depth
    // counting flushes one node label per outermost `t`.
    let mut t_depth = 0usize;
    let mut node = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"t" => t_depth += 1,
            Ok(Event::End(e)) if e.local_name().as_ref() == b"t" => {
                t_depth = t_depth.saturating_sub(1);
                if t_depth == 0 {
                    let text = node.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !text.is_empty() {
                        texts.push(text);
                    }
                    node.clear();
                }
            }
            Ok(Event::Text(t)) if t_depth > 0 => {
                node.push_str(&t.unescape().map(|c| c.into_owned()).unwrap_or_default());
            }
            Ok(Event::CData(t)) if t_depth > 0 => {
                node.push_str(&String::from_utf8_lossy(&t.into_inner()));
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
    (!texts.is_empty()).then_some(GraphicContent::Diagram { texts })
}

/// The slide's relationships, split by how spoor consumes them: image
/// targets become `spoor://` placeholders (charset-safe subset only), and
/// chart / diagram-data parts are read back for native data extraction.
#[derive(Debug, Default)]
struct SlideRels {
    images: HashMap<String, String>,
    parts: HashMap<String, String>,
}

/// Build the rel maps for `slide_name`'s rels file. Targets are normalized
/// through `normalize_zip_path` so `../media/foo.png` (the form OOXML writes)
/// becomes `ppt/media/foo.png`. Other rels (notes, hyperlinks, …) are
/// filtered by relationship type.
fn slide_rel_targets<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    slide_name: &str,
    max_parse_bytes: usize,
) -> Result<SlideRels> {
    let Some(file_name) = Path::new(slide_name).file_name().and_then(|s| s.to_str()) else {
        return Ok(SlideRels::default());
    };
    let rels_name = format!("ppt/slides/_rels/{file_name}.rels");
    let Some(rels_xml) = limits::read_zip_text_optional(zip, &rels_name, max_parse_bytes)? else {
        return Ok(SlideRels::default());
    };
    let base = Path::new(slide_name)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    Ok(SlideRels {
        images: parse_image_rel_targets(&rels_xml, base),
        parts: parse_part_rel_targets(&rels_xml, base),
    })
}

/// `rId → ppt/charts/chartN.xml` / `ppt/diagrams/dataN.xml` for the rel types
/// whose parts spoor reads back (`…/chart`, `…/diagramData`). These paths are
/// never emitted into output — they are only looked up in the archive — so
/// normalization plus the archive read is the safety boundary.
fn parse_part_rel_targets(xml: &str, base: &Path) -> HashMap<String, String> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut map = HashMap::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if e.local_name().as_ref() == b"Relationship" =>
            {
                let rel_type = attr(&e, b"Type").unwrap_or_default();
                if !(rel_type.ends_with("/chart") || rel_type.ends_with("/diagramData")) {
                    buf.clear();
                    continue;
                }
                if attr(&e, b"TargetMode").as_deref() == Some("External") {
                    buf.clear();
                    continue;
                }
                if let (Some(id), Some(target)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                    map.insert(id, normalize_zip_path(base.join(target)));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    map
}

fn parse_image_rel_targets(xml: &str, base: &Path) -> HashMap<String, String> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut map = HashMap::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if e.local_name().as_ref() == b"Relationship" =>
            {
                let rel_type = attr(&e, b"Type").unwrap_or_default();
                if !rel_type.ends_with("/image") {
                    buf.clear();
                    continue;
                }
                let Some(id) = attr(&e, b"Id") else {
                    buf.clear();
                    continue;
                };
                let Some(target) = attr(&e, b"Target") else {
                    buf.clear();
                    continue;
                };
                let normalized = normalize_zip_path(base.join(target));
                // Emit a handle only for a path that will also pass the
                // extract-time OPC validator. This stops a crafted media
                // filename (markdown link syntax, spaces) from breaking out of /
                // injecting into the `](spoor://pptx/part/...)` placeholder link,
                // and guarantees every emitted handle round-trips through
                // `--extract`. A dropped rel leaves the blip unresolved, which
                // the per-slide "partial / unresolved" warning already surfaces.
                if crate::engine::safe_opc_media_subpath("ppt", &normalized) {
                    map.insert(id, normalized);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    map
}

/// Render speaker notes under the slide; returns whether anything was
/// emitted, so the no-text-layer warning can say notes still carried text out.
/// Notes-furniture placeholders (slide number, date, header/footer, slide
/// image) are template chrome, not authored content, and are filtered — the
/// Tika `skipPlaceholders` lesson: without this, every real-world deck leaks
/// a stray page-number digit into each slide's notes.
fn render_notes(xml: &str, max_parse_bytes: usize, md: &mut MarkdownBuilder) -> Result<bool> {
    let mut budget = NodeBudget::new(max_parse_bytes);
    let (_background, shapes) = parse_slide(xml, &mut budget)?;
    let mut paragraphs = Vec::new();
    collect_note_paragraphs(shapes, &mut paragraphs);
    if paragraphs.is_empty() {
        return Ok(false);
    }
    md.paragraph("Notes:");
    for paragraph in paragraphs {
        md.paragraph(&paragraph);
    }
    Ok(true)
}

/// Note text in XML order (notes are a single prose body; geometry sorting
/// and bullet markers would add nothing), with furniture placeholders
/// skipped and group shapes flattened.
fn collect_note_paragraphs(shapes: Vec<Shape>, sink: &mut Vec<String>) {
    for shape in shapes {
        match shape.body {
            ShapeBody::Text { ph, paras } => {
                if matches!(
                    ph.as_deref(),
                    Some("sldNum") | Some("dt") | Some("ftr") | Some("hdr") | Some("sldImg")
                ) {
                    continue;
                }
                for para in paras {
                    let text = para.text.trim();
                    if !text.is_empty() {
                        sink.push(text.to_string());
                    }
                }
            }
            // A table in the notes body is authored content too; flatten each
            // row so its text survives (notes stay prose, no GFM machinery).
            ShapeBody::Table(rows) => {
                for row in rows {
                    let line = row
                        .iter()
                        .filter(|cell| !cell.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !line.is_empty() {
                        sink.push(line);
                    }
                }
            }
            ShapeBody::Group(children) => collect_note_paragraphs(children, sink),
            _ => {}
        }
    }
}

fn notes_slide_for<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    slide_name: &str,
    max_parse_bytes: usize,
) -> Result<Option<String>> {
    let Some(file_name) = Path::new(slide_name).file_name().and_then(|s| s.to_str()) else {
        return Ok(None);
    };
    let rels_name = format!("ppt/slides/_rels/{file_name}.rels");
    let rels_xml = match limits::read_zip_text_optional(zip, &rels_name, max_parse_bytes)? {
        Some(xml) => xml,
        None => return Ok(None),
    };
    let Some(target) = parse_notes_target(&rels_xml) else {
        return Ok(None);
    };
    let base = Path::new(slide_name)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    Ok(Some(normalize_zip_path(base.join(target))))
}

fn parse_notes_target(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if e.local_name().as_ref() == b"Relationship" =>
            {
                let rel_type = attr(&e, b"Type")?;
                if rel_type.ends_with("/notesSlide") {
                    return attr(&e, b"Target");
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    None
}

fn normalize_zip_path(path: impl AsRef<Path>) -> String {
    let mut parts = Vec::new();
    for component in path.as_ref().components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::ParentDir => {
                parts.pop();
            }
            Component::CurDir => {}
            _ => {}
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod deck_order_tests {
    use super::{parse_slide_rel_targets, sld_id_list, slide_is_hidden};

    #[test]
    fn sld_id_list_reads_relationship_ids_in_document_order() {
        let xml = r#"<p:presentation xmlns:p="urn:p" xmlns:r="urn:r">
            <p:sldIdLst>
                <p:sldId id="258" r:id="rId4"/>
                <p:sldId id="256" r:id="rId2"/>
                <p:sldId id="257" r:id="rId3"/>
            </p:sldIdLst>
        </p:presentation>"#;
        assert_eq!(sld_id_list(xml).unwrap(), vec!["rId4", "rId2", "rId3"]);
    }

    #[test]
    fn sld_id_list_takes_the_prefixed_id_regardless_of_attribute_order() {
        // `id` (slide creation id) and `r:id` (relationship id) share the
        // local name `id`; the relationship id must win in either order.
        let before = r#"<p:presentation xmlns:p="urn:p" xmlns:r="urn:r">
            <p:sldIdLst><p:sldId r:id="rId9" id="256"/></p:sldIdLst>
        </p:presentation>"#;
        assert_eq!(sld_id_list(before).unwrap(), vec!["rId9"]);
        let after = r#"<p:presentation xmlns:p="urn:p" xmlns:r="urn:r">
            <p:sldIdLst><p:sldId id="256" r:id="rId9"/></p:sldIdLst>
        </p:presentation>"#;
        assert_eq!(sld_id_list(after).unwrap(), vec!["rId9"]);
    }

    #[test]
    fn sld_id_list_without_relationship_ids_requests_fallback() {
        let xml = r#"<p:presentation xmlns:p="urn:p">
            <p:sldIdLst><p:sldId id="256"/></p:sldIdLst>
        </p:presentation>"#;
        assert!(sld_id_list(xml).unwrap().is_empty());
    }

    #[test]
    fn slide_rel_targets_keep_only_slide_type_and_normalize() {
        let xml = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>
            <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/>
            <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="/ppt/slides/slide1.xml"/>
            <Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/>
        </Relationships>"#;
        let map = parse_slide_rel_targets(xml);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("rId2").map(String::as_str),
            Some("ppt/slides/slide2.xml")
        );
        // An absolute (package-rooted) target normalizes to the same shape.
        assert_eq!(
            map.get("rId3").map(String::as_str),
            Some("ppt/slides/slide1.xml")
        );
        assert!(!map.contains_key("rId1"), "slideMaster must be filtered");
    }

    #[test]
    fn external_mode_slide_rels_are_dropped() {
        // A crafted rels entry with a /slide type but TargetMode="External"
        // carries a URL, not a package part; it must never enter the map.
        let xml = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="http://evil.example/x" TargetMode="External"/>
            <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
        </Relationships>"#;
        let map = parse_slide_rel_targets(xml);
        assert_eq!(map.len(), 1);
        assert!(!map.contains_key("rId1"));
    }

    #[test]
    fn foreign_prefixed_id_cannot_shadow_the_relationships_id() {
        // A crafted foo:id before r:id must not win; a lone renamed prefix
        // still resolves.
        let shadowed = r#"<p:presentation xmlns:p="urn:p" xmlns:r="urn:r" xmlns:foo="urn:foo">
            <p:sldIdLst><p:sldId foo:id="evil" id="256" r:id="rId7"/></p:sldIdLst>
        </p:presentation>"#;
        assert_eq!(sld_id_list(shadowed).unwrap(), vec!["rId7"]);
        let renamed = r#"<p:presentation xmlns:p="urn:p" xmlns:rel="urn:r">
            <p:sldIdLst><p:sldId id="256" rel:id="rId7"/></p:sldIdLst>
        </p:presentation>"#;
        assert_eq!(sld_id_list(renamed).unwrap(), vec!["rId7"]);
    }

    #[test]
    fn sld_id_list_skips_mc_fallback_branches() {
        // Collecting both Choice and Fallback would double the reference and
        // trip the duplicate check downstream; only the Choice branch counts.
        let xml = r#"<p:presentation xmlns:p="urn:p" xmlns:r="urn:r" xmlns:mc="urn:mc">
            <p:sldIdLst><mc:AlternateContent>
                <mc:Choice><p:sldId id="256" r:id="rId7"/></mc:Choice>
                <mc:Fallback><p:sldId id="256" r:id="rId7"/></mc:Fallback>
            </mc:AlternateContent></p:sldIdLst>
        </p:presentation>"#;
        assert_eq!(sld_id_list(xml).unwrap(), vec!["rId7"]);
    }

    #[test]
    fn hidden_flag_is_read_from_the_root_element_only() {
        assert!(slide_is_hidden(r#"<p:sld xmlns:p="urn:p" show="0"><p:cSld/></p:sld>"#).unwrap());
        assert!(slide_is_hidden(r#"<p:sld xmlns:p="urn:p" show="false"/>"#).unwrap());
        assert!(!slide_is_hidden(r#"<p:sld xmlns:p="urn:p"><p:cSld/></p:sld>"#).unwrap());
        assert!(!slide_is_hidden(r#"<p:sld xmlns:p="urn:p" show="1"/>"#).unwrap());
    }
}

#[cfg(test)]
mod shape_render_tests {
    use super::*;
    use crate::output::MarkdownBuilder;

    fn render_with_body(xml: &str) -> (String, SlideBody) {
        let mut md = MarkdownBuilder::new();
        let mut image_number = 0usize;
        let mut emission = SlideImageEmission::default();
        let mut tally = GraphicTally::default();
        let rels = HashMap::new();
        let graphics = HashMap::new();
        let mut budget = NodeBudget::new(usize::MAX);
        let (background_blips, shapes) = parse_slide(xml, &mut budget).unwrap();
        let body = render_slide(
            background_blips,
            shapes,
            1,
            &rels,
            &graphics,
            &mut image_number,
            &mut emission,
            &mut tally,
            &mut md,
        );
        (md.build().unwrap(), body)
    }

    fn render(xml: &str) -> String {
        render_with_body(xml).0
    }

    const NS: &str = r#"xmlns:p="urn:p" xmlns:a="urn:a" xmlns:r="urn:r""#;

    fn sp(ph: &str, off: &str, paras: &str) -> String {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="1" name="s"/>{ph}</p:nvSpPr><p:spPr>{off}</p:spPr><p:txBody>{paras}</p:txBody></p:sp>"#
        )
    }

    #[test]
    fn shapes_render_top_to_bottom_then_left_to_right() {
        // XML order is bottom row, top-right, top-left; visual order must win.
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>
                {}{}{}
            </p:spTree></p:cSld></p:sld>"#,
            sp(
                "",
                r#"<a:xfrm><a:off x="900" y="5000"/></a:xfrm>"#,
                "<a:p><a:t>bottom</a:t></a:p>"
            ),
            sp(
                "",
                r#"<a:xfrm><a:off x="6000" y="1000"/></a:xfrm>"#,
                "<a:p><a:t>top right</a:t></a:p>"
            ),
            sp(
                "",
                r#"<a:xfrm><a:off x="900" y="1000"/></a:xfrm>"#,
                "<a:p><a:t>top left</a:t></a:p>"
            ),
        );
        let out = render(&xml);
        let tl = out.find("top left").unwrap();
        let tr = out.find("top right").unwrap();
        let bottom = out.find("bottom").unwrap();
        assert!(tl < tr && tr < bottom, "visual order expected, got:\n{out}");
    }

    #[test]
    fn title_placeholder_leads_and_becomes_a_heading() {
        // The title sits geometrically below the body but must lead as `###`.
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>
                {}{}
            </p:spTree></p:cSld></p:sld>"#,
            sp(
                "",
                r#"<a:xfrm><a:off x="0" y="0"/></a:xfrm>"#,
                "<a:p><a:t>a floating box</a:t></a:p>"
            ),
            sp(
                r#"<p:ph type="title"/>"#,
                r#"<a:xfrm><a:off x="0" y="9000"/></a:xfrm>"#,
                "<a:p><a:t>The Title</a:t></a:p>"
            ),
        );
        let out = render(&xml);
        assert!(out.starts_with("### The Title\n"), "got:\n{out}");
    }

    #[test]
    fn bullets_follow_placeholder_defaults_and_explicit_props() {
        let paras = concat!(
            "<a:p><a:t>plain default bullet</a:t></a:p>",
            r#"<a:p><a:pPr lvl="1"/><a:t>nested bullet</a:t></a:p>"#,
            r#"<a:p><a:pPr><a:buAutoNum type="arabicPeriod"/></a:pPr><a:t>numbered</a:t></a:p>"#,
            r#"<a:p><a:pPr><a:buNone/></a:pPr><a:t>prose opt-out</a:t></a:p>"#,
        );
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>{}</p:spTree></p:cSld></p:sld>"#,
            sp(r#"<p:ph idx="1"/>"#, "", paras),
        );
        let out = render(&xml);
        assert!(out.contains("- plain default bullet\n"), "got:\n{out}");
        assert!(out.contains("    - nested bullet\n"), "got:\n{out}");
        assert!(out.contains("1. numbered\n"), "got:\n{out}");
        assert!(out.contains("\n\nprose opt-out\n"), "got:\n{out}");
    }

    #[test]
    fn text_boxes_default_to_prose_not_bullets() {
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>{}</p:spTree></p:cSld></p:sld>"#,
            sp("", "", "<a:p><a:t>just prose</a:t></a:p>"),
        );
        let out = render(&xml);
        assert!(out.contains("just prose"));
        assert!(!out.contains("- just prose"));
    }

    #[test]
    fn groups_flatten_and_sort_as_one_unit() {
        // The group sits above the lone box; children keep local order.
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>
                {}
                <p:grpSp><p:grpSpPr><a:xfrm><a:off x="100" y="100"/><a:chOff x="0" y="0"/></a:xfrm></p:grpSpPr>
                    {}{}
                </p:grpSp>
            </p:spTree></p:cSld></p:sld>"#,
            sp(
                "",
                r#"<a:xfrm><a:off x="100" y="8000"/></a:xfrm>"#,
                "<a:p><a:t>after group</a:t></a:p>"
            ),
            sp(
                "",
                r#"<a:xfrm><a:off x="0" y="900"/></a:xfrm>"#,
                "<a:p><a:t>grouped beta</a:t></a:p>"
            ),
            sp(
                "",
                r#"<a:xfrm><a:off x="0" y="100"/></a:xfrm>"#,
                "<a:p><a:t>grouped alpha</a:t></a:p>"
            ),
        );
        let out = render(&xml);
        let alpha = out.find("grouped alpha").unwrap();
        let beta = out.find("grouped beta").unwrap();
        let after = out.find("after group").unwrap();
        assert!(alpha < beta && beta < after, "got:\n{out}");
    }

    #[test]
    fn notes_furniture_placeholders_are_filtered() {
        let xml = format!(
            r#"<p:notes {NS}><p:cSld><p:spTree>
                {}{}{}
            </p:spTree></p:cSld></p:notes>"#,
            sp(r#"<p:ph type="sldImg"/>"#, "", ""),
            sp(
                r#"<p:ph type="body" idx="1"/>"#,
                "",
                "<a:p><a:t>real notes</a:t></a:p>"
            ),
            sp(
                r#"<p:ph type="sldNum" idx="10"/>"#,
                "",
                "<a:p><a:fld><a:t>12</a:t></a:fld></a:p>"
            ),
        );
        let mut md = MarkdownBuilder::new();
        assert!(render_notes(&xml, usize::MAX, &mut md).unwrap());
        let out = md.build().unwrap();
        assert!(out.contains("real notes"));
        assert!(!out.contains("12"), "page-number furniture leaked: {out}");
    }

    #[test]
    fn title_only_image_slides_still_count_as_no_text_layer() {
        // The real-world "pure image" slide: a title placeholder plus a
        // full-bleed screenshot. The title must not fake a text layer —
        // otherwise this page is indistinguishable from a genuine text+image
        // page and never gets routed to a VLM.
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>
                {}
                <p:pic><p:nvPicPr><p:cNvPr id="9" name="img"/></p:nvPicPr>
                <p:blipFill><a:blip r:embed="rId9"/></p:blipFill></p:pic>
            </p:spTree></p:cSld></p:sld>"#,
            sp(r#"<p:ph type="title"/>"#, "", "<a:p><a:t>系统架构图</a:t></a:p>"),
        );
        let (out, body) = render_with_body(&xml);
        assert!(out.contains("### 系统架构图"), "title still renders: {out}");
        assert!(
            !body.saw_body_text,
            "a bare title must not count as body text"
        );
    }

    #[test]
    fn timing_text_is_neither_output_nor_counted_as_body_text() {
        // <p:timing> animation data contains text nodes (attrName values); the
        // flat pre-P2 renderer leaked them into output and let them suppress
        // the no-text-layer posture. The shape-tree parser only reads txBody.
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>
                <p:pic><p:nvPicPr><p:cNvPr id="9" name="img"/></p:nvPicPr>
                <p:blipFill><a:blip r:embed="rId9"/></p:blipFill></p:pic>
            </p:spTree></p:cSld><p:timing><p:attrName>ppt_x</p:attrName></p:timing></p:sld>"#
        );
        let (out, body) = render_with_body(&xml);
        assert!(!out.contains("ppt_x"), "timing text leaked: {out}");
        assert!(!body.saw_body_text, "timing text must not count as body text");
    }

    #[test]
    fn cdata_text_is_extracted_and_counts_as_body_text() {
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>{}</p:spTree></p:cSld></p:sld>"#,
            sp("", "", "<a:p><a:t><![CDATA[Hello CDATA]]></a:t></a:p>"),
        );
        let (out, body) = render_with_body(&xml);
        assert!(out.contains("Hello CDATA"), "got:\n{out}");
        assert!(body.saw_body_text);
    }

    #[test]
    fn alternate_content_renders_choice_not_fallback() {
        // PowerPoint wraps equations/chartex/WordArt in mc:AlternateContent;
        // collecting both branches would render the same content twice and
        // double-count blips in the visuals warning.
        let xml = format!(
            r#"<p:sld {NS} xmlns:mc="urn:mc"><p:cSld><p:spTree>
                <mc:AlternateContent>
                    <mc:Choice Requires="a14">{}</mc:Choice>
                    <mc:Fallback>{}</mc:Fallback>
                </mc:AlternateContent>
            </p:spTree></p:cSld></p:sld>"#,
            sp("", "", "<a:p><a:t>modern text</a:t></a:p>"),
            sp("", "", "<a:p><a:t>fallback text</a:t></a:p>"),
        );
        let out = render(&xml);
        assert!(out.contains("modern text"), "got:\n{out}");
        assert!(!out.contains("fallback text"), "got:\n{out}");
    }

    #[test]
    fn soft_breaks_keep_runs_apart() {
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>{}</p:spTree></p:cSld></p:sld>"#,
            sp(
                "",
                "",
                "<a:p><a:r><a:t>2024 revenue</a:t></a:r><a:br/><a:r><a:t>up 12%</a:t></a:r></a:p>"
            ),
        );
        let out = render(&xml);
        assert!(out.contains("2024 revenue up 12%"), "got:\n{out}");
    }

    #[test]
    fn pretty_printed_xml_keeps_headings_on_one_line() {
        // Inter-element whitespace (third-party pretty-printed XML) must not
        // end up inside the heading text and break it across lines.
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>{}</p:spTree></p:cSld></p:sld>"#,
            sp(
                r#"<p:ph type="title"/>"#,
                "",
                "<a:p>\n  <a:r><a:t>Hello</a:t></a:r>\n  <a:r><a:t>World</a:t></a:r>\n</a:p>"
            ),
        );
        let out = render(&xml);
        assert_eq!(out.lines().next(), Some("### HelloWorld"), "got:\n{out}");
    }

    #[test]
    fn offset_less_placeholders_precede_negative_coordinate_shapes() {
        // An author's off-canvas scratch box (negative offset) must not cut
        // in front of layout-inherited (offset-less) content.
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>
                {}{}
            </p:spTree></p:cSld></p:sld>"#,
            sp(
                "",
                r#"<a:xfrm><a:off x="-100" y="-9000"/></a:xfrm>"#,
                "<a:p><a:t>offcanvas scratch</a:t></a:p>"
            ),
            sp(
                r#"<p:ph idx="1"/>"#,
                "",
                "<a:p><a:t>inherited body</a:t></a:p>"
            ),
        );
        let out = render(&xml);
        let body = out.find("inherited body").unwrap();
        let scratch = out.find("offcanvas scratch").unwrap();
        assert!(body < scratch, "got:\n{out}");
    }

    #[test]
    fn node_budget_bounds_crafted_shape_floods() {
        // Megabytes of empty shapes must hit the parse budget as a structured
        // error instead of ballooning into resident structs.
        let body = "<p:sp></p:sp>".repeat(5000);
        let xml = format!(
            r#"<p:sld xmlns:p="urn:p"><p:cSld><p:spTree>{body}</p:spTree></p:cSld></p:sld>"#
        );
        let mut budget = NodeBudget::new(1);
        assert!(parse_slide(&xml, &mut budget).is_err());
        // The same deck parses fine under the default-sized budget.
        let mut roomy = NodeBudget::new(64 * 1024 * 1024);
        assert!(parse_slide(&xml, &mut roomy).is_ok());
    }

    #[test]
    fn unknown_entities_degrade_to_raw_text_instead_of_dropping_the_chunk() {
        let xml = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree>{}</p:spTree></p:cSld></p:sld>"#,
            sp("", "", "<a:p><a:t>Q&nbsp;A</a:t></a:p>"),
        );
        let (out, body) = render_with_body(&xml);
        assert!(out.contains('Q') && out.contains('A'), "got:\n{out}");
        assert!(body.saw_body_text, "surviving text must count as body text");
    }

    #[test]
    fn notes_tables_are_not_dropped() {
        let xml = format!(
            r#"<p:notes {NS}><p:cSld><p:spTree>
                <p:graphicFrame><a:graphic>
                    <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table">
                        <a:tbl><a:tr><a:tc><a:txBody><a:p><a:t>metric</a:t></a:p></a:txBody></a:tc>
                        <a:tc><a:txBody><a:p><a:t>42</a:t></a:p></a:txBody></a:tc></a:tr></a:tbl>
                    </a:graphicData>
                </a:graphic></p:graphicFrame>
            </p:spTree></p:cSld></p:notes>"#
        );
        let mut md = MarkdownBuilder::new();
        assert!(render_notes(&xml, usize::MAX, &mut md).unwrap());
        let out = md.build().unwrap();
        assert!(out.contains("metric 42"), "got:\n{out}");
    }

    #[test]
    fn chart_part_data_becomes_a_table() {
        let xml = r#"<c:chartSpace xmlns:c="urn:c" xmlns:a="urn:a">
          <c:chart>
            <c:title><c:tx><c:rich><a:p><a:r><a:t>Quarterly</a:t></a:r></a:p></c:rich></c:tx></c:title>
            <c:plotArea><c:barChart>
              <c:ser>
                <c:tx><c:strRef><c:strCache><c:pt idx="0"><c:v>Revenue</c:v></c:pt></c:strCache></c:strRef></c:tx>
                <c:dLbls><c:tx><c:rich><a:p><a:r><a:t>noise</a:t></a:r></a:p></c:rich></c:tx></c:dLbls>
                <c:cat><c:strRef><c:strCache>
                  <c:pt idx="0"><c:v>Q1</c:v></c:pt><c:pt idx="1"><c:v>Q2</c:v></c:pt>
                </c:strCache></c:strRef></c:cat>
                <c:val><c:numRef><c:numCache><c:formatCode>General</c:formatCode>
                  <c:pt idx="0"><c:v>100</c:v></c:pt><c:pt idx="1"><c:v>120</c:v></c:pt>
                </c:numCache></c:numRef></c:val>
              </c:ser>
              <c:ser>
                <c:tx><c:strRef><c:strCache><c:pt idx="0"><c:v>Cost</c:v></c:pt></c:strCache></c:strRef></c:tx>
                <c:val><c:numRef><c:numCache>
                  <c:pt idx="0"><c:v>80</c:v></c:pt><c:pt idx="1"><c:v>90</c:v></c:pt>
                </c:numCache></c:numRef></c:val>
              </c:ser>
            </c:barChart></c:plotArea>
          </c:chart>
        </c:chartSpace>"#;
        let Some(GraphicContent::Chart { title, table, note }) = parse_chart(xml) else {
            panic!("expected chart content");
        };
        assert_eq!(title.as_deref(), Some("Quarterly"));
        assert!(note.is_none());
        assert_eq!(
            table,
            vec![
                vec!["".to_string(), "Revenue".to_string(), "Cost".to_string()],
                vec!["Q1".to_string(), "100".to_string(), "80".to_string()],
                vec!["Q2".to_string(), "120".to_string(), "90".to_string()],
            ],
            "dLbls text must not leak into names; second series reuses Q1/Q2 labels"
        );
    }

    #[test]
    fn chart_without_cached_values_stays_unresolved() {
        assert!(
            parse_chart(r#"<c:chartSpace xmlns:c="urn:c"><c:chart/></c:chartSpace>"#).is_none()
        );
    }

    #[test]
    fn diagram_part_text_becomes_node_labels() {
        let xml = r#"<dgm:dataModel xmlns:dgm="urn:dgm" xmlns:a="urn:a">
          <dgm:ptLst>
            <dgm:pt modelId="0" type="doc"/>
            <dgm:pt modelId="1"><dgm:t><a:p><a:r><a:t>Plan</a:t></a:r></a:p></dgm:t></dgm:pt>
            <dgm:pt modelId="2"><dgm:t><a:p><a:r><a:t>Bu</a:t></a:r><a:r><a:t>ild</a:t></a:r></a:p></dgm:t></dgm:pt>
            <dgm:pt modelId="3" type="parTrans"><dgm:t><a:p/></dgm:t></dgm:pt>
            <dgm:pt modelId="4"><dgm:t><a:p><a:r><a:t>Ship</a:t></a:r></a:p></dgm:t></dgm:pt>
          </dgm:ptLst>
        </dgm:dataModel>"#;
        let Some(GraphicContent::Diagram { texts }) = parse_diagram(xml) else {
            panic!("expected diagram content");
        };
        assert_eq!(texts, vec!["Plan", "Build", "Ship"]);
    }

    #[test]
    fn graphic_frames_classify_chart_diagram_and_opaque() {
        let chart = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree><p:graphicFrame><a:graphic>
                <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart">
                    <c:chart xmlns:c="urn:c" r:id="rId4"/>
                </a:graphicData>
            </a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#
        );
        let (_, shapes) = parse_slide(&chart, &mut NodeBudget::new(usize::MAX)).unwrap();
        assert!(
            matches!(&shapes[0].body, ShapeBody::Chart { rid: Some(r) } if r == "rId4"),
            "got {:?}",
            shapes[0].body
        );

        let diagram = format!(
            r#"<p:sld {NS} xmlns:dgm="urn:dgm"><p:cSld><p:spTree><p:graphicFrame><a:graphic>
                <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram">
                    <dgm:relIds r:dm="rId7" r:lo="rId8" r:qs="rId9" r:cs="rId10"/>
                </a:graphicData>
            </a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#
        );
        let (_, shapes) = parse_slide(&diagram, &mut NodeBudget::new(usize::MAX)).unwrap();
        assert!(
            matches!(&shapes[0].body, ShapeBody::Diagram { rid: Some(r) } if r == "rId7"),
            "got {:?}",
            shapes[0].body
        );

        let ole = format!(
            r#"<p:sld {NS}><p:cSld><p:spTree><p:graphicFrame><a:graphic>
                <a:graphicData uri="http://schemas.openxmlformats.org/presentationml/2006/ole"/>
            </a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#
        );
        let (_, shapes) = parse_slide(&ole, &mut NodeBudget::new(usize::MAX)).unwrap();
        assert!(matches!(&shapes[0].body, ShapeBody::Opaque));
    }

    #[test]
    fn fully_unpacked_graphics_draw_no_visuals_warning() {
        use super::{GraphicTally, SlideFeatures, feature_warnings};
        // Scan saw a chart, but render unpacked it: the output is complete,
        // so no embedded_visuals_omitted fires.
        let features = SlideFeatures {
            merged_table: false,
            embedded_visuals: true,
        };
        let unpacked = GraphicTally {
            rendered: 1,
            opaque: 0,
        };
        assert!(feature_warnings(1, features, SlideImageEmission::default(), unpacked).is_empty());
        // One chart unpacked, one OLE opaque: the warning stays.
        let partial = GraphicTally {
            rendered: 1,
            opaque: 1,
        };
        let warnings = feature_warnings(1, features, SlideImageEmission::default(), partial);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("未能解构"));
    }

    #[test]
    fn alt_text_is_sanitized_against_link_injection() {
        assert_eq!(
            sanitize_alt("Quarterly revenue chart ](evil) [x]").as_deref(),
            Some("Quarterly revenue chart evil x")
        );
        assert_eq!(sanitize_alt("  \n\t ()[]").as_deref(), None);
        let long = "字".repeat(500);
        assert_eq!(sanitize_alt(&long).unwrap().chars().count(), 120);
    }
}

#[cfg(test)]
mod feature_warning_tests {
    use super::{
        GraphicTally, SlideFeatures, SlideImageEmission, feature_warnings, parse_image_rel_targets,
        scan_slide_features,
    };
    use crate::result::WarningLocation;
    use std::path::Path;

    #[test]
    fn non_table_graphic_data_counts_as_embedded_visuals() {
        // A SmartArt-only slide must not degrade to a bare heading with no
        // posture signal: its graphicData uri (diagram) marks it visual.
        let smartart = scan_slide_features(
            r#"<p:sld xmlns:p="urn:p" xmlns:a="urn:a"><p:graphicFrame><a:graphic>
                <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram"/>
            </a:graphic></p:graphicFrame></p:sld>"#,
        )
        .unwrap();
        assert!(smartart.embedded_visuals);
        // A plain table graphicFrame is extracted natively — not a visual.
        let table = scan_slide_features(
            r#"<p:sld xmlns:p="urn:p" xmlns:a="urn:a"><p:graphicFrame><a:graphic>
                <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"/>
            </a:graphic></p:graphicFrame></p:sld>"#,
        )
        .unwrap();
        assert!(!table.embedded_visuals);
    }

    #[test]
    fn detects_merged_cells_and_visuals() {
        let features = scan_slide_features(
            r#"<p:sld xmlns:p="urn:p" xmlns:a="urn:a"><a:tc gridSpan="2"/><p:pic/></p:sld>"#,
        )
        .unwrap();

        assert_eq!(
            features,
            SlideFeatures {
                merged_table: true,
                embedded_visuals: true,
            }
        );
        // No blips actually rendered: emission stays zero, surfacing the
        // "chart/OLE-only" wording rather than the false "spoor 未解析" one.
        let warnings = feature_warnings(
            3,
            features,
            SlideImageEmission::default(),
            GraphicTally::default(),
        );
        assert_eq!(warnings.len(), 2);
        assert_eq!(
            warnings[0].location,
            Some(WarningLocation::Slide { number: 3 })
        );
        assert!(
            warnings[1].message.contains("无位图"),
            "expected chart/OLE wording, got {:?}",
            warnings[1].message
        );
    }

    #[test]
    fn fully_marked_visuals_get_extract_wording() {
        let features = SlideFeatures {
            merged_table: false,
            embedded_visuals: true,
        };
        let emission = SlideImageEmission {
            total_blips: 2,
            emitted_handles: 2,
        };
        let warnings = feature_warnings(1, features, emission, GraphicTally::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("spoor://pptx/part/"));
        assert!(warnings[0].message.contains("--extract"));
    }

    #[test]
    fn partially_marked_visuals_surface_gap() {
        let features = SlideFeatures {
            merged_table: false,
            embedded_visuals: true,
        };
        let emission = SlideImageEmission {
            total_blips: 3,
            emitted_handles: 1,
        };
        let warnings = feature_warnings(2, features, emission, GraphicTally::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("1 张可用"));
        assert!(warnings[0].message.contains("其余未能解析"));
    }

    #[test]
    fn parses_image_rels_and_normalizes_relative_paths() {
        // Real PPTX rels: image targets are written as `../media/imageN.png`
        // relative to `ppt/slides/`. `parse_image_rel_targets` must resolve
        // them to a canonical `ppt/media/imageN.png` ZIP entry, drop
        // non-image rels, and key on rId.
        let xml = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
            <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide1.xml"/>
            <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image2.jpeg"/>
        </Relationships>"#;
        let map = parse_image_rel_targets(xml, Path::new("ppt/slides"));
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("rId1").map(String::as_str),
            Some("ppt/media/image1.png")
        );
        assert_eq!(
            map.get("rId3").map(String::as_str),
            Some("ppt/media/image2.jpeg")
        );
        assert!(!map.contains_key("rId2"), "non-image rel must be skipped");
    }

    #[test]
    fn unsafe_media_filenames_are_dropped_not_emitted_as_handles() {
        // A crafted media filename with markdown link syntax or spaces must not
        // become a placeholder target — otherwise it breaks out of / injects
        // into the `](spoor://pptx/part/...)` link in agent-facing output. Only
        // charset-safe `ppt/media/<name>` paths (which also pass --extract) emit.
        let xml = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/evil) [x](http://e).png"/>
            <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/a b.png"/>
            <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
        </Relationships>"#;
        let map = parse_image_rel_targets(xml, Path::new("ppt/slides"));
        assert!(
            !map.contains_key("rId1"),
            "markdown-injection filename must be dropped"
        );
        assert!(
            !map.contains_key("rId2"),
            "filename with a space must be dropped"
        );
        assert_eq!(
            map.get("rId3").map(String::as_str),
            Some("ppt/media/image1.png"),
            "the safe filename still emits a handle"
        );
    }
}
