//! Deterministic quote grounding inside spoor's own Markdown output.
//!
//! An LLM answering from a parsed document cites a quote as its evidence;
//! [`locate_quote`] verifies that citation by finding the quote in the
//! Markdown spoor actually produced. Four tiers, strictest first, all
//! deterministic:
//!
//! 1. **Exact** substring.
//! 2. **Whitespace-insensitive**: spacing inside a model-written quote (around
//!    numbers and CJK punctuation) rarely matches the source byte-for-byte.
//! 3. **Table anchor**: a model quoting table data usually reassembles
//!    "column header + row label + value" into one string that never appears
//!    contiguously in a Markdown table. Anchor on the quote's most
//!    identifiable number, verify the hit line with the quote's label words,
//!    and return the whole table row as evidence.
//! 4. **Numeric equivalence**: the same value written under a different CJK
//!    magnitude unit (7771亿 vs 777102百万). Tried only when the quote carries
//!    an explicit unit, with 0.2% tolerance for rounding, and accepted only
//!    with a label-word hit or a document-unique value.
//!
//! A quote that fails all four tiers is absent from the source; the caller
//! should treat the claim it backs as unverifiable instead of trusting the
//! model's self-citation. `None` is that signal, not an error.

use crate::result::{ProvenanceSpan, SourceAnchor, TextRange};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Characters of collapsed context returned on each side of the hit.
const CONTEXT_CHARS: usize = 30;
/// Relative tolerance for tier 4: absorbs rounding (1335亿 vs 1334.54亿)
/// without matching a different magnitude.
const NUMERIC_TOLERANCE: f64 = 0.002;

/// The strictest tier that matched. Callers wanting verbatim-only evidence can
/// filter on this instead of asking for a different search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocateMethod {
    Exact,
    WhitespaceInsensitive,
    TableAnchor,
    NumericEquivalence,
}

impl LocateMethod {
    pub const ALL: [LocateMethod; 4] = [
        LocateMethod::Exact,
        LocateMethod::WhitespaceInsensitive,
        LocateMethod::TableAnchor,
        LocateMethod::NumericEquivalence,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::WhitespaceInsensitive => "whitespace_insensitive",
            Self::TableAnchor => "table_anchor",
            Self::NumericEquivalence => "numeric_equivalence",
        }
    }
}

impl fmt::Display for LocateMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A grounded quote: where it sits in the Markdown and what surrounds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocatedQuote {
    /// Byte range of the hit in the Markdown (same contract as provenance
    /// ranges); bindings convert to their language's native string indices.
    pub span: TextRange,
    /// Up to [`CONTEXT_CHARS`] chars of whitespace-collapsed context before the hit.
    pub before: String,
    /// The matched text, whitespace-collapsed.
    pub hit: String,
    /// Up to [`CONTEXT_CHARS`] chars of whitespace-collapsed context after the hit.
    pub after: String,
    /// 1-based source page, read from spoor's own `## Page N` markers when the
    /// Markdown carries them (PDF output); `None` otherwise.
    pub page: Option<usize>,
    pub method: LocateMethod,
    /// The source anchor whose provenance span overlaps the hit the most,
    /// when the caller supplied the document's provenance spans (see
    /// [`Locator::locate_grounded`]); block-level spans put an approximate
    /// page box here, turning a verified quote into a highlight target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<SourceAnchor>,
}

/// Locate `quote` inside `markdown`. Convenience wrapper over [`Locator`] for
/// one-off calls; build a [`Locator`] to ground many quotes against one
/// document without re-indexing it.
pub fn locate_quote(markdown: &str, quote: &str) -> Option<LocatedQuote> {
    Locator::new(markdown).locate(quote)
}

/// Like [`locate_quote`], but also resolves the hit against the document's
/// provenance spans (as returned by `parse` with `ProvenanceLevel::Page` or
/// `Block`), filling [`LocatedQuote::anchor`] with the best-overlapping
/// span's source. With block-level provenance this grounds a verified quote
/// to an approximate box on its source page.
pub fn locate_quote_grounded(
    markdown: &str,
    quote: &str,
    spans: &[ProvenanceSpan],
) -> Option<LocatedQuote> {
    Locator::new(markdown).locate_grounded(quote, spans)
}

/// Reusable matcher over one Markdown string. Construction builds the
/// whitespace-stripped index once; each [`Locator::locate`] call is then
/// independent and read-only.
pub struct Locator<'a> {
    md: &'a str,
    /// `md` with all whitespace removed.
    stripped: String,
    /// For every byte of `stripped`, the byte offset in `md` where the owning
    /// char starts.
    map: Vec<usize>,
}

impl<'a> Locator<'a> {
    pub fn new(markdown: &'a str) -> Self {
        let mut stripped = String::with_capacity(markdown.len());
        let mut map = Vec::with_capacity(markdown.len());
        for (offset, ch) in markdown.char_indices() {
            if ch.is_whitespace() {
                continue;
            }
            stripped.push(ch);
            for _ in 0..ch.len_utf8() {
                map.push(offset);
            }
        }
        Self {
            md: markdown,
            stripped,
            map,
        }
    }

    /// Locate one quote. Returns `None` when no tier matches — the quote is
    /// not in the document and the claim it backs is unverifiable.
    pub fn locate(&self, quote: &str) -> Option<LocatedQuote> {
        let quote = quote.trim();
        if quote.is_empty() {
            return None;
        }
        let (start, end, method) = self
            .find_span(quote)
            .or_else(|| {
                self.anchored_span(quote)
                    .map(|(s, e)| (s, e, LocateMethod::TableAnchor))
            })
            .or_else(|| {
                self.numeric_span(quote)
                    .map(|(s, e)| (s, e, LocateMethod::NumericEquivalence))
            })?;
        Some(LocatedQuote {
            span: TextRange { start, end },
            before: context_before(self.md, start),
            hit: collapse_whitespace(&self.md[start..end]),
            after: context_after(self.md, end),
            page: page_of(self.md, start),
            method,
            anchor: None,
        })
    }

    /// [`Locator::locate`], then resolve the hit against `spans` — the
    /// provenance spans of the same Markdown — attaching the source anchor of
    /// the span the hit overlaps the most. A hit crossing a line boundary
    /// overlaps several block-level spans; the dominant one wins.
    pub fn locate_grounded(&self, quote: &str, spans: &[ProvenanceSpan]) -> Option<LocatedQuote> {
        let mut found = self.locate(quote)?;
        found.anchor = spans
            .iter()
            .filter_map(|span| {
                let overlap = span
                    .output
                    .end
                    .min(found.span.end)
                    .saturating_sub(span.output.start.max(found.span.start));
                (overlap > 0).then_some((overlap, span))
            })
            .max_by_key(|(overlap, _)| *overlap)
            .map(|(_, span)| span.source.clone());
        Some(found)
    }

    /// Tiers 1–2: exact substring, then whitespace-insensitive.
    fn find_span(&self, quote: &str) -> Option<(usize, usize, LocateMethod)> {
        if let Some(start) = self.md.find(quote) {
            return Some((start, start + quote.len(), LocateMethod::Exact));
        }
        let needle: String = quote.chars().filter(|c| !c.is_whitespace()).collect();
        if needle.is_empty() {
            return None;
        }
        let at = self.stripped.find(&needle)?;
        let (start, end) = self.map_back(at, &needle);
        Some((start, end, LocateMethod::WhitespaceInsensitive))
    }

    /// All whitespace-insensitive occurrences of `needle`, as `md` byte ranges.
    /// Also serves PDF link-anchor weaving, which must find every candidate
    /// position for an anchor rather than only the first.
    pub(crate) fn all_occurrences(&self, needle: &str) -> Vec<(usize, usize)> {
        let needle: String = needle.chars().filter(|c| !c.is_whitespace()).collect();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(found) = self.stripped[from..].find(&needle) {
            let at = from + found;
            out.push(self.map_back(at, &needle));
            let first_len = self.stripped[at..].chars().next().map_or(1, char::len_utf8);
            from = at + first_len;
        }
        out
    }

    /// Convert a match at `stripped` byte offset `at` back to `md` byte range.
    fn map_back(&self, at: usize, needle: &str) -> (usize, usize) {
        let start = self.map[at];
        let last = needle.chars().next_back().expect("needle is non-empty");
        let end = self.map[at + needle.len() - last.len_utf8()] + last.len_utf8();
        (start, end)
    }

    /// Tier 3: anchor on the quote's most identifiable number, verify the hit
    /// line with the quote's label words, return the whole table row.
    fn anchored_span(&self, quote: &str) -> Option<(usize, usize)> {
        let numbers: Vec<&str> = number_tokens(quote)
            .into_iter()
            .map(|(start, len)| &quote[start..start + len])
            .collect();
        if numbers.is_empty() {
            return None;
        }
        // Prefer separator-bearing numbers (thousands comma, decimal, percent):
        // financial values usually carry one, bare years (2024) do not, so this
        // keeps a year from becoming the anchor. Then take the longest.
        let with_separator: Vec<&str> = numbers
            .iter()
            .copied()
            .filter(|n| has_separator(n))
            .collect();
        let pool = if with_separator.is_empty() {
            &numbers
        } else {
            &with_separator
        };
        let anchor = pool
            .iter()
            .copied()
            .fold(None::<&str>, |best, n| match best {
                Some(b) if n.len() <= b.len() => best,
                _ => Some(n),
            })?;
        let anchor_has_separator = has_separator(anchor);
        if anchor.len() < 3 && !anchor_has_separator {
            // Too short (single/double digits) to identify anything.
            return None;
        }

        let occurrences = self.all_occurrences(anchor);
        if occurrences.is_empty() {
            return None;
        }

        // Score each occurrence's line by how many of the quote's label words
        // (row/column names) it contains, to survive number collisions.
        let labels = label_tokens(quote);
        let mut best: Option<(usize, (usize, usize))> = None;
        for &(start, end) in &occurrences {
            let (line_start, line_end) = line_bounds(self.md, start);
            let line = &self.md[line_start..line_end];
            let score = labels.iter().filter(|w| line.contains(*w)).count();
            if best.is_none_or(|(best_score, _)| score > best_score) {
                best = Some((score, (start, end)));
            }
        }
        let (best_score, (start, end)) = best?;

        if !labels.is_empty() {
            // With label words present, at least one must corroborate the line:
            // a fabricated label riding on a coincidental number is rejected.
            if best_score < 1 {
                return None;
            }
        } else if !(occurrences.len() == 1 && (anchor.len() >= 4 || anchor_has_separator)) {
            // Without labels, only a document-unique, identifiable anchor counts.
            return None;
        }

        // A hit inside a Markdown table row returns the whole row: row label
        // and sibling cells are the evidence a reader needs.
        let (line_start, line_end) = line_bounds(self.md, start);
        let line = &self.md[line_start..line_end];
        if line.trim_start().starts_with('|') {
            return Some(trimmed_line_span(line, line_start));
        }
        Some((start, end))
    }

    /// Tier 4: numeric equivalence across CJK magnitude units. Enabled only
    /// when the quote's number carries an explicit unit — that is the sign the
    /// model converted units instead of copying the source verbatim.
    fn numeric_span(&self, quote: &str) -> Option<(usize, usize)> {
        let mut target = None;
        for token in number_unit_tokens(quote) {
            let Some(unit) = token.unit else { continue };
            let number = &quote[token.start..token.start + token.len];
            if significant_digits(number) < 3 {
                continue;
            }
            if let Some(value) = numeric_value(number, Some(unit)) {
                if value >= 1000.0 {
                    target = Some(value);
                    break;
                }
            }
        }
        let target = target?;

        let labels = label_tokens(quote);
        struct Candidate {
            start: usize,
            number_len: usize,
            line_start: usize,
            line_end: usize,
            score: usize,
        }
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut line_start = 0;
        for line in self.md.split('\n') {
            let line_end = line_start + line.len();
            // A unit in the row/column header ("（百万元）") applies to the
            // line's bare numbers; an explicit per-number unit wins.
            let line_unit = header_unit(line);
            for token in number_unit_tokens(line) {
                let number = &line[token.start..token.start + token.len];
                if significant_digits(number) < 3 {
                    continue;
                }
                let Some(value) = numeric_value(number, token.unit.or(line_unit)) else {
                    continue;
                };
                if value == 0.0 {
                    continue;
                }
                if ((value - target).abs() / target) <= NUMERIC_TOLERANCE {
                    let score = labels.iter().filter(|w| line.contains(*w)).count();
                    candidates.push(Candidate {
                        start: line_start + token.start,
                        number_len: token.len,
                        line_start,
                        line_end,
                        score,
                    });
                }
            }
            line_start = line_end + 1;
        }
        if candidates.is_empty() {
            return None;
        }

        let best = candidates
            .iter()
            .fold(None::<&Candidate>, |best, c| match best {
                Some(b) if c.score <= b.score => best,
                _ => Some(c),
            })?;
        // A label hit guards against value collisions; a document-unique value
        // is strong enough on its own (rescues synonym row labels).
        let accept = (!labels.is_empty() && best.score >= 1) || candidates.len() == 1;
        if !accept {
            return None;
        }

        let line = &self.md[best.line_start..best.line_end];
        if line.trim_start().starts_with('|') {
            return Some(trimmed_line_span(line, best.line_start));
        }
        Some((best.start, best.start + best.number_len))
    }
}

/// Last `## Page N` marker at or before `pos`, i.e. the page the offset is on.
fn page_of(md: &str, pos: usize) -> Option<usize> {
    let bytes = md.as_bytes();
    let mut page = None;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'#' && bytes[i + 1] == b'#' {
            if let Some((number, consumed)) = parse_page_marker(&md[i + 2..]) {
                if i <= pos {
                    page = Some(number);
                } else {
                    break;
                }
                i += 2 + consumed;
                continue;
            }
        }
        i += 1;
    }
    page
}

/// Parse `\s*Page\s+(\d+)` after a `##`; returns (page number, bytes consumed).
fn parse_page_marker(s: &str) -> Option<(usize, usize)> {
    let leading = s.len() - s.trim_start().len();
    let rest = s[leading..].strip_prefix("Page")?;
    let gap = rest.len() - rest.trim_start().len();
    if gap == 0 {
        return None;
    }
    let digits = rest[gap..].bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let number = rest[gap..gap + digits].parse().ok()?;
    Some((number, leading + "Page".len() + gap + digits))
}

fn line_bounds(md: &str, pos: usize) -> (usize, usize) {
    let start = md[..pos].rfind('\n').map_or(0, |i| i + 1);
    let end = md[pos..].find('\n').map_or(md.len(), |i| pos + i);
    (start, end)
}

/// Byte span of `line` (at `line_start` in the document) without the
/// surrounding whitespace.
fn trimmed_line_span(line: &str, line_start: usize) -> (usize, usize) {
    let start = line_start + (line.len() - line.trim_start().len());
    let end = line_start + line.trim_end().len();
    (start, end)
}

fn has_separator(number: &str) -> bool {
    number.contains([',', '.', '%'])
}

/// Number tokens `\d[\d,]*(\.\d+)?%?` as (byte start, byte len).
fn number_tokens(s: &str) -> Vec<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b',') {
            i += 1;
        }
        if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
            i += 2;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
        if i < bytes.len() && bytes[i] == b'%' {
            i += 1;
        }
        out.push((start, i - start));
    }
    out
}

struct NumberUnitToken {
    /// Byte start of the number (unit excluded).
    start: usize,
    /// Byte length of the number (unit excluded).
    len: usize,
    unit: Option<&'static str>,
}

const MAGNITUDE_UNITS: [(&str, f64); 4] = [("百万", 1e6), ("亿", 1e8), ("万", 1e4), ("千", 1e3)];

fn unit_multiplier(unit: &str) -> f64 {
    MAGNITUDE_UNITS
        .iter()
        .find(|(name, _)| *name == unit)
        .map_or(1.0, |(_, mult)| *mult)
}

/// Match a magnitude unit at the head of `s`, longest first (百万 before 万).
fn leading_unit(s: &str) -> Option<&'static str> {
    MAGNITUDE_UNITS
        .iter()
        .map(|(name, _)| *name)
        .find(|name| s.starts_with(name))
}

/// Number tokens `\d[\d,]*(\.\d+)?` each with an optional trailing CJK
/// magnitude unit separated by optional whitespace.
fn number_unit_tokens(s: &str) -> Vec<NumberUnitToken> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b',') {
            i += 1;
        }
        if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
            i += 2;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
        let len = i - start;
        let after = &s[i..];
        let gap = after.len() - after.trim_start().len();
        let unit = leading_unit(&after[gap..]);
        out.push(NumberUnitToken { start, len, unit });
    }
    out
}

/// First `（百万元）`-style unit hint in a row/column header.
fn header_unit(line: &str) -> Option<&'static str> {
    for (i, ch) in line.char_indices() {
        if ch != '（' && ch != '(' {
            continue;
        }
        let after = &line[i + ch.len_utf8()..];
        let rest = after.trim_start();
        let Some(unit) = leading_unit(rest) else {
            continue;
        };
        if rest[unit.len()..].trim_start().starts_with('元') {
            return Some(unit);
        }
    }
    None
}

/// Digits carried by a number string, separators and leading zeros dropped.
fn significant_digits(number: &str) -> usize {
    number
        .chars()
        .filter(|c| *c != '.' && *c != ',')
        .collect::<String>()
        .trim_start_matches('0')
        .len()
}

fn numeric_value(number: &str, unit: Option<&str>) -> Option<f64> {
    let value: f64 = number.replace(',', "").parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some(value * unit.map_or(1.0, unit_multiplier))
}

/// Label words used to corroborate a numeric hit: CJK runs or ASCII words,
/// both at least two chars.
fn label_tokens(s: &str) -> Vec<&str> {
    fn is_cjk(c: char) -> bool {
        ('\u{4e00}'..='\u{9fff}').contains(&c)
    }
    let mut out = Vec::new();
    let mut run_start: Option<(usize, bool)> = None;
    for (i, ch) in s.char_indices() {
        let kind = if is_cjk(ch) {
            Some(true)
        } else if ch.is_ascii_alphabetic() {
            Some(false)
        } else {
            None
        };
        match (run_start, kind) {
            (None, Some(cjk)) => run_start = Some((i, cjk)),
            (Some((start, cjk)), current) if current != Some(cjk) => {
                push_label(&mut out, &s[start..i]);
                run_start = current.map(|c| (i, c));
            }
            _ => {}
        }
    }
    if let Some((start, _)) = run_start {
        push_label(&mut out, &s[start..]);
    }
    out
}

fn push_label<'a>(out: &mut Vec<&'a str>, run: &'a str) {
    if run.chars().count() >= 2 {
        out.push(run);
    }
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn context_before(md: &str, start: usize) -> String {
    let s = &md[..start];
    let cut = s
        .char_indices()
        .rev()
        .nth(CONTEXT_CHARS - 1)
        .map_or(0, |(i, _)| i);
    collapse_whitespace(&s[cut..])
}

fn context_after(md: &str, end: usize) -> String {
    let s = &md[end..];
    let cut = s
        .char_indices()
        .nth(CONTEXT_CHARS)
        .map_or(s.len(), |(i, _)| i);
    collapse_whitespace(&s[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE_MD: &str = "## Page 1\n\n\
        | 指标 | 2023A | 2024A |\n\
        | 营业总收入（百万元） | 602315 | 777102 |\n\
        | 归母净利润（百万元） | 30041 | 53128 |\n\n\
        ## Page 2\n\n\
        比亚迪 2024 年实现营业总收入 7,771.02 亿元，同比增长 29.0%。\n";

    #[test]
    fn exact_match_is_tier_one() {
        let found = locate_quote(TABLE_MD, "同比增长 29.0%").unwrap();
        assert_eq!(found.method, LocateMethod::Exact);
        assert_eq!(
            &TABLE_MD[found.span.start..found.span.end],
            "同比增长 29.0%"
        );
        assert_eq!(found.page, Some(2));
    }

    #[test]
    fn whitespace_differences_fall_to_tier_two() {
        let found = locate_quote(TABLE_MD, "营业总收入7,771.02亿元").unwrap();
        assert_eq!(found.method, LocateMethod::WhitespaceInsensitive);
        assert_eq!(found.hit, "营业总收入 7,771.02 亿元");
        assert_eq!(found.page, Some(2));
    }

    #[test]
    fn reassembled_table_quote_lands_on_the_row() {
        // "column header + row label + value" is never contiguous in the table;
        // tier 3 anchors on 53128 and returns the whole row.
        let found = locate_quote(TABLE_MD, "2024A 归母净利润（百万元） 53128").unwrap();
        assert_eq!(found.method, LocateMethod::TableAnchor);
        assert!(found.hit.contains("53128"));
        assert!(found.hit.contains("归母净利润"));
        assert_eq!(found.page, Some(1));
    }

    #[test]
    fn fabricated_label_on_a_real_number_is_rejected() {
        assert!(locate_quote(TABLE_MD, "海外收入 53128").is_none());
    }

    #[test]
    fn unit_converted_value_falls_to_tier_four() {
        // 7771.5 亿 ≈ 777102 百万 within the 0.2% tolerance, and "7771.5" is
        // not a digit-substring of anything in the table, so only tier 4 hits.
        let found = locate_quote(TABLE_MD, "营业总收入 7771.5 亿").unwrap();
        assert_eq!(found.method, LocateMethod::NumericEquivalence);
        assert!(found.hit.contains("777102"));
        assert_eq!(found.page, Some(1));
    }

    #[test]
    fn synonym_label_survives_on_document_unique_value() {
        // "净利约" never appears in the row label ("归母净利润"), so the label
        // check fails; 531 亿 ≈ 53128 百万 is document-unique, which accepts it.
        let found = locate_quote(TABLE_MD, "净利约 531 亿元").unwrap();
        assert_eq!(found.method, LocateMethod::NumericEquivalence);
        assert!(found.hit.contains("53128"));
    }

    #[test]
    fn fabricated_magnitude_is_rejected() {
        assert!(locate_quote(TABLE_MD, "海外收入 9999 亿").is_none());
    }

    #[test]
    fn empty_and_whitespace_quotes_are_rejected() {
        assert!(locate_quote(TABLE_MD, "").is_none());
        assert!(locate_quote(TABLE_MD, "   ").is_none());
    }

    #[test]
    fn absent_quote_returns_none() {
        assert!(locate_quote(TABLE_MD, "这段话根本不在文档里").is_none());
    }

    #[test]
    fn page_is_none_without_markers() {
        let found = locate_quote("没有页标记的普通文本。", "普通文本").unwrap();
        assert_eq!(found.page, None);
    }

    #[test]
    fn locator_grounds_many_quotes_against_one_index() {
        let locator = Locator::new(TABLE_MD);
        assert!(locator.locate("同比增长 29.0%").is_some());
        assert!(locator.locate("2024A 归母净利润（百万元） 53128").is_some());
        assert!(locator.locate("海外收入 9999 亿").is_none());
    }

    #[test]
    fn context_is_collapsed_and_bounded() {
        let found = locate_quote(TABLE_MD, "同比增长 29.0%").unwrap();
        assert!(!found.before.contains('\n'));
        assert!(found.before.chars().count() <= CONTEXT_CHARS);
        assert!(found.after.chars().count() <= CONTEXT_CHARS);
    }

    #[test]
    fn method_wire_format_is_stable() {
        for method in LocateMethod::ALL {
            assert_eq!(
                serde_json::to_string(&method).unwrap(),
                format!("\"{}\"", method.as_str())
            );
            assert_eq!(method.to_string(), method.as_str());
        }
    }

    #[test]
    fn span_is_a_valid_byte_range() {
        let found = locate_quote(TABLE_MD, "营业总收入7,771.02亿元").unwrap();
        // Slicing at the reported offsets must not split a UTF-8 char.
        let slice = &TABLE_MD[found.span.start..found.span.end];
        assert_eq!(collapse_whitespace(slice), found.hit);
    }

    #[test]
    fn grounded_hit_attaches_best_overlapping_anchor() {
        use crate::result::{ProvenanceSpan, Rect, SourceAnchor, TextRange};
        let md = "## Page 1\n\nRevenue grew 12% in Q1.";
        let line_start = md.find("Revenue").expect("line");
        let spans = vec![
            ProvenanceSpan {
                output: TextRange {
                    start: 0,
                    end: line_start,
                },
                source: SourceAnchor::Page {
                    number: 1,
                    bbox: None,
                },
            },
            ProvenanceSpan {
                output: TextRange {
                    start: line_start,
                    end: md.len(),
                },
                source: SourceAnchor::Page {
                    number: 1,
                    bbox: Some(Rect {
                        x0: 72.0,
                        y0: 700.0,
                        x1: 300.0,
                        y1: 715.0,
                    }),
                },
            },
        ];

        let found = locate_quote_grounded(md, "Revenue grew 12%", &spans).expect("hit");
        let Some(SourceAnchor::Page {
            number,
            bbox: Some(bbox),
        }) = found.anchor
        else {
            panic!("expected boxed page anchor: {:?}", found.anchor);
        };
        assert_eq!(number, 1);
        assert!((bbox.x0 - 72.0).abs() < f32::EPSILON);

        // Without spans the anchor stays empty and serialization omits it.
        let plain = locate_quote(md, "Revenue grew 12%").expect("hit");
        assert_eq!(plain.anchor, None);
        let json = serde_json::to_value(&plain).expect("serialize");
        assert!(json.get("anchor").is_none());
    }
}
