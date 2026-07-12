//! PDF hyperlink recovery: URI link annotations → Markdown links.
//!
//! The engine tags each text span with the link annotation whose rectangle
//! contains it (runs are segmented at rect boundaries), so a link's anchor
//! text is exactly the concatenation of its tagged spans. This module turns
//! that into Markdown: an anchored link wraps its anchor occurrence in the
//! page text as `[anchor](target)`; a link whose anchor cannot be recovered
//! or found is appended to the page as a bare `<target>` autolink, so the
//! agent never loses a destination — the acceptance rule is "no URL is
//! dropped", not "every anchor is beautiful".
//!
//! Only `http://`, `https://` and `mailto:` targets are emitted. A PDF can
//! carry `javascript:`, `file:` or custom-scheme URI actions; spoor's output
//! is consumed by agents that may follow links mechanically, so executable
//! and local-resource schemes are dropped rather than surfaced.

use crate::locate::Locator;
use crate::parse::pdf_engine::{EnginePage, EngineSpan};
use std::cmp::Ordering;

/// Longest anchor text (in chars) still treated as a real anchor. A rect
/// covering half the page (some generators emit sloppy annotation boxes)
/// would otherwise wrap a whole paragraph in one link.
const MAX_ANCHOR_CHARS: usize = 200;

/// One link ready for weaving: a safe target plus the anchor text recovered
/// from the spans inside its rectangle, when there was any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageLink {
    pub(crate) uri: String,
    pub(crate) anchor: Option<String>,
}

fn allowed_scheme(uri: &str) -> bool {
    let lower = uri.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
}

/// Assemble the page's links with their anchor text, dropping unsafe schemes.
pub(crate) fn page_links(page: &EnginePage) -> Vec<PageLink> {
    page.links
        .iter()
        .enumerate()
        .filter(|(_, link)| allowed_scheme(&link.uri))
        .map(|(index, link)| {
            let mut spans: Vec<&EngineSpan> = page
                .spans
                .iter()
                .filter(|span| span.link == Some(index))
                .collect();
            // Reading order within the anchor: top-to-bottom, then
            // left-to-right, so a two-line anchor joins in prose order.
            spans.sort_by(|a, b| {
                a.y.partial_cmp(&b.y)
                    .unwrap_or(Ordering::Equal)
                    .then(a.x0.partial_cmp(&b.x0).unwrap_or(Ordering::Equal))
            });
            let anchor = spans
                .iter()
                .map(|span| span.text.trim())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let anchor = (!anchor.is_empty() && anchor.chars().count() <= MAX_ANCHOR_CHARS)
                .then_some(anchor);
            PageLink {
                uri: link.uri.clone(),
                anchor,
            }
        })
        .collect()
}

/// Weave `links` into the page text. Anchored links wrap the first free
/// (whitespace-insensitive) occurrence of their anchor; everything else is
/// appended as an autolink so the target survives.
pub(crate) fn apply_links(text: &str, links: &[PageLink]) -> String {
    if links.is_empty() {
        return text.to_string();
    }

    let mut placed: Vec<(usize, usize, &str)> = Vec::new();
    let mut leftover: Vec<&str> = Vec::new();
    {
        let locator = Locator::new(text);
        for link in links {
            let hit = link.anchor.as_deref().and_then(|anchor| {
                locator
                    .all_occurrences(anchor)
                    .into_iter()
                    .find(|(start, end)| {
                        placed.iter().all(|(used_start, used_end, _)| {
                            *end <= *used_start || *start >= *used_end
                        })
                    })
            });
            match hit {
                Some((start, end)) => placed.push((start, end, &link.uri)),
                None => leftover.push(&link.uri),
            }
        }
    }
    placed.sort_by_key(|(start, _, _)| *start);

    let mut out = String::with_capacity(text.len() + 32 * links.len());
    let mut cursor = 0;
    for (start, end, uri) in &placed {
        out.push_str(&text[cursor..*start]);
        out.push('[');
        out.push_str(&escape_anchor(&text[*start..*end]));
        out.push_str("](");
        out.push_str(&escape_uri(uri));
        out.push(')');
        cursor = *end;
    }
    out.push_str(&text[cursor..]);

    for uri in leftover {
        if !out.trim().is_empty() {
            out.push_str("\n\n");
        }
        out.push('<');
        out.push_str(&escape_uri(uri));
        out.push('>');
    }
    out
}

/// Escape the characters that would terminate or nest Markdown link text.
fn escape_anchor(anchor: &str) -> String {
    let mut out = String::with_capacity(anchor.len());
    for ch in anchor.chars() {
        if ch == '[' || ch == ']' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Percent-encode the characters that would break out of a Markdown link
/// destination or a GFM table cell.
fn escape_uri(uri: &str) -> String {
    let mut out = String::with_capacity(uri.len());
    for ch in uri.chars() {
        match ch {
            ' ' => out.push_str("%20"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '|' => out.push_str("%7C"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{PageLink, apply_links, page_links};
    use crate::parse::pdf_engine::{EngineLink, EnginePage, EngineSpan};

    fn span(text: &str, x0: f64, y: f64, link: Option<usize>) -> EngineSpan {
        EngineSpan {
            text: text.to_string(),
            x0,
            x1: x0 + 10.0 * text.len() as f64,
            y,
            font_size: 10.0,
            link,
        }
    }

    fn page_with(links: Vec<&str>, spans: Vec<EngineSpan>) -> EnginePage {
        EnginePage {
            width: 600.0,
            height: 800.0,
            spans,
            vector: Default::default(),
            links: links
                .into_iter()
                .map(|uri| EngineLink {
                    uri: uri.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn anchor_is_assembled_from_tagged_spans_in_reading_order() {
        let page = page_with(
            vec!["https://example.com/guide"],
            vec![
                span("See the", 50.0, 100.0, None),
                span("full", 120.0, 100.0, Some(0)),
                // Second anchor line sits below the first.
                span("guide", 50.0, 120.0, Some(0)),
                span("for details.", 110.0, 120.0, None),
            ],
        );

        let links = page_links(&page);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].anchor.as_deref(), Some("full guide"));
    }

    #[test]
    fn unsafe_schemes_are_dropped_entirely() {
        let page = page_with(
            vec![
                "javascript:alert(1)",
                "file:///etc/passwd",
                "HTTPS://ok.example",
            ],
            vec![span("click", 50.0, 100.0, Some(0))],
        );

        let links = page_links(&page);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].uri, "HTTPS://ok.example");
    }

    #[test]
    fn oversized_anchor_degrades_to_bare_target() {
        let long = "word ".repeat(60);
        let page = page_with(
            vec!["https://example.com"],
            vec![span(&long, 50.0, 100.0, Some(0))],
        );

        let links = page_links(&page);
        assert_eq!(links[0].anchor, None);
    }

    #[test]
    fn anchored_link_wraps_in_place_and_leftover_appends() {
        let text = "See the full guide\nfor details.";
        let woven = apply_links(
            text,
            &[
                PageLink {
                    uri: "https://example.com/guide".into(),
                    anchor: Some("full guide".into()),
                },
                PageLink {
                    uri: "https://example.com/api".into(),
                    anchor: None,
                },
            ],
        );

        assert_eq!(
            woven,
            "See the [full guide](https://example.com/guide)\nfor details.\n\n<https://example.com/api>"
        );
    }

    #[test]
    fn anchor_crossing_a_line_break_is_still_wrapped() {
        // The anchor text was assembled from two lines ("full guide"), while
        // the page text breaks between them; the whitespace-insensitive find
        // must still locate and wrap it.
        let text = "See the full\nguide for details.";
        let woven = apply_links(
            text,
            &[PageLink {
                uri: "https://example.com/guide".into(),
                anchor: Some("full guide".into()),
            }],
        );

        assert_eq!(
            woven,
            "See the [full\nguide](https://example.com/guide) for details."
        );
    }

    #[test]
    fn repeated_anchor_claims_distinct_occurrences() {
        let text = "docs here and docs there";
        let woven = apply_links(
            text,
            &[
                PageLink {
                    uri: "https://a.example".into(),
                    anchor: Some("docs".into()),
                },
                PageLink {
                    uri: "https://b.example".into(),
                    anchor: Some("docs".into()),
                },
            ],
        );

        assert_eq!(
            woven,
            "[docs](https://a.example) here and [docs](https://b.example) there"
        );
    }

    #[test]
    fn markdown_hostile_characters_are_escaped() {
        let text = "spec [draft] here";
        let woven = apply_links(
            text,
            &[PageLink {
                uri: "https://example.com/a(b)|c d".into(),
                anchor: Some("spec [draft]".into()),
            }],
        );

        assert_eq!(
            woven,
            "[spec \\[draft\\]](https://example.com/a%28b%29%7Cc%20d) here"
        );
    }

    #[test]
    fn missing_anchor_occurrence_falls_back_to_autolink() {
        let text = "completely different words";
        let woven = apply_links(
            text,
            &[PageLink {
                uri: "https://example.com".into(),
                anchor: Some("not present".into()),
            }],
        );

        assert_eq!(woven, "completely different words\n\n<https://example.com>");
    }

    #[test]
    fn empty_page_text_lists_targets_without_leading_gap() {
        let woven = apply_links(
            "",
            &[PageLink {
                uri: "https://example.com".into(),
                anchor: None,
            }],
        );

        assert_eq!(woven, "<https://example.com>");
    }
}
