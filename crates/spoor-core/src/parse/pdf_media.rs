//! PDF embedded-image discovery and single-image extraction.
//!
//! pith does not render or decode PDF pixels. This module only:
//!   1. locates image XObjects per page so the renderer can mark their
//!      position and warn (the agent learns an image is there), and
//!   2. hands back the raw bytes of an image whose stream is already a usable
//!      file (JPEG via `DCTDecode`, JPEG 2000 via `JPXDecode`) when an agent
//!      asks via `spoor://pdf/obj/{id}/{gen}`.
//!
//! Images in other encodings (`FlateDecode` raster, `CCITTFax`, `JBIG2`, …)
//! need real pixel decoding plus a PNG re-encode, which is out of scope; they
//! are surfaced as a position marker plus a warning but carry no extractable
//! handle. Discovery is best-effort: a malformed PDF yields no images rather
//! than failing the document, which is still rendered from its text layer.

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::HashSet;

const MAX_PAGE_TREE_DEPTH: usize = 32;
/// Bound on `Do` recursion through nested Form XObjects.
const MAX_XOBJECT_DEPTH: usize = 16;

/// One image XObject found on a page.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PageImage {
    pub(crate) id: u32,
    pub(crate) generation: u16,
    /// Whether [`extract_image`] can return a usable file for this image
    /// (JPEG/JPEG2000 verbatim, or an 8-bit gray/RGB raster re-encoded as PNG).
    /// When false the renderer emits a marker without an extractable handle.
    pub(crate) extractable: bool,
}

/// Why a `spoor://pdf/obj/` image could not be returned as bytes.
#[derive(Debug)]
pub(crate) enum ImageExtractError {
    Unreadable,
    NotFound,
    NotAnImage,
    /// Codec/colorspace pith does not turn into a usable file (CMYK, indexed,
    /// CCITTFax, JBIG2, non-8-bit, …). The string names what was seen.
    UnsupportedEncoding(String),
    /// Decoding the raster would exceed the parse budget; `needed` is the raw
    /// size in bytes the consumer must allow via `--max-parse-mib`.
    TooLarge(usize),
}

impl ImageExtractError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Unreadable => "无法解析该 PDF。".to_string(),
            Self::NotFound => "spoor://pdf/obj/ 指向的对象不存在。".to_string(),
            Self::NotAnImage => "spoor://pdf/obj/ 指向的对象不是图片。".to_string(),
            Self::UnsupportedEncoding(detail) => {
                format!("该图片编码或色彩空间（{detail}）无法直接导出；需外部渲染后交 VLM。")
            }
            Self::TooLarge(needed) => format!(
                "该图片解码后约 {needed} 字节，超过解析上限；可调高 --max-parse-mib 后重试，或在外部渲染该页。"
            ),
        }
    }
}

/// Image lists per page, indexed to match the text engine's page order
/// (index 0 = page 1). A load failure yields all-empty lists.
#[cfg(test)]
pub(crate) fn discover_images(bytes: &[u8], page_count: usize) -> Vec<Vec<PageImage>> {
    let Ok(doc) = Document::load_mem(bytes) else {
        return vec![Vec::new(); page_count];
    };
    discover_images_from_doc(&doc, page_count, None)
}

/// Image discovery over an already-loaded document — the shared-`Document` entry
/// point for the single-parse path.
pub(crate) fn discover_images_from_doc(
    doc: &Document,
    page_count: usize,
    page_range: Option<(usize, usize)>,
) -> Vec<Vec<PageImage>> {
    let mut per_page = vec![Vec::new(); page_count];
    let mut selected_index = 0usize;
    for (page_num, page_id) in doc.get_pages() {
        let page_number = page_num as usize;
        if let Some((first, last)) = page_range {
            if page_number < first || page_number > last {
                continue;
            }
        }
        let Some(slot) = per_page.get_mut(selected_index) else {
            break;
        };
        collect_page_images(doc, page_id, slot);
        selected_index += 1;
    }
    per_page
}

/// Return a directly-usable image file for `obj/{id}/{gen}`, or an error
/// explaining why it cannot be produced. Pure: reads only `bytes`.
///
/// `DCTDecode`/`JPXDecode` streams are already JPEG/JPEG2000 files and are
/// returned verbatim. `FlateDecode`/`LZWDecode` rasters in Gray or RGB at 8
/// bits/component are decoded and re-wrapped as PNG (using the `flate2` already
/// in the tree — no new dependency). Everything else degrades to an error so
/// the page keeps its marker and the consumer renders it externally.
pub(crate) fn extract_image(
    bytes: &[u8],
    id: u32,
    generation: u16,
    max_parse_bytes: usize,
) -> Result<Vec<u8>, ImageExtractError> {
    let doc = Document::load_mem(bytes).map_err(|_| ImageExtractError::Unreadable)?;
    let object = doc
        .get_object((id, generation))
        .map_err(|_| ImageExtractError::NotFound)?;
    let Object::Stream(stream) = object else {
        return Err(ImageExtractError::NotAnImage);
    };
    // Re-validate the object really is an image XObject, exactly as the DOCX
    // path re-checks the media path: never trust the URI to address arbitrary
    // objects just because spoor emitted it.
    if dict_name(&doc, &stream.dict, b"Subtype") != Some(&b"Image"[..]) {
        return Err(ImageExtractError::NotAnImage);
    }

    let filter = effective_filter(&doc, &stream.dict);
    if matches!(filter, Some(f) if f == b"DCTDecode" || f == b"JPXDecode") {
        return Ok(stream.content.clone());
    }

    // Raster path: decode the samples and wrap them as PNG.
    let is_raster = matches!(filter, Some(f) if f == b"FlateDecode" || f == b"LZWDecode");
    let color = image_color(&doc, &stream.dict);
    let bits = bits_per_component(&stream.dict);
    let (Some(color), Some(8)) = (color, bits) else {
        return Err(unsupported(&doc, filter, &stream.dict));
    };
    if !is_raster {
        return Err(unsupported(&doc, filter, &stream.dict));
    }

    let width = dimension(&stream.dict, b"Width").ok_or(ImageExtractError::NotAnImage)?;
    let height = dimension(&stream.dict, b"Height").ok_or(ImageExtractError::NotAnImage)?;
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(color.components()))
        .ok_or(ImageExtractError::NotAnImage)?;
    if expected > max_parse_bytes {
        return Err(ImageExtractError::TooLarge(expected));
    }

    // lopdf's `decompressed_content` rejects image streams, so inflate the
    // FlateDecode bytes ourselves (flate2 is already in the tree) and undo any
    // PNG/TIFF predictor before handing the raw samples to the PNG encoder.
    let samples = decode_raster_samples(&doc, stream, color, width, height, expected)?;
    Ok(raster_to_png(width as u32, height as u32, color, &samples))
}

fn decode_raster_samples(
    doc: &Document,
    stream: &lopdf::Stream,
    color: PngColor,
    width: usize,
    height: usize,
    expected: usize,
) -> Result<Vec<u8>, ImageExtractError> {
    let filter = effective_filter(doc, &stream.dict);
    if !matches!(filter, Some(f) if f == b"FlateDecode") {
        // LZWDecode and friends are rarer; degrade rather than grow the matrix.
        return Err(unsupported(doc, filter, &stream.dict));
    }

    // Cap the inflate near the expected raster (plus one filter byte per PNG row)
    // so a malformed stream cannot balloon allocation past the budgeted size.
    let cap = expected.saturating_add(height).saturating_add(64);
    let inflated = inflate_zlib(&stream.content, cap)?;

    let colors = color.components();
    let samples = match predictor(doc, &stream.dict) {
        1 => inflated,
        2 => tiff_unpredict(inflated, width, colors),
        p if (10..=15).contains(&p) => png_unpredict(inflated, width, colors),
        p => {
            return Err(ImageExtractError::UnsupportedEncoding(format!(
                "predictor {p}"
            )));
        }
    };
    if samples.len() < expected {
        return Err(ImageExtractError::UnsupportedEncoding(
            "样本数与宽高/色彩空间不符".to_owned(),
        ));
    }
    let mut samples = samples;
    samples.truncate(expected);
    Ok(samples)
}

fn inflate_zlib(content: &[u8], cap: usize) -> Result<Vec<u8>, ImageExtractError> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(content)
        .take(cap as u64)
        .read_to_end(&mut out)
        .map_err(|_| ImageExtractError::UnsupportedEncoding("FlateDecode 解压失败".to_owned()))?;
    Ok(out)
}

/// `/Predictor` from `/DecodeParms` (1 = none when absent).
fn predictor(doc: &Document, dict: &Dictionary) -> i64 {
    let parms = match dict.get(b"DecodeParms").or_else(|_| dict.get(b"DP")) {
        Ok(value) => resolve(doc, value),
        Err(_) => return 1,
    };
    let parms = match parms {
        Object::Dictionary(parms) => Some(parms),
        Object::Array(items) => items
            .iter()
            .rev()
            .find_map(|item| resolve(doc, item).as_dict().ok()),
        _ => None,
    };
    parms
        .and_then(|parms| parms.get(b"Predictor").ok())
        .and_then(|value| value.as_i64().ok())
        .unwrap_or(1)
}

/// Undo a TIFF horizontal-differencing predictor (8 bits/component).
fn tiff_unpredict(mut data: Vec<u8>, width: usize, colors: usize) -> Vec<u8> {
    let stride = width * colors;
    if stride == 0 {
        return data;
    }
    for row in data.chunks_mut(stride) {
        for i in colors..row.len() {
            row[i] = row[i].wrapping_add(row[i - colors]);
        }
    }
    data
}

/// Undo PNG per-row predictors (each input row is a filter byte + `stride`
/// bytes); 8 bits/component.
fn png_unpredict(data: Vec<u8>, width: usize, colors: usize) -> Vec<u8> {
    let stride = width * colors;
    if stride == 0 {
        return Vec::new();
    }
    let row_in = stride + 1;
    let rows = data.len() / row_in;
    let mut out = vec![0u8; rows * stride];
    for r in 0..rows {
        let input = &data[r * row_in..r * row_in + row_in];
        let kind = input[0];
        let filtered = &input[1..];
        let (before, current) = out.split_at_mut(r * stride);
        let current = &mut current[..stride];
        let above = r
            .checked_sub(1)
            .and_then(|prev| before.get(prev * stride..r * stride));
        for i in 0..stride {
            let a = if i >= colors { current[i - colors] } else { 0 };
            let b = above.map_or(0, |row| row[i]);
            let c = match above {
                Some(row) if i >= colors => row[i - colors],
                _ => 0,
            };
            let x = filtered[i];
            current[i] = match kind {
                1 => x.wrapping_add(a),
                2 => x.wrapping_add(b),
                3 => x.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => x.wrapping_add(paeth(a, b, c)),
                _ => x,
            };
        }
    }
    out
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (a as i32, b as i32, c as i32);
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

fn unsupported(doc: &Document, filter: Option<&[u8]>, dict: &Dictionary) -> ImageExtractError {
    let filter = filter
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .unwrap_or_else(|| "raw".to_owned());
    let colorspace = match dict.get(b"ColorSpace").ok().map(|cs| resolve(doc, cs)) {
        Some(Object::Name(name)) => String::from_utf8_lossy(name).into_owned(),
        Some(Object::Array(arr)) => arr
            .first()
            .and_then(|head| resolve(doc, head).as_name().ok())
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .unwrap_or_else(|| "?".to_owned()),
        _ => "?".to_owned(),
    };
    ImageExtractError::UnsupportedEncoding(format!("{filter}/{colorspace}"))
}

/// Collect the images actually *drawn* on a page. A page's `/Resources/XObject`
/// only lists what is *available* (often one inherited dictionary shared by
/// every page), so enumerating it reports the same objects on every page. The
/// truth is in the content stream: each `Do` paints a named XObject, and a Form
/// XObject's own content paints more. We walk that, resolving names through the
/// active resource scope and recursing into forms, deduplicating per page.
fn collect_page_images(doc: &Document, page_id: ObjectId, out: &mut Vec<PageImage>) {
    let resources = page_resources(doc, page_id);
    let content = page_content(doc, page_id);
    let mut seen = HashSet::new();
    walk_drawn_xobjects(doc, &content, resources, &mut seen, out, 0);
}

fn walk_drawn_xobjects(
    doc: &Document,
    content: &[u8],
    resources: Option<&Dictionary>,
    seen: &mut HashSet<ObjectId>,
    out: &mut Vec<PageImage>,
    depth: usize,
) {
    if depth > MAX_XOBJECT_DEPTH {
        return;
    }
    let Ok(decoded) = Content::decode(content) else {
        return;
    };
    for operation in decoded.operations {
        if operation.operator != "Do" {
            continue;
        }
        let Some(name) = operation.operands.first().and_then(|o| o.as_name().ok()) else {
            continue;
        };
        let Some(id) = xobject_id(doc, resources, name) else {
            continue;
        };
        let Ok(Object::Stream(stream)) = doc.get_object(id) else {
            continue;
        };
        match dict_name(doc, &stream.dict, b"Subtype") {
            Some(subtype) if subtype == b"Image" && seen.insert(id) => {
                out.push(PageImage {
                    id: id.0,
                    generation: id.1,
                    extractable: is_extractable_image(doc, &stream.dict),
                });
            }
            // Dedupe forms too, not just images: without this a form referenced
            // k times is re-decoded k times, so a crafted diamond of forms fans
            // out ~k^depth content-stream decodes before the depth cap stops it
            // (the work budget does not reach this discovery path). Trade-off: a
            // Resources-less form drawn under two different scopes is walked once
            // (first scope wins) — acceptable next to the work blowup.
            Some(subtype) if subtype == b"Form" && seen.insert(id) => {
                let form_content = stream
                    .decompressed_content()
                    .unwrap_or_else(|_| stream.content.clone());
                // A form inherits the caller's resources when it has none.
                let form_resources = stream
                    .dict
                    .get(b"Resources")
                    .ok()
                    .and_then(|r| resolve(doc, r).as_dict().ok())
                    .or(resources);
                walk_drawn_xobjects(doc, &form_content, form_resources, seen, out, depth + 1);
            }
            _ => {}
        }
    }
}

/// Resolve an XObject `name` to its object id within the active resource scope.
fn xobject_id(doc: &Document, resources: Option<&Dictionary>, name: &[u8]) -> Option<ObjectId> {
    let xobjects = resolve(doc, resources?.get(b"XObject").ok()?)
        .as_dict()
        .ok()?;
    xobjects.get(name).ok()?.as_reference().ok()
}

/// The page's content stream bytes (decompressed; concatenated if an array).
fn page_content(doc: &Document, page_id: ObjectId) -> Vec<u8> {
    let Ok(page) = doc.get_object(page_id).and_then(|object| object.as_dict()) else {
        return Vec::new();
    };
    let Ok(contents) = page.get(b"Contents") else {
        return Vec::new();
    };
    let mut buffer = Vec::new();
    match resolve(doc, contents) {
        Object::Stream(stream) => {
            buffer = stream
                .decompressed_content()
                .unwrap_or_else(|_| stream.content.clone());
        }
        Object::Array(items) => {
            for item in items {
                if let Object::Stream(stream) = resolve(doc, item) {
                    buffer.extend_from_slice(
                        &stream
                            .decompressed_content()
                            .unwrap_or_else(|_| stream.content.clone()),
                    );
                    buffer.push(b'\n');
                }
            }
        }
        _ => {}
    }
    buffer
}

/// Resolve a page's resource dictionary, walking up `/Parent` for the
/// inherited case where `/Resources` lives on a Pages tree node.
fn page_resources(doc: &Document, page_id: ObjectId) -> Option<&Dictionary> {
    let mut current = page_id;
    for _ in 0..MAX_PAGE_TREE_DEPTH {
        let dict = doc.get_object(current).ok()?.as_dict().ok()?;
        if let Ok(resources) = dict.get(b"Resources") {
            if let Ok(resources) = resolve(doc, resources).as_dict() {
                return Some(resources);
            }
        }
        current = dict
            .get(b"Parent")
            .ok()
            .and_then(|p| p.as_reference().ok())?;
    }
    None
}

/// Follow indirect references to the concrete object (bounded).
fn resolve<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
    let mut current = object;
    for _ in 0..MAX_PAGE_TREE_DEPTH {
        match current {
            Object::Reference(id) => match doc.get_object(*id) {
                Ok(next) => current = next,
                Err(_) => return current,
            },
            _ => return current,
        }
    }
    current
}

fn dict_name<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a [u8]> {
    resolve(doc, dict.get(key).ok()?).as_name().ok()
}

/// The image codec filter — the last entry when `/Filter` is a chain.
fn effective_filter<'a>(doc: &'a Document, dict: &'a Dictionary) -> Option<&'a [u8]> {
    match resolve(doc, dict.get(b"Filter").ok()?) {
        Object::Name(name) => Some(name),
        Object::Array(filters) => resolve(doc, filters.last()?).as_name().ok(),
        _ => None,
    }
}

/// Whether [`extract_image`] can return a usable file for this image XObject:
/// JPEG/JPEG2000 verbatim, or an 8-bit gray/RGB raster re-encodable as PNG.
fn is_extractable_image(doc: &Document, dict: &Dictionary) -> bool {
    match effective_filter(doc, dict) {
        Some(f) if f == b"DCTDecode" || f == b"JPXDecode" => true,
        Some(f) if f == b"FlateDecode" || f == b"LZWDecode" => {
            image_color(doc, dict).is_some() && bits_per_component(dict) == Some(8)
        }
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum PngColor {
    Gray,
    Rgb,
}

impl PngColor {
    fn components(self) -> usize {
        match self {
            PngColor::Gray => 1,
            PngColor::Rgb => 3,
        }
    }

    fn png_type(self) -> u8 {
        match self {
            PngColor::Gray => 0,
            PngColor::Rgb => 2,
        }
    }
}

/// Map a PDF image color space to a PNG color model, for the subset pith can
/// re-encode (gray / RGB). Calibrated/ICC variants map to their device
/// equivalents; indexed/CMYK/separation/unknown return `None` (degrade).
fn image_color(doc: &Document, dict: &Dictionary) -> Option<PngColor> {
    match resolve(doc, dict.get(b"ColorSpace").ok()?) {
        Object::Name(name) => device_color(name),
        Object::Array(space) => match resolve(doc, space.first()?).as_name().ok()? {
            b"CalGray" => Some(PngColor::Gray),
            b"CalRGB" => Some(PngColor::Rgb),
            b"ICCBased" => match resolve(doc, space.get(1)?) {
                Object::Stream(stream) => match stream.dict.get(b"N").ok()?.as_i64().ok()? {
                    1 => Some(PngColor::Gray),
                    3 => Some(PngColor::Rgb),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

fn device_color(name: &[u8]) -> Option<PngColor> {
    match name {
        b"DeviceGray" | b"G" => Some(PngColor::Gray),
        b"DeviceRGB" | b"RGB" => Some(PngColor::Rgb),
        _ => None,
    }
}

fn bits_per_component(dict: &Dictionary) -> Option<i64> {
    dict.get(b"BitsPerComponent").ok()?.as_i64().ok()
}

fn dimension(dict: &Dictionary, key: &[u8]) -> Option<usize> {
    usize::try_from(dict.get(key).ok()?.as_i64().ok()?)
        .ok()
        .filter(|&n| n > 0)
}

/// Wrap raw 8-bit gray/RGB samples (row-major, no padding) as a PNG. Uses the
/// `flate2` already in the tree for the zlib IDAT; the CRC is computed inline.
/// No image crate, so this stays small and WASM-safe.
fn raster_to_png(width: u32, height: u32, color: PngColor, samples: &[u8]) -> Vec<u8> {
    use std::io::Write;

    let row_len = width as usize * color.components();
    let mut filtered = Vec::with_capacity((row_len + 1) * height as usize);
    for row in samples.chunks_exact(row_len) {
        filtered.push(0); // PNG filter type 0 (None)
        filtered.extend_from_slice(row);
    }

    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = encoder.write_all(&filtered);
    let idat = encoder.finish().unwrap_or_default();

    let mut png = Vec::with_capacity(idat.len() + 64);
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, color.png_type(), 0, 0, 0]);
    png_chunk(&mut png, b"IHDR", &ihdr);
    png_chunk(&mut png, b"IDAT", &idat);
    png_chunk(&mut png, b"IEND", &[]);
    png
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc32::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// CRC-32/ISO-HDLC (the polynomial PNG and zlib share).
struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Self(0xFFFF_FFFF)
    }

    fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= byte as u32;
            for _ in 0..8 {
                self.0 = if self.0 & 1 != 0 {
                    (self.0 >> 1) ^ 0xEDB8_8320
                } else {
                    self.0 >> 1
                };
            }
        }
    }

    fn finish(self) -> u32 {
        !self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Object, Stream, dictionary};

    /// Build a one-page PDF whose single image XObject uses `filter`, and
    /// return the bytes plus the image's object id.
    fn build_pdf(filter: &str, image_bytes: &[u8]) -> (Vec<u8>, (u32, u16)) {
        let mut doc = Document::with_version("1.5");
        let image_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
                "Filter" => filter,
            },
            image_bytes.to_vec(),
        ));
        let resources_id = doc.add_object(dictionary! {
            "XObject" => dictionary! { "Im0" => image_id },
        });
        let content_id = doc.add_object(Stream::new(dictionary! {}, b"/Im0 Do".to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Resources" => resources_id,
            "Contents" => content_id,
        });
        let pages_id = doc.add_object(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        });
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut(page_id) {
            page.set("Parent", pages_id);
        }
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        let mut buf = Vec::new();
        doc.save_to(&mut buf).expect("save pdf");
        (buf, image_id)
    }

    fn image_dict(filter: &str) -> lopdf::Dictionary {
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 1,
            "Height" => 1,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
            "Filter" => filter,
        }
    }

    fn finish(mut doc: Document, root_pages: (u32, u16)) -> Vec<u8> {
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root_pages });
        doc.trailer.set("Root", catalog);
        let mut buf = Vec::new();
        doc.save_to(&mut buf).expect("save pdf");
        buf
    }

    /// Two pages that share ONE inherited Resources dictionary (on the Pages
    /// node) holding two images; each page's content draws a different one.
    fn build_shared_resources_two_pages() -> (Vec<u8>, (u32, u16), (u32, u16)) {
        let mut doc = Document::with_version("1.5");
        let img_a = doc.add_object(Stream::new(image_dict("DCTDecode"), b"AAAA".to_vec()));
        let img_b = doc.add_object(Stream::new(image_dict("DCTDecode"), b"BBBB".to_vec()));
        let shared = doc.add_object(dictionary! {
            "XObject" => dictionary! { "ImA" => img_a, "ImB" => img_b },
        });
        let content_a = doc.add_object(Stream::new(dictionary! {}, b"/ImA Do".to_vec()));
        let content_b = doc.add_object(Stream::new(dictionary! {}, b"/ImB Do".to_vec()));
        let page_a = doc.add_object(dictionary! { "Type" => "Page", "Contents" => content_a });
        let page_b = doc.add_object(dictionary! { "Type" => "Page", "Contents" => content_b });
        let pages = doc.add_object(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_a.into(), page_b.into()],
            "Count" => 2,
            "Resources" => shared,
        });
        for page in [page_a, page_b] {
            if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(page) {
                dict.set("Parent", pages);
            }
        }
        (finish(doc, pages), img_a, img_b)
    }

    /// A page whose content draws a Form XObject; the image lives inside the
    /// form's own content and resources.
    fn build_form_wrapped_image() -> (Vec<u8>, (u32, u16)) {
        let mut doc = Document::with_version("1.5");
        let img = doc.add_object(Stream::new(image_dict("DCTDecode"), b"FORM".to_vec()));
        let form_resources = doc.add_object(dictionary! {
            "XObject" => dictionary! { "Im0" => img },
        });
        let form = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![0i64.into(), 0i64.into(), 1i64.into(), 1i64.into()],
                "Resources" => form_resources,
            },
            b"/Im0 Do".to_vec(),
        ));
        let page_resources = doc.add_object(dictionary! {
            "XObject" => dictionary! { "Fm0" => form },
        });
        let content = doc.add_object(Stream::new(dictionary! {}, b"/Fm0 Do".to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Resources" => page_resources,
            "Contents" => content,
        });
        let pages = doc.add_object(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page.into()],
            "Count" => 1,
        });
        if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(page) {
            dict.set("Parent", pages);
        }
        (finish(doc, pages), img)
    }

    #[test]
    fn each_page_reports_only_the_image_it_draws_not_shared_resources() {
        // Regression: enumerating shared Resources reported BOTH images on
        // BOTH pages (and could collapse to one object id across a document).
        // Walking the content stream attributes each image to the page that
        // actually draws it.
        let (bytes, img_a, img_b) = build_shared_resources_two_pages();
        let images = discover_images(&bytes, 2);

        assert_eq!(images[0].len(), 1, "page 1 must report only its own image");
        assert_eq!((images[0][0].id, images[0][0].generation), img_a);
        assert_eq!(images[1].len(), 1, "page 2 must report only its own image");
        assert_eq!((images[1][0].id, images[1][0].generation), img_b);
    }

    #[test]
    fn finds_image_drawn_inside_a_form_xobject() {
        let (bytes, img) = build_form_wrapped_image();
        let images = discover_images(&bytes, 1);
        assert_eq!(images[0].len(), 1);
        assert_eq!((images[0][0].id, images[0][0].generation), img);
    }

    #[test]
    fn jpeg_image_is_discoverable_and_extractable() {
        let jpeg = b"\xFF\xD8\xFF\xE0 dummy jpeg \xFF\xD9";
        let (bytes, (id, generation)) = build_pdf("DCTDecode", jpeg);

        let images = discover_images(&bytes, 1);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].len(), 1);
        assert_eq!((images[0][0].id, images[0][0].generation), (id, generation));
        assert!(images[0][0].extractable);

        // DCTDecode stream is already a JPEG: bytes round-trip verbatim.
        assert_eq!(
            extract_image(&bytes, id, generation, usize::MAX).unwrap(),
            jpeg
        );
    }

    /// One-page PDF with a `width`×`height` DeviceRGB image, FlateDecode-
    /// compressed by lopdf itself (so `decompressed_content` round-trips),
    /// drawn via `/Im0 Do`. A constant fill guarantees lopdf actually keeps the
    /// compressed form (and sets `/Filter /FlateDecode`).
    fn build_flate_rgb(width: i64, height: i64) -> (Vec<u8>, (u32, u16)) {
        let mut doc = Document::with_version("1.5");
        let raw = vec![128u8; (width * height * 3) as usize];
        let mut image = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => width,
                "Height" => height,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
            },
            raw,
        );
        image.compress().expect("flate-compress image stream");
        let image_id = doc.add_object(image);
        let resources =
            doc.add_object(dictionary! { "XObject" => dictionary! { "Im0" => image_id } });
        let content = doc.add_object(Stream::new(dictionary! {}, b"/Im0 Do".to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Resources" => resources,
            "Contents" => content,
        });
        let pages = doc.add_object(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page.into()],
            "Count" => 1,
        });
        if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(page) {
            dict.set("Parent", pages);
        }
        (finish(doc, pages), image_id)
    }

    #[test]
    fn flate_rgb_image_is_classified_extractable() {
        // Through a real reloaded PDF: a FlateDecode DeviceRGB 8-bit image is
        // recognized as extractable (so the renderer emits a handle, not a
        // marker). The decode→PNG round-trip is covered separately by
        // `raster_to_png_produces_a_decodable_png` and the real-PDF e2e test.
        let (bytes, _) = build_flate_rgb(8, 8);
        let images = discover_images(&bytes, 1);
        assert_eq!(images[0].len(), 1);
        assert!(images[0][0].extractable);
    }

    #[test]
    fn raster_to_png_produces_a_decodable_png() {
        use std::io::Read;

        // 2×1 RGB: two pixels.
        let samples = [10u8, 20, 30, 40, 50, 60];
        let png = raster_to_png(2, 1, PngColor::Rgb, &samples);

        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "valid PNG signature");
        assert_eq!(&png[16..24], &[0, 0, 0, 2, 0, 0, 0, 1], "IHDR 2×1");

        // IHDR is a fixed 13-byte chunk, so IDAT data starts at byte 41.
        let idat_len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        let idat = &png[41..41 + idat_len];
        let mut inflated = Vec::new();
        flate2::read::ZlibDecoder::new(idat)
            .read_to_end(&mut inflated)
            .unwrap();
        // One filter byte (0 = None) followed by the row's RGB bytes.
        assert_eq!(inflated, [0, 10, 20, 30, 40, 50, 60]);
    }

    #[test]
    fn png_predictor_is_undone_across_filter_types() {
        // 3 wide, 1 component (grayscale), 3 rows exercising Sub(1), Up(2),
        // Paeth(4). Each input row is `filter_byte` + 3 sample bytes.
        //   row0 Sub:   5,3,2  -> 5, 8, 10
        //   row1 Up:    1,1,1  + above -> 6, 9, 11
        //   row2 Paeth: 0,0,0  -> picks `above` -> 6, 9, 11
        let input = vec![1, 5, 3, 2, 2, 1, 1, 1, 4, 0, 0, 0];
        let out = png_unpredict(input, 3, 1);
        assert_eq!(out, [5, 8, 10, 6, 9, 11, 6, 9, 11]);
    }

    #[test]
    fn tiff_predictor_undoes_horizontal_differencing() {
        // 2 RGB pixels: first pixel absolute, second stored as a delta.
        let out = tiff_unpredict(vec![10, 20, 30, 1, 2, 3], 2, 3);
        assert_eq!(out, [10, 20, 30, 11, 22, 33]);
    }

    #[test]
    fn oversized_raster_reports_budget_instead_of_allocating() {
        let (bytes, (id, generation)) = build_flate_rgb(8, 8);
        // Raw size is 8*8*3 = 192 bytes; a smaller budget must refuse.
        assert!(matches!(
            extract_image(&bytes, id, generation, 100),
            Err(ImageExtractError::TooLarge(192))
        ));
    }

    #[test]
    fn unsupported_codec_image_is_marked_present_but_not_extractable() {
        let (bytes, (id, generation)) = build_pdf("CCITTFaxDecode", b"fax bytes");

        let images = discover_images(&bytes, 1);
        assert_eq!(images[0].len(), 1);
        assert!(!images[0][0].extractable);

        assert!(matches!(
            extract_image(&bytes, id, generation, usize::MAX),
            Err(ImageExtractError::UnsupportedEncoding(_))
        ));
    }

    #[test]
    fn missing_or_non_image_object_is_rejected() {
        let (bytes, _) = build_pdf("DCTDecode", b"x");
        assert!(matches!(
            extract_image(&bytes, 9999, 0, usize::MAX),
            Err(ImageExtractError::NotFound)
        ));
    }

    #[test]
    fn unreadable_pdf_yields_no_images() {
        assert!(discover_images(b"not a pdf", 1).iter().all(Vec::is_empty));
    }

    /// Two Form XObjects that draw each other (a reference cycle), each also
    /// drawing its own image. Exercises the form-dedup guard: the walk must
    /// visit each form once, terminate, and report both images exactly once.
    fn build_mutually_recursive_forms() -> (Vec<u8>, (u32, u16), (u32, u16)) {
        let mut doc = Document::with_version("1.5");
        let img_a = doc.add_object(Stream::new(image_dict("DCTDecode"), b"AAAA".to_vec()));
        let img_b = doc.add_object(Stream::new(image_dict("DCTDecode"), b"BBBB".to_vec()));

        // Content references the other form by name only, so it can be set at
        // creation; the name→id wiring goes into Resources, patched afterwards to
        // break the mutual-reference chicken-and-egg.
        let form_a = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![0i64.into(), 0i64.into(), 1i64.into(), 1i64.into()],
            },
            b"/ImA Do /FmB Do".to_vec(),
        ));
        let form_b = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![0i64.into(), 0i64.into(), 1i64.into(), 1i64.into()],
            },
            b"/ImB Do /FmA Do".to_vec(),
        ));
        if let Ok(Object::Stream(stream)) = doc.get_object_mut(form_a) {
            stream.dict.set(
                "Resources",
                dictionary! { "XObject" => dictionary! { "ImA" => img_a, "FmB" => form_b } },
            );
        }
        if let Ok(Object::Stream(stream)) = doc.get_object_mut(form_b) {
            stream.dict.set(
                "Resources",
                dictionary! { "XObject" => dictionary! { "ImB" => img_b, "FmA" => form_a } },
            );
        }

        let page_resources =
            doc.add_object(dictionary! { "XObject" => dictionary! { "Fm0" => form_a } });
        let content = doc.add_object(Stream::new(dictionary! {}, b"/Fm0 Do".to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Resources" => page_resources,
            "Contents" => content,
        });
        let pages = doc.add_object(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page.into()],
            "Count" => 1,
        });
        if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(page) {
            dict.set("Parent", pages);
        }
        (finish(doc, pages), img_a, img_b)
    }

    #[test]
    fn mutually_recursive_forms_terminate_and_report_each_image_once() {
        let (bytes, img_a, img_b) = build_mutually_recursive_forms();
        let images = discover_images(&bytes, 1);
        assert_eq!(images.len(), 1);

        let mut found: Vec<(u32, u16)> = images[0].iter().map(|i| (i.id, i.generation)).collect();
        found.sort();
        let mut expected = vec![img_a, img_b];
        expected.sort();
        assert_eq!(
            found, expected,
            "both images reported once, no duplication from the cycle"
        );
    }
}
