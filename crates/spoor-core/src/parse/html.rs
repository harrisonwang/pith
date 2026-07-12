use crate::output::MarkdownBuilder;
use crate::output::decode_text;
use crate::source::Source;
use anyhow::Result;
use scraper::node::Node;
use scraper::{ElementRef, Html, Selector};

pub fn extract(source: &Source<'_>, max_parse_bytes: usize) -> Result<String> {
    let html = decode_text(source.bytes());
    let doc = Html::parse_document(&html);

    let article_sel = Selector::parse("article").unwrap();
    let main_sel = Selector::parse("main").unwrap();
    let body_sel = Selector::parse("body").unwrap();

    let root = doc
        .select(&article_sel)
        .next()
        .or_else(|| doc.select(&main_sel).next())
        .or_else(|| doc.select(&body_sel).next());

    // Resolve relative links/images against the document URL when we have one;
    // for local files, stdin, or raw byte calls there is no base, so links stay
    // verbatim. See `absolutize`.
    let base = source.url_base();

    let mut md = MarkdownBuilder::with_max_bytes(max_parse_bytes);
    if let Some(root) = root {
        render_children(root, &mut md, base);
    }
    md.build()
}

fn render_children(element: ElementRef<'_>, md: &mut MarkdownBuilder, base: Option<&str>) {
    for child in element.children() {
        let Some(child_el) = ElementRef::wrap(child) else {
            continue;
        };
        render_block(child_el, md, base);
    }
}

fn render_block(element: ElementRef<'_>, md: &mut MarkdownBuilder, base: Option<&str>) {
    let name = element.value().name();
    if should_skip_block(name) {
        return;
    }

    match name {
        "h1" => md.heading(1, &inline_text(element, base)),
        "h2" => md.heading(2, &inline_text(element, base)),
        "h3" => md.heading(3, &inline_text(element, base)),
        "h4" => md.heading(4, &inline_text(element, base)),
        "h5" => md.heading(5, &inline_text(element, base)),
        "h6" => md.heading(6, &inline_text(element, base)),
        "p" => md.paragraph(&inline_text(element, base)),
        "ul" => render_list(element, md, false, base),
        "ol" => render_list(element, md, true, base),
        "table" => render_table(element, md, base),
        "blockquote" => render_blockquote(element, md, base),
        "pre" => render_preformatted(element, md),
        "article" | "main" | "section" | "div" | "body" => render_children(element, md, base),
        _ => {
            if has_block_children(element) {
                render_children(element, md, base);
            } else {
                md.paragraph(&inline_text(element, base));
            }
        }
    }
}

fn should_skip_block(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "noscript" | "template" | "header" | "nav" | "footer" | "aside"
    )
}

fn has_block_children(element: ElementRef<'_>) -> bool {
    element.children().any(|child| {
        ElementRef::wrap(child)
            .map(|el| {
                matches!(
                    el.value().name(),
                    "h1" | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "p"
                        | "ul"
                        | "ol"
                        | "table"
                        | "blockquote"
                        | "pre"
                        | "article"
                        | "main"
                        | "section"
                        | "div"
                )
            })
            .unwrap_or(false)
    })
}

fn render_blockquote(element: ElementRef<'_>, md: &mut MarkdownBuilder, base: Option<&str>) {
    let text = inline_text(element, base);
    if text.is_empty() {
        return;
    }
    md.blank_line();
    for line in text.lines() {
        md.raw(&format!("> {}\n", line.trim()));
    }
}

fn render_preformatted(element: ElementRef<'_>, md: &mut MarkdownBuilder) {
    let text = element.text().collect::<String>();
    let text = text.trim_matches('\n');
    if text.trim().is_empty() {
        return;
    }
    md.blank_line();
    md.raw("```\n");
    md.raw(text);
    if !text.ends_with('\n') {
        md.raw("\n");
    }
    md.raw("```\n");
}

fn render_list(
    element: ElementRef<'_>,
    md: &mut MarkdownBuilder,
    ordered: bool,
    base: Option<&str>,
) {
    let mut index = 1usize;
    for child in element.children() {
        let Some(li) = ElementRef::wrap(child) else {
            continue;
        };
        if li.value().name() != "li" {
            continue;
        }

        let text = inline_text(li, base);
        if text.is_empty() {
            continue;
        }
        md.blank_line();
        if ordered {
            md.raw(&format!("{index}. {text}\n"));
            index += 1;
        } else {
            md.raw(&format!("- {text}\n"));
        }
    }
}

fn render_table(element: ElementRef<'_>, md: &mut MarkdownBuilder, base: Option<&str>) {
    let row_sel = Selector::parse("tr").unwrap();
    let cell_sel = Selector::parse("th, td").unwrap();
    let rows: Vec<Vec<String>> = element
        .select(&row_sel)
        .map(|row| {
            row.select(&cell_sel)
                .map(|cell| inline_text(cell, base))
                .collect::<Vec<_>>()
        })
        .filter(|row| !row.is_empty())
        .collect();

    md.table(&rows);
}

fn inline_text(element: ElementRef<'_>, base: Option<&str>) -> String {
    normalize_inline(&inline_children(element, base))
}

fn inline_children(element: ElementRef<'_>, base: Option<&str>) -> String {
    let mut out = String::new();
    for child in element.children() {
        match child.value() {
            Node::Text(text) => out.push_str(text),
            Node::Element(el) => {
                let Some(child_el) = ElementRef::wrap(child) else {
                    continue;
                };
                match el.name() {
                    "script" | "style" | "noscript" | "template" => {}
                    "a" => {
                        let text = inline_text(child_el, base);
                        match el.attr("href") {
                            Some(href) => {
                                let href = absolutize(base, href);
                                if text.is_empty() {
                                    out.push_str(&href);
                                } else {
                                    out.push_str(&format!("[{text}]({href})"));
                                }
                            }
                            None => out.push_str(&text),
                        }
                    }
                    "strong" | "b" => {
                        let text = inline_text(child_el, base);
                        if !text.is_empty() {
                            out.push_str(&format!("**{text}**"));
                        }
                    }
                    "em" | "i" => {
                        let text = inline_text(child_el, base);
                        if !text.is_empty() {
                            out.push_str(&format!("*{text}*"));
                        }
                    }
                    "code" => {
                        let text = inline_text(child_el, base);
                        if !text.is_empty() {
                            out.push_str(&format!("`{text}`"));
                        }
                    }
                    "img" => render_image(el, base, &mut out),
                    "br" => out.push('\n'),
                    "ul" | "ol" | "table" => {}
                    _ => out.push_str(&inline_children(child_el, base)),
                }
            }
            _ => {}
        }
    }
    out
}

/// Render `<img>` as a standard Markdown image so an agent can hand the URL to
/// a VLM directly. The src is absolutized against the document URL when there
/// is one. `data:` URIs carry no fetchable handle and inlining their base64
/// payload only burns context, so they fall back to an alt-only placeholder —
/// the same signal used when an `<img>` has no usable src at all.
fn render_image(el: &scraper::node::Element, base: Option<&str>, out: &mut String) {
    let alt = el.attr("alt").map(str::trim).unwrap_or("");
    match el.attr("src").map(str::trim).filter(|src| !src.is_empty()) {
        Some(src) if !src.starts_with("data:") => {
            out.push_str(&format!("![{alt}]({})", absolutize(base, src)));
        }
        _ if !alt.is_empty() => out.push_str(&format!("[图片：{alt}]")),
        _ => {}
    }
}

fn normalize_inline(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Best-effort resolution of an HTML href/src against the document's source
/// URL. Pure string work — no network and no `url` crate — so it behaves
/// identically across the CLI, native library, and WASM. Anything already
/// absolute (a scheme like `http:`/`https:`/`data:`/`mailto:`, a protocol-
/// relative `//host`, or an in-page `#fragment`) is left untouched, and when
/// there is no usable http(s) base the link is returned verbatim.
fn absolutize(base: Option<&str>, link: &str) -> String {
    let link = link.trim();
    if link.is_empty() || link.starts_with('#') || link.starts_with("//") || has_scheme(link) {
        return link.to_string();
    }

    let Some((scheme, authority, base_path)) = base.and_then(split_http_url) else {
        return link.to_string();
    };

    if let Some(rest) = link.strip_prefix('/') {
        // Absolute path: keep authority, replace path.
        return format!("{scheme}://{authority}/{rest}");
    }
    if link.starts_with('?') {
        // Query-only ref attaches to the base path.
        return format!("{scheme}://{authority}{base_path}{link}");
    }

    // Relative path: resolve against the base directory.
    let base_dir = base_path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let merged = format!("{base_dir}/{link}");
    let normalized = remove_dot_segments(&merged);
    format!("{scheme}://{authority}{normalized}")
}

/// Whether `link` begins with a URI scheme (`scheme:`), e.g. `https:`,
/// `data:`, `mailto:`. A leading `/` (path) or digit is not a scheme.
fn has_scheme(link: &str) -> bool {
    let mut chars = link.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    for c in chars {
        if c == ':' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') {
            return false;
        }
    }
    false
}

/// Split an absolute http(s) URL into `(scheme, authority, path)`, dropping any
/// query and fragment. Returns `None` for non-http(s) or authority-less URLs.
fn split_http_url(url: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = if let Some(rest) = url.strip_prefix("https://") {
        ("https", rest)
    } else {
        ("http", url.strip_prefix("http://")?)
    };

    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    if authority.is_empty() {
        return None;
    }

    let after_auth = &rest[auth_end..];
    let path = if after_auth.starts_with('/') {
        let end = after_auth.find(['?', '#']).unwrap_or(after_auth.len());
        &after_auth[..end]
    } else {
        ""
    };
    Some((scheme, authority, path))
}

/// RFC 3986 dot-segment removal over an absolute path (`.` dropped, `..` pops
/// the previous segment). Preserves a trailing slash.
fn remove_dot_segments(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            seg => out.push(seg),
        }
    }
    let mut result = String::from("/");
    result.push_str(&out.join("/"));
    let trailing = path.ends_with('/') || path.ends_with("/.") || path.ends_with("/..");
    if trailing && !result.ends_with('/') {
        result.push('/');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::absolutize;

    const BASE: &str = "https://example.com/blog/post.html";

    #[test]
    fn leaves_absolute_and_special_links_untouched() {
        for link in [
            "https://other.com/x",
            "http://other.com/x",
            "//cdn.example.com/x.png",
            "data:image/png;base64,AAAA",
            "mailto:hi@example.com",
            "tel:+123",
            "#section",
            "",
        ] {
            assert_eq!(absolutize(Some(BASE), link), link, "link: {link}");
        }
    }

    #[test]
    fn resolves_relative_links_against_url_base() {
        assert_eq!(
            absolutize(Some(BASE), "chart.png"),
            "https://example.com/blog/chart.png"
        );
        assert_eq!(
            absolutize(Some(BASE), "./chart.png"),
            "https://example.com/blog/chart.png"
        );
        assert_eq!(
            absolutize(Some(BASE), "../img/a.png"),
            "https://example.com/img/a.png"
        );
        assert_eq!(
            absolutize(Some(BASE), "/about"),
            "https://example.com/about"
        );
        assert_eq!(
            absolutize(Some(BASE), "?page=2"),
            "https://example.com/blog/post.html?page=2"
        );
    }

    #[test]
    fn directory_base_keeps_relative_inside_directory() {
        assert_eq!(
            absolutize(Some("https://example.com/blog/"), "chart.png"),
            "https://example.com/blog/chart.png"
        );
        assert_eq!(
            absolutize(Some("https://example.com"), "chart.png"),
            "https://example.com/chart.png"
        );
    }

    #[test]
    fn without_a_url_base_links_are_left_verbatim() {
        // A library/WASM caller that did not pass the source URL has no base;
        // degrade safely rather than fabricate one.
        assert_eq!(absolutize(None, "chart.png"), "chart.png");
        assert_eq!(absolutize(None, "/about"), "/about");
    }
}
