//! Deterministic quote grounding inside spoor's own Markdown output.
//!
//! An LLM answering from a parsed document cites a quote as its evidence;
//! [`locate_quote`] searches for that text or data in the Markdown spoor
//! actually produced. Five tiers, strictest first, all deterministic:
//!
//! 1. **Exact** substring (skipping matches whose edges split a digit run:
//!    `…3004` must not "verify" inside `…30041…`).
//! 2. **Whitespace-insensitive** (normalized): matching in a normalized
//!    space that folds whitespace, full/half-width and CJK punctuation
//!    variants, ASCII case, emphasis/code markers, line-leading structure
//!    markers (`- `, `1. `, `#`, `>`) and Markdown link syntax — the
//!    formatting a model rewrites without changing the text. The
//!    architecture follows the mature annotation-anchoring pattern (W3C Web
//!    Annotation / Hypothesis): match in normalized space, map offsets back
//!    to the original bytes. A hit that stitches text across a paragraph
//!    boundary (`\n\n`) is reported with `corroborated: false` — present in
//!    the document in that order, but not one contiguous statement.
//! 3. **Fuzzy**: bounded-edit-distance search (seed–extend–filter, the text
//!    reuse detection pipeline) for lightly rewritten quotes. Guarded by a
//!    similarity acceptance threshold, a cap on consecutive edits (so an
//!    aligned span cannot bridge across a sentence to borrow a neighbor's
//!    figure), digit-run edge checks, and a hard constraint that every
//!    number in the quote — including single digits and CJK numerals —
//!    appears (by value) inside the matched span.
//! 4. **Table anchor**: a model quoting table data usually reassembles
//!    "column header + row label + value" into one string that never appears
//!    contiguously in a Markdown table. Anchor on the quote's most
//!    identifiable number — comma-insensitively, with digit-boundary checks —
//!    verify the hit line with the quote's label words, and return the whole
//!    table row as a source candidate. Other figures in the quote must also
//!    appear on the hit row or the table's header row, else the hit is
//!    reported uncorroborated (a right value under a wrong year must not
//!    pass silently).
//! 5. **Numeric equivalence**: the same value written under a different CJK
//!    magnitude unit (7771亿 vs 777102百万). Tried only when the quote carries
//!    an explicit unit, with 0.2% tolerance for rounding, and accepted only
//!    with a label-word hit or a document-unique value; the
//!    [`LocatedQuote::corroborated`] flag reports which of the two held.
//!
//! Known non-goals, so `None` stays honest: no traditional↔simplified
//! folding, no synonym or paraphrase semantics beyond bounded edits, and
//! list enumerators are treated as formatting (spoor itself renumbers
//! ordered lists), so item numbers are neither matched nor verified.
//!
//! `None` means none of these rules found a match in the supplied Markdown.
//! It does not prove the original file lacks the content: a scan, visual, or
//! parse omission may still contain it. A match also does not establish that
//! the cited material supports the claim or that the claim is true — that
//! judgment is not mechanical and spoor does not attempt it.

use crate::result::{ProvenanceSpan, SourceAnchor, TextRange};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Characters of collapsed context returned on each side of the hit.
const CONTEXT_CHARS: usize = 30;
/// Relative tolerance for tier 5: absorbs rounding (1335亿 vs 1334.54亿)
/// without matching a different magnitude.
const NUMERIC_TOLERANCE: f64 = 0.002;
/// Minimum normalized quote length (chars) for the fuzzy tier. Short, generic
/// quotes make approximate search both slow and unreliable (the documented
/// Hypothesis production failure mode), so they stop at the exact tiers.
const FUZZY_MIN_CHARS: usize = 12;
/// Acceptance threshold for a fuzzy hit: 1 − errors/quote_chars must reach
/// this. Stricter than Hypothesis' effective bound because spoor has no
/// stored prefix/suffix context to co-score with.
const FUZZY_MIN_SIMILARITY: f64 = 0.8;
/// Upper bound on normalized quote length (chars) for the fuzzy tier: keeps
/// the alignment windows — O(quote × window) each — from ballooning on a
/// hostile or degenerate "quote". Longer texts still hit the exact tiers.
const FUZZY_MAX_CHARS: usize = 600;
/// Longest run of consecutive insertions or deletions the fuzzy alignment
/// accepts. Scattered small edits are transcription; a longer contiguous gap
/// means the quote glues disjoint document segments (and could borrow a
/// neighboring sentence's figure into the span).
const MAX_EDIT_RUN: usize = 3;
/// At most this many candidate windows are aligned per quote.
const FUZZY_MAX_CANDIDATES: usize = 16;
/// At most this many document hits are collected per seed.
const SEED_MAX_HITS: usize = 32;
/// Occurrence counting cap: enough to say "ambiguous", cheap to compute.
const OCCURRENCE_CAP: usize = 100;

/// The strictest tier that matched. Callers wanting verbatim-grade evidence
/// should accept `Exact`, or `WhitespaceInsensitive` with
/// `corroborated: true` (a normalized hit that does not stitch across
/// paragraph boundaries).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocateMethod {
    Exact,
    WhitespaceInsensitive,
    Fuzzy,
    TableAnchor,
    NumericEquivalence,
}

impl LocateMethod {
    pub const ALL: [LocateMethod; 5] = [
        LocateMethod::Exact,
        LocateMethod::WhitespaceInsensitive,
        LocateMethod::Fuzzy,
        LocateMethod::TableAnchor,
        LocateMethod::NumericEquivalence,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::WhitespaceInsensitive => "whitespace_insensitive",
            Self::Fuzzy => "fuzzy",
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

/// A located text match or source candidate, with its Markdown context.
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
    /// Similarity of a fuzzy hit (`1.0` = no edits); `None` for other tiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// How many places in the document this tier could have matched (capped
    /// at 100). `> 1` means the returned location — and its page/anchor — is
    /// one of several plausible ones; treat position-sensitive conclusions
    /// with care.
    pub occurrences: usize,
    /// `false` flags a hit that needs human judgment before being cited as
    /// evidence: a normalized hit stitched across a paragraph boundary, a
    /// table hit whose other quote figures are absent from the row/header,
    /// or a numeric candidate accepted on value uniqueness alone.
    pub corroborated: bool,
    /// The source anchor whose provenance span overlaps the hit the most,
    /// when the caller supplied the document's provenance spans (see
    /// [`Locator::locate_grounded`]); block-level spans put an approximate
    /// page box here, turning a matched candidate into a highlight target.
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
/// span's source. With block-level provenance this attaches an approximate
/// source-page box to a matched candidate.
pub fn locate_quote_grounded(
    markdown: &str,
    quote: &str,
    spans: &[ProvenanceSpan],
) -> Option<LocatedQuote> {
    Locator::new(markdown).locate_grounded(quote, spans)
}

/// One tier's result before it is dressed up as a [`LocatedQuote`].
struct Hit {
    start: usize,
    end: usize,
    method: LocateMethod,
    score: Option<f64>,
    occurrences: usize,
    corroborated: bool,
}

/// Reusable matcher over one Markdown string. Construction builds two
/// indices once; each [`Locator::locate`] call is then independent and
/// read-only:
///
/// - a whitespace-stripped index (also used by the PDF line anchorer, whose
///   semantics must stay exactly "whitespace-insensitive");
/// - a normalized index (whitespace + width/punctuation folding + structure
///   markers + link syntax) for the quote tiers.
pub struct Locator<'a> {
    md: &'a str,
    /// `md` with all whitespace removed.
    stripped: String,
    /// For every byte of `stripped`, the byte offset in `md` where the owning
    /// char starts.
    map: Vec<usize>,
    /// `md` normalized for quote matching (see [`build_normalized`]).
    norm: String,
    /// For every byte of `norm`, the byte offset in `md` of the owning
    /// original char.
    norm_map: Vec<usize>,
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
        let (norm, norm_map) = build_normalized(markdown);
        Self {
            md: markdown,
            stripped,
            map,
            norm,
            norm_map,
        }
    }

    /// Locate one quote. Returns `None` when no tier finds a match in this
    /// Markdown; it makes no claim about omitted or unparsed source content.
    pub fn locate(&self, quote: &str) -> Option<LocatedQuote> {
        let quote = quote.trim();
        if quote.is_empty() {
            return None;
        }
        let hit = self
            .exact_hit(quote)
            .or_else(|| self.normalized_hit(quote))
            .or_else(|| self.fuzzy_hit(quote))
            .or_else(|| self.anchored_hit(quote))
            .or_else(|| self.numeric_hit(quote))?;
        Some(LocatedQuote {
            span: TextRange {
                start: hit.start,
                end: hit.end,
            },
            before: context_before(self.md, hit.start),
            hit: collapse_whitespace(&self.md[hit.start..hit.end]),
            after: context_after(self.md, hit.end),
            page: page_of(self.md, hit.start),
            method: hit.method,
            score: hit.score,
            occurrences: hit.occurrences,
            corroborated: hit.corroborated,
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

    /// Tier 1: exact substring of the raw Markdown, skipping matches whose
    /// edges split a digit run.
    fn exact_hit(&self, quote: &str) -> Option<Hit> {
        let mut chosen = None;
        let mut occurrences = 0;
        for (at, _) in self.md.match_indices(quote) {
            if splits_digit_run(self.md, at, at + quote.len()) {
                continue;
            }
            occurrences += 1;
            if chosen.is_none() {
                chosen = Some(at);
            }
            if occurrences >= OCCURRENCE_CAP {
                break;
            }
        }
        let start = chosen?;
        Some(Hit {
            start,
            end: start + quote.len(),
            method: LocateMethod::Exact,
            score: None,
            occurrences,
            corroborated: true,
        })
    }

    /// Tier 2: exact match in the normalized space (whitespace, width and
    /// punctuation variants, structure markers, link syntax). The wire name
    /// stays `whitespace_insensitive` for contract stability; semantically it
    /// is "verbatim modulo formatting". A hit whose original span crosses a
    /// paragraph boundary is flagged uncorroborated: the pieces exist in that
    /// order, but they were not one contiguous statement.
    fn normalized_hit(&self, quote: &str) -> Option<Hit> {
        let (needle, _) = build_normalized(quote);
        if needle.is_empty() {
            return None;
        }
        let mut chosen: Option<(usize, usize)> = None;
        let mut occurrences = 0;
        let mut from = 0;
        while let Some(found) = self.norm[from..].find(&needle) {
            let at = from + found;
            if !splits_digit_run(&self.norm, at, at + needle.len()) {
                occurrences += 1;
                if chosen.is_none() {
                    chosen = Some(norm_map_back(self.md, &self.norm_map, at, needle.len()));
                }
                if occurrences >= OCCURRENCE_CAP {
                    break;
                }
            }
            let step = self.norm[at..].chars().next().map_or(1, char::len_utf8);
            from = at + step;
        }
        let (start, end) = chosen?;
        Some(Hit {
            start,
            end,
            method: LocateMethod::WhitespaceInsensitive,
            score: None,
            occurrences,
            corroborated: !self.md[start..end].contains("\n\n"),
        })
    }

    /// Tier 3: bounded-edit-distance search in normalized space.
    /// Seed–extend–filter: k-gram seeds locate candidate windows, a
    /// semi-global alignment (with a cap on consecutive edits) scores each,
    /// and a hit must clear the similarity threshold, digit-run edge checks
    /// and the numeric hard constraint.
    fn fuzzy_hit(&self, quote: &str) -> Option<Hit> {
        let (needle, _) = build_normalized(quote);
        let needle_chars: Vec<char> = needle.chars().collect();
        let quote_len = needle_chars.len();
        if !(FUZZY_MIN_CHARS..=FUZZY_MAX_CHARS).contains(&quote_len) {
            return None;
        }

        let windows = self.seed_windows(&needle, quote_len)?;
        let quote_values = number_values(&needle, 1);
        let quote_numerals = cjk_numeral_runs(&needle);

        struct Candidate {
            norm_start: usize,
            norm_end: usize,
            similarity: f64,
        }
        let mut accepted: Vec<Candidate> = Vec::new();
        for (win_start, win_end) in windows {
            let window = &self.norm[win_start..win_end];
            let window_chars: Vec<char> = window.chars().collect();
            let Some((errors, start_char, end_char)) = align(&needle_chars, &window_chars) else {
                continue;
            };
            let similarity = 1.0 - errors as f64 / quote_len as f64;
            if similarity < FUZZY_MIN_SIMILARITY {
                continue;
            }
            // Char range → byte range within the window.
            let byte_of = |char_idx: usize| -> usize {
                window
                    .char_indices()
                    .nth(char_idx)
                    .map_or(window.len(), |(i, _)| i)
            };
            let norm_start = win_start + byte_of(start_char);
            let norm_end = win_start + byte_of(end_char);
            if norm_end <= norm_start {
                continue;
            }
            // A span edge inside a digit run means the alignment clipped a
            // number ("…3004" landing inside "…30041"); such a candidate can
            // never be honest evidence.
            if splits_digit_run(&self.norm, norm_start, norm_end) {
                continue;
            }
            // The span must not contain more sentence boundaries than the
            // quote itself: extra enders mean the alignment bridged across a
            // sentence to glue disjoint segments (and possibly borrow a
            // neighboring sentence's figure), while presenting the quote as
            // contiguous.
            if sentence_enders(&self.norm[norm_start..norm_end]) > sentence_enders(&needle) {
                continue;
            }
            // Numeric hard constraint: every number the quote carries — down
            // to single digits and CJK numerals — must appear, by value,
            // inside the matched span. A lightly rewritten sentence keeps its
            // figures; a wrong figure must not ride on the surrounding
            // text's similarity.
            let span = &self.norm[norm_start..norm_end];
            if !quote_values.is_empty() {
                let span_values = number_values(span, 1);
                let all_present = quote_values
                    .iter()
                    .all(|qv| value_present(*qv, &span_values, span));
                if !all_present {
                    continue;
                }
            }
            if !quote_numerals.iter().all(|run| span.contains(run.as_str())) {
                continue;
            }
            accepted.push(Candidate {
                norm_start,
                norm_end,
                similarity,
            });
        }
        if accepted.is_empty() {
            return None;
        }
        // Deterministic ranking: best similarity, then leftmost.
        accepted.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.norm_start.cmp(&b.norm_start))
        });
        let occurrences = accepted.len().min(OCCURRENCE_CAP);
        let best = &accepted[0];
        let (start, end) = norm_map_back(
            self.md,
            &self.norm_map,
            best.norm_start,
            best.norm_end - best.norm_start,
        );
        Some(Hit {
            start,
            end,
            method: LocateMethod::Fuzzy,
            score: Some((best.similarity * 1000.0).round() / 1000.0),
            occurrences,
            corroborated: true,
        })
    }

    /// Candidate windows for the fuzzy tier: evenly spaced k-gram seeds from
    /// the quote (the last seed anchored at the quote's tail, so edits at
    /// the head cannot blind the search) are looked up in the normalized
    /// document; hits cluster by diagonal (document offset minus seed
    /// offset), each cluster becoming one window spanning its diagonal range.
    /// Returns `None` when no seed hits — a quote sharing no k-gram with the
    /// document cannot be a light rewrite of it.
    fn seed_windows(&self, needle: &str, quote_chars: usize) -> Option<Vec<(usize, usize)>> {
        let needle_bytes = needle.len();
        let k_chars = if quote_chars >= 24 {
            6
        } else {
            (quote_chars / 3).max(4)
        };
        let char_offsets: Vec<usize> = needle.char_indices().map(|(i, _)| i).collect();
        let seed_count = 8.min(quote_chars / k_chars).max(1);
        let mut diagonals: Vec<i64> = Vec::new();
        for s in 0..seed_count {
            let start_char = if seed_count == 1 {
                0
            } else {
                (quote_chars - k_chars) * s / (seed_count - 1)
            };
            let start_byte = char_offsets[start_char];
            let end_byte = char_offsets
                .get(start_char + k_chars)
                .copied()
                .unwrap_or(needle_bytes);
            let seed = &needle[start_byte..end_byte];
            if seed.is_empty() {
                continue;
            }
            let mut from = 0;
            let mut hits = 0;
            while let Some(found) = self.norm[from..].find(seed) {
                let at = from + found;
                diagonals.push(at as i64 - start_byte as i64);
                hits += 1;
                if hits >= SEED_MAX_HITS {
                    break;
                }
                let step = self.norm[at..].chars().next().map_or(1, char::len_utf8);
                from = at + step;
            }
        }
        if diagonals.is_empty() {
            return None;
        }
        diagonals.sort_unstable();
        // Cluster diagonals; each cluster projects one window covering its
        // full diagonal range (capped so repetitive text cannot balloon it).
        let slack = (needle_bytes / 4 + 16) as i64;
        let max_window = needle_bytes * 3 + 2 * slack as usize;
        let mut windows: Vec<(usize, usize)> = Vec::new();
        let flush = |min_diag: i64, max_diag: i64, windows: &mut Vec<(usize, usize)>| {
            let start = (min_diag - slack).max(0) as usize;
            let end = ((max_diag + needle_bytes as i64 + slack).max(0) as usize)
                .min(self.norm.len())
                .min(start + max_window);
            let start = floor_char_boundary(&self.norm, start);
            let end = ceil_char_boundary(&self.norm, end);
            if start < end {
                windows.push((start, end));
            }
        };
        let mut cluster_min = diagonals[0];
        let mut previous = diagonals[0];
        for &diag in &diagonals[1..] {
            if diag - previous > slack {
                flush(cluster_min, previous, &mut windows);
                cluster_min = diag;
            }
            previous = diag;
        }
        flush(cluster_min, previous, &mut windows);
        windows.dedup();
        windows.truncate(FUZZY_MAX_CANDIDATES);
        Some(windows)
    }

    /// All whitespace-insensitive occurrences of `needle`, as `md` byte ranges.
    /// Also serves PDF link-anchor weaving, which must find every candidate
    /// position for an anchor rather than only the first. Deliberately NOT
    /// the normalized index: PDF line anchoring needs plain
    /// whitespace-insensitive semantics.
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

    /// Occurrences of a number token, comma-insensitive on both sides
    /// ("30,041" finds "30041" and vice versa), whose neighbors are not
    /// digit-continuations: `3004` must not anchor inside `30041`, `0.3004`,
    /// `.3004` or `1,3004`-style tails.
    fn bounded_number_occurrences(&self, anchor: &str) -> Vec<(usize, usize)> {
        let pattern: Vec<u8> = anchor
            .bytes()
            .filter(|b| *b != b',' && !b.is_ascii_whitespace())
            .collect();
        if pattern.is_empty() {
            return Vec::new();
        }
        let s = self.stripped.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < s.len() {
            if s[i] != pattern[0] {
                i += 1;
                continue;
            }
            // Try to match the pattern at i, skipping grouping commas in the
            // document between digits.
            let mut si = i;
            let mut pi = 0;
            while pi < pattern.len() && si < s.len() {
                let b = s[si];
                if b == pattern[pi] {
                    si += 1;
                    pi += 1;
                } else if b == b','
                    && pi > 0
                    && pattern[pi - 1].is_ascii_digit()
                    && pattern[pi].is_ascii_digit()
                {
                    si += 1;
                } else {
                    break;
                }
            }
            if pi == pattern.len() {
                let prev = if i > 0 { Some(s[i - 1]) } else { None };
                let prev2 = if i > 1 { Some(s[i - 2]) } else { None };
                let next = s.get(si).copied();
                let next2 = s.get(si + 1).copied();
                let joined_left = matches!(prev, Some(b) if b.is_ascii_digit())
                    || matches!(prev, Some(b'.'))
                    || (matches!(prev, Some(b','))
                        && matches!(prev2, Some(b) if b.is_ascii_digit()));
                let joined_right = matches!(next, Some(b) if b.is_ascii_digit())
                    || (matches!(next, Some(b'.') | Some(b','))
                        && matches!(next2, Some(b) if b.is_ascii_digit()));
                if !joined_left && !joined_right {
                    out.push((self.map[i], {
                        let last = si - 1;
                        let origin = self.map[last];
                        origin + self.md[origin..].chars().next().map_or(1, char::len_utf8)
                    }));
                }
            }
            i += 1;
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

    /// Tier 4: anchor on the quote's most identifiable number, verify the hit
    /// line with the quote's label words, return the whole table row.
    fn anchored_hit(&self, quote: &str) -> Option<Hit> {
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
        let with_separator: Vec<&str> =
            numbers.iter().copied().filter(|n| has_separator(n)).collect();
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

        // Table anchoring is for tables: a bare year in prose must not become
        // "evidence" for a reassembled data quote. Only occurrences sitting on
        // a `|` table row qualify.
        let occurrences: Vec<(usize, usize)> = self
            .bounded_number_occurrences(anchor)
            .into_iter()
            .filter(|&(start, _)| {
                let (line_start, line_end) = line_bounds(self.md, start);
                self.md[line_start..line_end].trim_start().starts_with('|')
            })
            .collect();
        if occurrences.is_empty() {
            return None;
        }

        // Score each occurrence's line by how many of the quote's label words
        // (row/column names) it contains, to survive number collisions.
        let labels = label_tokens(quote);
        let mut best: Option<(usize, (usize, usize))> = None;
        let mut viable = 0;
        for &(start, end) in &occurrences {
            let (line_start, line_end) = line_bounds(self.md, start);
            let line = &self.md[line_start..line_end];
            let score = labels
                .iter()
                .filter(|w| label_corroborates(line, w))
                .count();
            if labels.is_empty() || score >= 1 {
                viable += 1;
            }
            if best.is_none_or(|(best_score, _)| score > best_score) {
                best = Some((score, (start, end)));
            }
        }
        let (best_score, (start, end)) = best?;

        let mut corroborated;
        if !labels.is_empty() {
            // With label words present, at least one must corroborate the line:
            // a fabricated label riding on a coincidental number is rejected.
            if best_score < 1 {
                return None;
            }
            corroborated = true;
        } else if occurrences.len() == 1 && (anchor.len() >= 4 || anchor_has_separator) {
            // Without labels, only a document-unique, identifiable anchor counts.
            corroborated = false;
        } else {
            return None;
        }

        // Every other identifiable figure in the quote must appear on the hit
        // row or the table's header — and a year must sit in the header cell
        // of the anchor's own column: "2023年…53128" when 53128 is the 2024
        // column must not pass as corroborated evidence.
        let (line_start, line_end) = line_bounds(self.md, start);
        let line = &self.md[line_start..line_end];
        if corroborated {
            let header = table_header_line(self.md, line_start);
            let anchor_header_cell = header.and_then(|h| {
                let cell = cell_index_of(line, start - line_start)?;
                table_cells(h).get(cell).copied()
            });
            let mut context_values = number_values(line, 2);
            if let Some(header) = header {
                context_values.extend(number_values(header, 2));
            }
            let mut anchor_used = false;
            let others_ok = numbers.iter().all(|token| {
                if !anchor_used && *token == anchor {
                    anchor_used = true;
                    return true;
                }
                if token.chars().filter(|c| c.is_ascii_digit()).count() < 2 {
                    return true;
                }
                let Some(value) = numeric_value(token.trim_end_matches('%'), None) else {
                    return true;
                };
                if is_year_token(token) {
                    // Column-aware: the year must label the anchor's column
                    // when the table structure lets us check that.
                    if let Some(cell) = anchor_header_cell {
                        return number_values(cell, 2).contains(&value);
                    }
                }
                context_values.iter().any(|cv| *cv == value)
            });
            corroborated = others_ok;
        }

        // A hit inside a Markdown table row returns the whole row: row label
        // and sibling cells are the evidence a reader needs.
        let (start, end) = if line.trim_start().starts_with('|') {
            trimmed_line_span(line, line_start)
        } else {
            (start, end)
        };
        Some(Hit {
            start,
            end,
            method: LocateMethod::TableAnchor,
            score: None,
            occurrences: viable.clamp(1, OCCURRENCE_CAP),
            corroborated,
        })
    }

    /// Tier 5: numeric equivalence across CJK magnitude units. Enabled only
    /// when the quote's number carries an explicit unit — that is the sign the
    /// model converted units instead of copying the source verbatim.
    fn numeric_hit(&self, quote: &str) -> Option<Hit> {
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
                    let score = labels
                        .iter()
                        .filter(|w| label_corroborates(line, w))
                        .count();
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
        // is strong enough on its own (rescues synonym row labels) but is
        // reported uncorroborated — a fabricated label is indistinguishable
        // from a synonym at this tier.
        let corroborated = !labels.is_empty() && best.score >= 1;
        if !(corroborated || candidates.len() == 1) {
            return None;
        }

        let occurrences = candidates.len().min(OCCURRENCE_CAP);
        let line = &self.md[best.line_start..best.line_end];
        let (start, end) = if line.trim_start().starts_with('|') {
            trimmed_line_span(line, best.line_start)
        } else {
            (best.start, best.start + best.number_len)
        };
        Some(Hit {
            start,
            end,
            method: LocateMethod::NumericEquivalence,
            score: None,
            occurrences,
            corroborated,
        })
    }
}

/// Build the normalized matching space and its offset map. Folds, per char:
/// whitespace removed; full-width ASCII and common CJK punctuation folded to
/// half-width; emphasis/code markers (`*`, `` ` ``) removed; ASCII upper →
/// lower. Structurally: line-leading Markdown markers (`#{1,6} `, `- `/`* `/
/// `+ `, `1. `, `> `) are dropped, and Markdown link syntax `[text](url)`
/// keeps only `text`. Every output byte maps to the byte offset of the
/// original char in `md`, so matches map back to exact original spans (the
/// annotation-anchoring "translate offsets" pattern).
fn build_normalized(md: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(md.len());
    let mut map = Vec::with_capacity(md.len());
    let mut i = 0;
    let mut at_line_start = true;
    while i < md.len() {
        if at_line_start {
            if let Some(skip) = marker_len(&md[i..]) {
                i += skip;
                at_line_start = false;
                continue;
            }
        }
        let ch = md[i..].chars().next().expect("in-bounds char");
        let ch_len = ch.len_utf8();
        if ch == '\n' {
            at_line_start = true;
            i += ch_len;
            continue;
        }
        if ch.is_whitespace() {
            // Indentation before a list marker keeps line-start state.
            i += ch_len;
            continue;
        }
        if matches!(ch, '*' | '`') {
            // Emphasis / inline-code markers from spoor's own emitters.
            i += ch_len;
            continue;
        }
        if ch == '[' {
            i += ch_len;
            at_line_start = false;
            continue;
        }
        if ch == ']' {
            if let Some(skip) = link_url_len(&md[i..]) {
                i += skip;
                at_line_start = false;
                continue;
            }
        }
        let folded = fold_char(ch);
        out.push(folded);
        for _ in 0..folded.len_utf8() {
            map.push(i);
        }
        i += ch_len;
        at_line_start = false;
    }
    (out, map)
}

/// Byte length of a line-leading structural marker (heading hashes, list
/// bullet, short ordered-list number, blockquote), including its trailing
/// space(s). `None` when the line does not start with one. Enumerator digits
/// are deliberately treated as formatting on both the document and quote
/// side: spoor itself renumbers ordered lists, so item numbers are not
/// reliable content.
fn marker_len(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let hashes = bytes.iter().take_while(|b| **b == b'#').count();
    if (1..=6).contains(&hashes) && bytes.get(hashes) == Some(&b' ') {
        return Some(hashes + trailing_spaces(&rest[hashes..]));
    }
    if matches!(bytes.first(), Some(b'-') | Some(b'*') | Some(b'+')) && bytes.get(1) == Some(&b' ')
    {
        return Some(1 + trailing_spaces(&rest[1..]));
    }
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    // Cap at 3 digits so a year ("2024. 事件") is not mistaken for a marker.
    if (1..=3).contains(&digits)
        && bytes.get(digits) == Some(&b'.')
        && bytes.get(digits + 1) == Some(&b' ')
    {
        return Some(digits + 1 + trailing_spaces(&rest[digits + 1..]));
    }
    if bytes.first() == Some(&b'>') && bytes.get(1) == Some(&b' ') {
        return Some(1 + trailing_spaces(&rest[1..]));
    }
    None
}

fn trailing_spaces(s: &str) -> usize {
    s.bytes().take_while(|b| *b == b' ').count()
}

/// Byte length of a `](url)` tail starting at a `]`, when a closing `)`
/// exists before the next newline (bounded lookahead). `None` leaves the
/// bracket to be indexed literally.
fn link_url_len(s: &str) -> Option<usize> {
    let after = s.strip_prefix(']')?;
    let after = after.strip_prefix('(')?;
    let bound = floor_char_boundary(after, after.len().min(1024));
    let close = after[..bound].find(')')?;
    if after[..close].contains('\n') {
        return None;
    }
    Some(1 + 1 + close + 1)
}

/// Per-char folding: full-width ASCII → half-width, common CJK punctuation →
/// ASCII equivalents (corner brackets fold to quotes, the way models
/// transcribe them), ASCII case folded. Digits and separators survive
/// verbatim so numeric constraints keep their teeth.
fn fold_char(c: char) -> char {
    let c = match c as u32 {
        // Full-width ASCII block (！ .. ～) → ASCII.
        0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        _ => c,
    };
    let c = match c {
        '。' => '.',
        '、' => ',',
        '“' | '”' | '„' | '﹁' | '﹂' | '「' | '」' | '『' | '』' => '"',
        '‘' | '’' => '\'',
        '【' | '〔' => '[',
        '】' | '〕' => ']',
        '《' => '<',
        '》' => '>',
        '—' | '–' | '−' | '―' => '-',
        '·' | '•' => '.',
        '…' => '.',
        _ => c,
    };
    c.to_ascii_lowercase()
}

/// Map a byte range of the normalized space back to original `md` bytes.
fn norm_map_back(md: &str, norm_map: &[usize], at: usize, len: usize) -> (usize, usize) {
    let start = norm_map[at];
    let last_origin = norm_map[at + len - 1];
    let last_len = md[last_origin..].chars().next().map_or(1, char::len_utf8);
    (start, last_origin + last_len)
}

/// Sentence-ender count in normalized text (`.` `!` `?` `;` after folding),
/// excluding decimal points (a digit on both sides).
fn sentence_enders(s: &str) -> usize {
    let b = s.as_bytes();
    let mut count = 0;
    for (i, ch) in s.char_indices() {
        if !matches!(ch, '.' | '!' | '?' | ';') {
            continue;
        }
        let decimal = ch == '.'
            && i > 0
            && b[i - 1].is_ascii_digit()
            && b.get(i + 1).is_some_and(|c| c.is_ascii_digit());
        if !decimal {
            count += 1;
        }
    }
    count
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Whether the range's edges fall inside an ASCII digit run — i.e. the match
/// starts or ends mid-number. Digits are single bytes, so byte indexing is
/// UTF-8 safe here.
fn splits_digit_run(text: &str, start: usize, end: usize) -> bool {
    let b = text.as_bytes();
    let head = start > 0
        && b[start - 1].is_ascii_digit()
        && b.get(start).is_some_and(|c| c.is_ascii_digit());
    let tail = end > 0
        && end < b.len()
        && b[end - 1].is_ascii_digit()
        && b[end].is_ascii_digit();
    head || tail
}

/// Semi-global alignment: match all of `needle` against the best substring of
/// `window`, with consecutive insertions/deletions capped at [`MAX_EDIT_RUN`]
/// so the span cannot bridge disjoint document segments. Returns
/// `(edit errors, window start char, window end char)`, or `None` when no
/// path satisfies the run cap. O(m·n) over char slices, both bounded by the
/// fuzzy tier's window sizing. Deterministic tie-breaks: substitution over
/// deletion over insertion, then leftmost start.
fn align(needle: &[char], window: &[char]) -> Option<(usize, usize, usize)> {
    const BLOCKED: usize = usize::MAX / 2;
    #[derive(Clone, Copy)]
    struct Cell {
        cost: usize,
        start: usize,
        ins_run: u8,
        del_run: u8,
        sub_run: u8,
    }
    let m = needle.len();
    let n = window.len();
    let mut prev: Vec<Cell> = (0..=n)
        .map(|j| Cell {
            cost: 0,
            start: j,
            ins_run: 0,
            del_run: 0,
            sub_run: 0,
        })
        .collect();
    let mut cur = prev.clone();
    for i in 1..=m {
        cur[0] = if i <= MAX_EDIT_RUN {
            Cell {
                cost: i,
                start: 0,
                ins_run: 0,
                del_run: i as u8,
                sub_run: 0,
            }
        } else {
            Cell {
                cost: BLOCKED,
                start: 0,
                ins_run: 0,
                del_run: 0,
                sub_run: 0,
            }
        };
        for j in 1..=n {
            let mismatch = needle[i - 1] != window[j - 1];
            let sub = if mismatch && (prev[j - 1].sub_run as usize) >= MAX_EDIT_RUN {
                Cell {
                    cost: BLOCKED,
                    start: 0,
                    ins_run: 0,
                    del_run: 0,
                    sub_run: 0,
                }
            } else {
                Cell {
                    cost: prev[j - 1].cost.saturating_add(usize::from(mismatch)),
                    start: prev[j - 1].start,
                    ins_run: 0,
                    del_run: 0,
                    sub_run: if mismatch { prev[j - 1].sub_run + 1 } else { 0 },
                }
            };
            let del = if (prev[j].del_run as usize) < MAX_EDIT_RUN {
                Cell {
                    cost: prev[j].cost.saturating_add(1),
                    start: prev[j].start,
                    ins_run: 0,
                    del_run: prev[j].del_run + 1,
                    sub_run: 0,
                }
            } else {
                Cell {
                    cost: BLOCKED,
                    start: 0,
                    ins_run: 0,
                    del_run: 0,
                    sub_run: 0,
                }
            };
            let ins = if (cur[j - 1].ins_run as usize) < MAX_EDIT_RUN {
                Cell {
                    cost: cur[j - 1].cost.saturating_add(1),
                    start: cur[j - 1].start,
                    ins_run: cur[j - 1].ins_run + 1,
                    del_run: 0,
                    sub_run: 0,
                }
            } else {
                Cell {
                    cost: BLOCKED,
                    start: 0,
                    ins_run: 0,
                    del_run: 0,
                    sub_run: 0,
                }
            };
            let mut chosen = sub;
            if del.cost < chosen.cost {
                chosen = del;
            }
            if ins.cost < chosen.cost {
                chosen = ins;
            }
            cur[j] = chosen;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let mut best: Option<(usize, usize, usize)> = None;
    for (j, cell) in prev.iter().enumerate() {
        if cell.cost >= BLOCKED {
            continue;
        }
        let candidate = (cell.cost, cell.start, j);
        if best.is_none_or(|(c, s, _)| candidate.0 < c || (candidate.0 == c && cell.start < s)) {
            best = Some(candidate);
        }
    }
    best
}

/// Whether a quote-side numeric value is present among the span's values.
/// Exact match first; an integer quote value also accepts a span value that
/// rounds to it (a model citing `7771亿` for `7,771.02` is rounding, not
/// fabricating); single-digit values 0–10 also accept the corresponding CJK
/// numeral in the span (a model transcribing `三个` as `3 个`).
fn value_present(qv: f64, span_values: &[f64], span: &str) -> bool {
    if span_values
        .iter()
        .any(|sv| *sv == qv || (qv.fract() == 0.0 && sv.round() == qv))
    {
        return true;
    }
    if qv.fract() == 0.0 && (0.0..=10.0).contains(&qv) {
        let numeral = match qv as u32 {
            0 => "〇",
            1 => "一",
            2 => "二",
            3 => "三",
            4 => "四",
            5 => "五",
            6 => "六",
            7 => "七",
            8 => "八",
            9 => "九",
            10 => "十",
            _ => return false,
        };
        if span.contains(numeral) || (qv == 2.0 && span.contains('两')) {
            return true;
        }
    }
    false
}

/// Values of the number tokens in `s` (comma-insensitive, `%` stripped) with
/// at least `min_digits` digits.
fn number_values(s: &str, min_digits: usize) -> Vec<f64> {
    number_tokens(s)
        .into_iter()
        .filter_map(|(start, len)| {
            let token = &s[start..start + len];
            let token = token.strip_suffix('%').unwrap_or(token);
            if token.chars().filter(|c| c.is_ascii_digit()).count() < min_digits {
                return None;
            }
            numeric_value(token, None)
        })
        .collect()
}

/// Maximal runs of CJK numeral characters in `s` — the figures written in
/// characters rather than digits (一/三/几/十…), which the ASCII-digit
/// constraint cannot see.
fn cjk_numeral_runs(s: &str) -> Vec<String> {
    const NUMERALS: &str = "〇零一二三四五六七八九十百千万亿两几";
    let mut out = Vec::new();
    let mut run = String::new();
    for ch in s.chars() {
        if NUMERALS.contains(ch) {
            run.push(ch);
        } else if !run.is_empty() {
            out.push(std::mem::take(&mut run));
        }
    }
    if !run.is_empty() {
        out.push(run);
    }
    out
}

/// The `|`-separated cells of a table row, trimmed.
fn table_cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(str::trim)
        .collect()
}

/// Which cell of the table row `line` the byte offset `pos` falls in.
fn cell_index_of(line: &str, pos: usize) -> Option<usize> {
    let trimmed_lead = line.len() - line.trim_start().len();
    let inner = line.trim_start().strip_prefix('|')?;
    let inner_start = trimmed_lead + 1;
    if pos < inner_start {
        return None;
    }
    let rel = pos - inner_start;
    let mut cell = 0;
    for (i, b) in inner.bytes().enumerate() {
        if i >= rel {
            break;
        }
        if b == b'|' {
            cell += 1;
        }
    }
    Some(cell)
}

/// Whether a number token looks like a calendar year (1900–2099, bare).
fn is_year_token(token: &str) -> bool {
    token.len() == 4
        && token.bytes().all(|b| b.is_ascii_digit())
        && (token.starts_with("19") || token.starts_with("20"))
}

/// The topmost line of the contiguous `|`-table block containing
/// `line_start`, when the hit line is part of a Markdown table — the header
/// row carries the column labels (and years) a reassembled quote refers to.
fn table_header_line(md: &str, line_start: usize) -> Option<&str> {
    let mut header: Option<(usize, usize)> = None;
    let mut cursor = line_start;
    for _ in 0..200 {
        if cursor == 0 {
            break;
        }
        let prev_end = cursor - 1; // the '\n' before the current line
        let prev_start = md[..prev_end].rfind('\n').map_or(0, |i| i + 1);
        let line = &md[prev_start..prev_end];
        if !line.trim_start().starts_with('|') {
            break;
        }
        header = Some((prev_start, prev_end));
        cursor = prev_start;
    }
    header.map(|(s, e)| &md[s..e])
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

/// CJK magnitude units, longest first so `万亿` is not parsed as `万` and
/// `千万` is not parsed as `千`.
const MAGNITUDE_UNITS: [(&str, f64); 6] = [
    ("万亿", 1e12),
    ("千万", 1e7),
    ("百万", 1e6),
    ("亿", 1e8),
    ("万", 1e4),
    ("千", 1e3),
];

fn unit_multiplier(unit: &str) -> f64 {
    MAGNITUDE_UNITS
        .iter()
        .find(|(name, _)| *name == unit)
        .map_or(1.0, |(_, mult)| *mult)
}

/// Match a magnitude unit at the head of `s`, longest first (万亿 before 万).
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

/// Whether `line` corroborates a label run. Exact containment first; a CJK
/// run of 3+ chars also corroborates when a strict majority of its character
/// bigrams appear in the line — a model gluing adjacent characters onto a
/// real label ("2024年归母净利润" → run "年归母净利润", 4 of 5 bigrams) must
/// not defeat a true hit, while near-miss metric names ("净利率" vs
/// "净利润": 1 of 2 bigrams) and fabricated labels ("海外收入" vs a
/// 营业总收入 row: 1 of 3) stay rejected.
fn label_corroborates(line: &str, run: &str) -> bool {
    if line.contains(run) {
        return true;
    }
    let chars: Vec<char> = run.chars().collect();
    if chars.len() < 3 || !chars.iter().all(|c| ('\u{4e00}'..='\u{9fff}').contains(c)) {
        return false;
    }
    let total = chars.len() - 1;
    let hits = chars
        .windows(2)
        .filter(|w| line.contains(&w.iter().collect::<String>()[..]))
        .count();
    hits * 2 > total
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
        assert_eq!(found.occurrences, 1);
        assert!(found.corroborated);
    }

    #[test]
    fn exact_matches_never_end_mid_number() {
        // "…为3004" is a literal substring of "…为30041…" but ends inside the
        // digit run — a wrong figure must not be "verified" by tier 1.
        let md = "2023年归母净利润为30041百万元。\n";
        assert!(locate_quote(md, "归母净利润为3004").is_none());
    }

    #[test]
    fn whitespace_differences_fall_to_tier_two() {
        let found = locate_quote(TABLE_MD, "营业总收入7,771.02亿元").unwrap();
        assert_eq!(found.method, LocateMethod::WhitespaceInsensitive);
        assert_eq!(found.hit, "营业总收入 7,771.02 亿元");
        assert_eq!(found.page, Some(2));
        assert!(found.corroborated);
    }

    #[test]
    fn punctuation_width_variants_match_at_tier_two() {
        // The model rewrote full-width，。 as half-width — the everyday LLM
        // transcription drift that used to return None.
        let md = "营收持续增长，利润稳步提升。\n";
        let found = locate_quote(md, "营收持续增长,利润稳步提升.").unwrap();
        assert_eq!(found.method, LocateMethod::WhitespaceInsensitive);
        assert_eq!(found.hit, "营收持续增长，利润稳步提升。");
    }

    #[test]
    fn corner_bracket_quotes_fold_like_straight_quotes() {
        let md = "他称之为「智能体优先」的架构。\n";
        let found = locate_quote(md, "他称之为\"智能体优先\"的架构").unwrap();
        assert_eq!(found.method, LocateMethod::WhitespaceInsensitive);
    }

    #[test]
    fn quotes_across_bullets_match_and_stay_corroborated() {
        // Consecutive list items are separated by single newlines: joining
        // them is reading the list in order, still verbatim-grade.
        let md = "## Slide 2\n\n### 目标\n\n- 降低运营成本\n- 提升交付效率\n";
        let joined = locate_quote(md, "降低运营成本 提升交付效率").unwrap();
        assert_eq!(joined.method, LocateMethod::WhitespaceInsensitive);
        assert!(joined.corroborated);
    }

    #[test]
    fn quotes_across_paragraph_boundaries_are_flagged() {
        // A heading and a bullet are separate blocks (blank line between):
        // the join exists in document order but is not one contiguous
        // statement, so it is reported uncorroborated for human judgment.
        let md = "## Slide 2\n\n### 目标\n\n- 降低运营成本\n- 提升交付效率\n";
        let with_title = locate_quote(md, "目标 降低运营成本").unwrap();
        assert_eq!(with_title.method, LocateMethod::WhitespaceInsensitive);
        assert!(!with_title.corroborated);
    }

    #[test]
    fn link_anchor_text_matches_without_the_url() {
        let md = "详见[公司官网](https://example.com)公告全文。\n";
        let found = locate_quote(md, "详见公司官网公告全文").unwrap();
        assert_eq!(found.method, LocateMethod::WhitespaceInsensitive);
    }

    #[test]
    fn long_cjk_text_after_a_short_link_does_not_panic() {
        // Regression: the link-tail lookahead used to slice at a fixed byte
        // 1024, panicking mid-char when the tail was CJK prose.
        let tail = "这是一段很长的中文叙述,用来把链接后的第一千零二十四字节推进多字节字符内部。".repeat(30);
        let md = format!("详见[官网](u)。{tail}\n");
        let locator = Locator::new(&md);
        assert!(locator.locate("这是一段很长的中文叙述").is_some());
        // And a hostile quote with an unterminated link tail must not panic.
        let hostile = format!("]({}文", "a".repeat(1023));
        assert!(locate_quote(&md, &hostile).is_none());
    }

    #[test]
    fn emphasis_markers_do_not_break_matching() {
        let md = "**核心结论**:营收持续增长。\n";
        let found = locate_quote(md, "核心结论:营收持续增长").unwrap();
        assert_eq!(found.method, LocateMethod::WhitespaceInsensitive);
    }

    #[test]
    fn lightly_rewritten_quote_matches_at_fuzzy_with_score() {
        // "年" → "年度": one edit on a ~20-char quote. Figures intact.
        let found =
            locate_quote(TABLE_MD, "比亚迪2024年度实现营业总收入7,771.02亿元").unwrap();
        assert_eq!(found.method, LocateMethod::Fuzzy);
        let score = found.score.expect("fuzzy carries a score");
        assert!(score >= 0.9, "one edit should score high, got {score}");
        assert_eq!(found.page, Some(2));
    }

    #[test]
    fn fuzzy_rejects_wrong_figures_even_with_similar_text() {
        // Same sentence but a wrong number: the numeric hard constraint must
        // refuse to let surrounding-text similarity "verify" it.
        assert!(locate_quote(TABLE_MD, "比亚迪2024年度实现营业总收入8,881.02亿元").is_none());
    }

    #[test]
    fn fuzzy_rejects_wrong_single_digit_figures() {
        // 5% → 9%: single digits are figures too.
        let md = "本年度公司净利率提升至5%,创下历史新高。\n";
        assert!(locate_quote(md, "本年度公司净利率提升至9%,创新高").is_none());
        let genuine = locate_quote(md, "本年度公司净利率提升至5%,创新高").unwrap();
        assert_eq!(genuine.method, LocateMethod::Fuzzy);
    }

    #[test]
    fn fuzzy_rejects_wrong_cjk_numerals() {
        // 阶段一 vs 阶段四: CJK numerals are figures the ASCII constraint
        // cannot see; they get their own check.
        let md = "阶段四的定位是完整的执行系统与治理框架的组合形态。\n";
        assert!(locate_quote(md, "阶段一的定位是完整的执行系统与治理组合形态").is_none());
    }

    #[test]
    fn fuzzy_accepts_digit_for_cjk_numeral_transcription() {
        let md = "公司目前在欧洲共设有三个生产基地,分布于两国。\n";
        let found = locate_quote(md, "公司目前在欧洲共设有3个生产基地,分布两国").unwrap();
        assert_eq!(found.method, LocateMethod::Fuzzy);
    }

    #[test]
    fn fuzzy_rounding_tolerates_integer_citations() {
        // A model citing 7,771 for 7,771.02 is rounding, not fabricating.
        let md = "比亚迪本年度实现营业总收入 7,771.02 亿元,继续领跑行业。\n";
        let found = locate_quote(md, "比亚迪本年度实现营业总收入7771亿元,继续领跑").unwrap();
        assert_eq!(found.method, LocateMethod::Fuzzy);
    }

    #[test]
    fn fuzzy_cannot_bridge_sentences_to_borrow_figures() {
        // The aligned span must not skip "。乙产品" to attribute 乙产品's
        // figure to 甲产品: edit-run caps and the sentence-boundary budget
        // keep this quote out of every TEXT tier. The numeric tier may still
        // surface "120万台" as a labeled data candidate — that is its
        // documented "needs human confirmation" contract, not verification
        // of the stitched sentence.
        let md = "关于甲产品,公司披露其市场份额持续提升并保持行业领先地位。乙产品销量为120万台。\n";
        let quote = "关于甲产品,公司披露其市场份额持续提升并保持行业领先地位,销量为120万台";
        match locate_quote(md, quote) {
            None => {}
            Some(found) => assert_eq!(
                found.method,
                LocateMethod::NumericEquivalence,
                "a stitched quote must never pass a text tier"
            ),
        }
    }

    #[test]
    fn fuzzy_span_cannot_end_mid_number() {
        // The alignment must not clip "30041" to "3004" and then pass the
        // numeric constraint on the clipped span.
        let md = "2023年归母净利润为30041百万元,超出市场预期。\n";
        assert!(locate_quote(md, "2023年度归母净利润为3004").is_none());
    }

    #[test]
    fn fuzzy_survives_edits_at_the_quote_head() {
        // Seeds sample through the tail, so edits clustered at the head do
        // not blind the search.
        let md = "该平台支持跨部门协作与统一权限治理,已在三个事业群落地。\n";
        let found = locate_quote(md, "此平台支撑跨部门协作与统一权限治理,已在三个事业群落地").unwrap();
        assert_eq!(found.method, LocateMethod::Fuzzy);
    }

    #[test]
    fn fuzzy_skips_short_quotes() {
        // A short generic quote must not enter approximate search (the
        // documented Hypothesis performance/reliability failure mode).
        let md = "增长很快。\n";
        assert!(locate_quote(md, "增长飞快").is_none());
    }

    #[test]
    fn wrong_number_cannot_anchor_inside_a_longer_one() {
        // 3004 is a digit-substring of 30041; the boundary guard must reject
        // it instead of "verifying" a wrong figure with the right row.
        assert!(locate_quote(TABLE_MD, "归母净利润 3004").is_none());
        // Leading-dot decimals join too: .3004 must not verify 3004.
        let md = "| 增长率 | .3004 |\n";
        assert!(locate_quote(md, "增长率 3004").is_none());
    }

    #[test]
    fn thousands_separator_mismatch_still_anchors() {
        // Model writes 30,041 for the table's 30041 (or vice versa): the
        // anchor search is comma-insensitive in both directions.
        let grouped = locate_quote(TABLE_MD, "归母净利润 30,041").unwrap();
        assert_eq!(grouped.method, LocateMethod::TableAnchor);
        assert!(grouped.hit.contains("30041"));

        let md = "| 归母净利润（百万元） | 30,041 | 53,128 |\n";
        let plain = locate_quote(md, "归母净利润 30041").unwrap();
        assert_eq!(plain.method, LocateMethod::TableAnchor);
    }

    #[test]
    fn reassembled_table_quote_lands_on_the_row() {
        // "column header + row label + value" is never contiguous in the table;
        // tier 4 anchors on 53128 and returns the whole row. The year 2024
        // appears in the table's header row, corroborating the pairing.
        let found = locate_quote(TABLE_MD, "2024A 归母净利润（百万元） 53128").unwrap();
        assert_eq!(found.method, LocateMethod::TableAnchor);
        assert!(found.hit.contains("53128"));
        assert!(found.hit.contains("归母净利润"));
        assert_eq!(found.page, Some(1));
        assert!(found.corroborated);
    }

    #[test]
    fn glued_cjk_labels_still_corroborate() {
        // "2024年归母净利润53128": the CJK run "年归母净利润" is not a
        // substring of the row label, but 4 of its 5 bigrams are — the quote
        // is genuine and must land, corroborated (2024 sits in the header).
        let found = locate_quote(TABLE_MD, "2024年归母净利润53128").unwrap();
        assert_eq!(found.method, LocateMethod::TableAnchor);
        assert!(found.hit.contains("53128"));
        assert!(found.corroborated);
    }

    #[test]
    fn wrong_year_pairing_is_flagged() {
        // 53128 is the 2024 column; a quote pairing it with 2023 still lands
        // on the row (the value is real) but must not read as corroborated.
        let found = locate_quote(TABLE_MD, "2023年归母净利润53128").unwrap();
        assert_eq!(found.method, LocateMethod::TableAnchor);
        assert!(!found.corroborated);
    }

    #[test]
    fn near_miss_metric_names_do_not_corroborate() {
        // 净利率 is one character off 净利润 (1 of 2 bigrams): a strict
        // bigram majority keeps ratio-vs-amount confusions out.
        assert!(locate_quote(TABLE_MD, "净利率 53128").is_none());
        assert!(locate_quote(TABLE_MD, "利润率 53128").is_none());
    }

    #[test]
    fn fabricated_label_on_a_real_number_is_rejected() {
        assert!(locate_quote(TABLE_MD, "海外收入 53128").is_none());
        assert!(locate_quote(TABLE_MD, "海外收入 602315").is_none());
    }

    #[test]
    fn unit_converted_value_falls_to_numeric_tier() {
        // 7771.5 亿 ≈ 777102 百万 within the 0.2% tolerance, and "7771.5" is
        // not a digit-substring of anything in the table, so only tier 5 hits.
        let found = locate_quote(TABLE_MD, "营业总收入 7771.5 亿").unwrap();
        assert_eq!(found.method, LocateMethod::NumericEquivalence);
        assert!(found.hit.contains("777102"));
        assert_eq!(found.page, Some(1));
        assert!(found.corroborated);
    }

    #[test]
    fn wan_yi_and_qian_wan_units_parse_correctly() {
        // 万亿 must not be read as 万 (an off-by-1e8 that would let absurd
        // magnitudes "verify"), and 千万 must not be read as 千.
        let md = "全年 GDP 总量为 2.75万亿 元。\n";
        let found = locate_quote(md, "GDP 达 27500 亿元").unwrap();
        assert_eq!(found.method, LocateMethod::NumericEquivalence);
        assert!(locate_quote(md, "GDP 达 27500 万元").is_none());
    }

    #[test]
    fn synonym_labels_fall_to_unique_value_rescue_uncorroborated() {
        // "净利约" shares only 1 of 2 bigrams with 归母净利润 — under the
        // strict majority rule that is not corroboration, so the hit rides on
        // value uniqueness and says so.
        let found = locate_quote(TABLE_MD, "净利约 531 亿元").unwrap();
        assert_eq!(found.method, LocateMethod::NumericEquivalence);
        assert!(found.hit.contains("53128"));
        assert!(!found.corroborated);
    }

    #[test]
    fn fabricated_magnitude_is_rejected() {
        assert!(locate_quote(TABLE_MD, "海外收入 9999 亿").is_none());
    }

    #[test]
    fn repeated_quotes_disclose_ambiguity() {
        let md = "## Page 1\n\n净利润率保持在 15% 的水平。\n\n## Page 9\n\n净利润率保持在 15% 的水平。\n";
        let found = locate_quote(md, "净利润率保持在 15% 的水平").unwrap();
        assert_eq!(found.occurrences, 2, "two plausible locations must be disclosed");
        assert_eq!(found.page, Some(1), "deterministically the first");
    }

    #[test]
    fn empty_and_whitespace_quotes_are_rejected() {
        assert!(locate_quote(TABLE_MD, "").is_none());
        assert!(locate_quote(TABLE_MD, "   ").is_none());
    }

    #[test]
    fn absent_quote_returns_none() {
        assert!(locate_quote(TABLE_MD, "这段话根本不在文档里,一个字都对不上").is_none());
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
    fn new_fields_serialize_predictably() {
        let found = locate_quote(TABLE_MD, "同比增长 29.0%").unwrap();
        let json = serde_json::to_value(&found).unwrap();
        assert!(json.get("score").is_none(), "no score outside fuzzy");
        assert_eq!(json["occurrences"], 1);
        assert_eq!(json["corroborated"], true);

        let fuzzy =
            locate_quote(TABLE_MD, "比亚迪2024年度实现营业总收入7,771.02亿元").unwrap();
        let json = serde_json::to_value(&fuzzy).unwrap();
        assert!(json["score"].as_f64().is_some());
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
