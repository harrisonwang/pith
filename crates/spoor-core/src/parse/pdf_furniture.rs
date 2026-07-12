//! PDF repeated-region (header/footer) classification and deduplication.
//!
//! Running headers, footers and page numbers repeat on (nearly) every page;
//! left in the output they pollute retrieval, chunking and the LLM's context
//! with dozens of copies of the same line. This pass classifies a line as
//! page furniture only from cross-page evidence — the same normalized text at
//! the same page edge on enough pages — and then *deduplicates*: the first
//! occurrence stays in the output, later repeats are dropped, and a stable
//! warning names what was removed. Nothing is silently or permanently lost:
//! `DocumentFilter::keep_repeated_regions` turns the pass off entirely for
//! consumers that need verbatim page text (for example to ground quotes
//! against unmodified output).
//!
//! Classification is deliberately statistical, not positional guessing: a
//! single-page document is never touched, and a line must repeat on at least
//! [`MIN_REPEATS`] pages and a clear majority of all pages before it counts.

use std::collections::HashMap;

/// Lines from each page edge that qualify as furniture candidates.
const EDGE_LINES: usize = 2;
/// A normalized line must appear on at least this many pages…
const MIN_REPEATS: usize = 3;
/// …and on at least this fraction of all parsed pages (3/5).
const MIN_PAGE_FRACTION_NUM: usize = 3;
const MIN_PAGE_FRACTION_DEN: usize = 5;
/// Longer lines are prose, not furniture.
const MAX_FURNITURE_CHARS: usize = 120;

/// One deduplicated region: what it looked like and where it repeated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemovedRegion {
    /// The first occurrence's exact text (kept in the output).
    pub(crate) sample: String,
    /// 1-based pages the region was removed from (excludes the kept one).
    pub(crate) removed_from_pages: Vec<usize>,
}

/// Normalize a candidate line so per-page variation ("Page 1 of 4" vs
/// "Page 2 of 4") maps to one key: collapse whitespace, fold digit runs, and
/// lowercase ASCII.
fn normalize(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_space = false;
    let mut in_digits = false;
    for ch in line.trim().chars() {
        if ch.is_whitespace() {
            in_space = true;
            continue;
        }
        if ch.is_ascii_digit() {
            if !in_digits {
                if in_space && !out.is_empty() {
                    out.push(' ');
                }
                in_space = false;
                out.push('#');
                in_digits = true;
            }
            continue;
        }
        in_digits = false;
        if in_space && !out.is_empty() {
            out.push(' ');
        }
        in_space = false;
        for lower in ch.to_lowercase() {
            out.push(lower);
        }
    }
    out
}

/// Furniture candidates for one page: normalized text plus the line index,
/// for lines sitting within [`EDGE_LINES`] of the top or bottom edge. The
/// edge is a *qualifier* (furniture lives at page edges), not part of the
/// region's identity — the same running header counts once wherever it sits.
fn page_candidates(text: &str) -> Vec<(String, usize)> {
    let lines: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .collect();
    let mut out = Vec::new();
    for (position, (line_index, line)) in lines.iter().enumerate() {
        if line.trim().chars().count() > MAX_FURNITURE_CHARS {
            continue;
        }
        let from_end = lines.len() - 1 - position;
        if position < EDGE_LINES || from_end < EDGE_LINES {
            out.push((normalize(line), *line_index));
        }
    }
    out
}

/// Deduplicate cross-page repeated regions in-place. Returns the regions that
/// were classified together with the pages they were removed from; the
/// caller turns those into warnings.
pub(crate) fn dedupe_repeated_regions(pages: &mut [(usize, String)]) -> Vec<RemovedRegion> {
    if pages.len() < MIN_REPEATS {
        return Vec::new();
    }

    // Pass 1: count on how many pages each normalized edge line occurs.
    let mut page_counts: HashMap<String, usize> = HashMap::new();
    for (_, text) in pages.iter() {
        let mut seen_on_page: Vec<String> = Vec::new();
        for (normalized, _) in page_candidates(text) {
            if normalized.is_empty() {
                continue;
            }
            if !seen_on_page.contains(&normalized) {
                *page_counts.entry(normalized.clone()).or_default() += 1;
                seen_on_page.push(normalized);
            }
        }
    }

    let needed = MIN_REPEATS.max(pages.len() * MIN_PAGE_FRACTION_NUM / MIN_PAGE_FRACTION_DEN);
    let classified: HashMap<String, usize> = page_counts
        .into_iter()
        .filter(|(_, count)| *count >= needed)
        .collect();
    if classified.is_empty() {
        return Vec::new();
    }

    // Furniture surrounds content. If the classification would leave any
    // text-bearing page without a single body line, it grabbed the body
    // itself (short pages of repetitive text) — abort the whole pass rather
    // than gut a page.
    for (_, text) in pages.iter() {
        let mut has_any = false;
        let mut has_body = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            has_any = true;
            if !classified.contains_key(&normalize(trimmed)) {
                has_body = true;
                break;
            }
        }
        if has_any && !has_body {
            return Vec::new();
        }
    }

    // Pass 2: keep each region's first occurrence, drop later repeats.
    let mut first_seen: HashMap<String, String> = HashMap::new();
    let mut removed: HashMap<String, Vec<usize>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (number, text) in pages.iter_mut() {
        let mut drop_lines: Vec<usize> = Vec::new();
        for (normalized, line_index) in page_candidates(text) {
            let key = normalized;
            if !classified.contains_key(&key) {
                continue;
            }
            match first_seen.get(&key) {
                None => {
                    let sample = text
                        .lines()
                        .nth(line_index)
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    first_seen.insert(key.clone(), sample);
                    order.push(key);
                }
                Some(_) => {
                    if !drop_lines.contains(&line_index) {
                        drop_lines.push(line_index);
                    }
                    removed.entry(key).or_default().push(*number);
                }
            }
        }
        if !drop_lines.is_empty() {
            *text = text
                .lines()
                .enumerate()
                .filter(|(index, _)| !drop_lines.contains(index))
                .map(|(_, line)| line)
                .collect::<Vec<_>>()
                .join("\n")
                .trim_matches('\n')
                .to_string();
        }
    }

    order
        .into_iter()
        .filter_map(|key| {
            let mut pages_removed = removed.remove(&key)?;
            pages_removed.dedup();
            Some(RemovedRegion {
                sample: first_seen.remove(&key).unwrap_or_default(),
                removed_from_pages: pages_removed,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::dedupe_repeated_regions;

    fn pages(texts: &[&str]) -> Vec<(usize, String)> {
        texts
            .iter()
            .enumerate()
            .map(|(index, text)| (index + 1, text.to_string()))
            .collect()
    }

    #[test]
    fn repeated_header_keeps_first_occurrence_only() {
        let mut input = pages(&[
            "ACME Report\nRevenue grew.",
            "ACME Report\nCosts fell.",
            "ACME Report\nOutlook improved.",
            "ACME Report\nAppendix follows.",
        ]);

        let removed = dedupe_repeated_regions(&mut input);

        assert_eq!(input[0].1, "ACME Report\nRevenue grew.");
        assert_eq!(input[1].1, "Costs fell.");
        assert_eq!(input[2].1, "Outlook improved.");
        assert_eq!(input[3].1, "Appendix follows.");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].sample, "ACME Report");
        assert_eq!(removed[0].removed_from_pages, vec![2, 3, 4]);
    }

    #[test]
    fn page_numbers_normalize_across_digits() {
        let mut input = pages(&[
            "Body one.\nPage 1 of 3",
            "Body two.\nPage 2 of 3",
            "Body three.\nPage 3 of 3",
        ]);

        let removed = dedupe_repeated_regions(&mut input);

        assert_eq!(input[0].1, "Body one.\nPage 1 of 3");
        assert_eq!(input[1].1, "Body two.");
        assert_eq!(input[2].1, "Body three.");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].sample, "Page 1 of 3");
    }

    #[test]
    fn pages_made_entirely_of_repeats_abort_the_pass() {
        // Every line on every page repeats after digit folding (short
        // repetitive pages): treating them as furniture would empty the
        // pages, so nothing is removed.
        let mut input = pages(&[
            "Page 1 content begins here.\nSome text on this page.",
            "Page 2 content begins here.\nSome text on this page.",
            "Page 3 content begins here.\nSome text on this page.",
        ]);

        let removed = dedupe_repeated_regions(&mut input);
        assert!(removed.is_empty(), "{removed:?}");
        assert!(input[1].1.contains("Some text on this page."));
    }

    #[test]
    fn two_page_documents_are_never_touched() {
        let mut input = pages(&["Header\nBody.", "Header\nMore body."]);
        let removed = dedupe_repeated_regions(&mut input);
        assert!(removed.is_empty());
        assert_eq!(input[0].1, "Header\nBody.");
        assert_eq!(input[1].1, "Header\nMore body.");
    }

    #[test]
    fn body_prose_that_repeats_only_twice_stays() {
        let mut input = pages(&[
            "Header X\nUnique alpha.",
            "Header X\nUnique beta.",
            "Header X\nUnique gamma.",
            "Other start\nUnique delta.",
        ]);

        let removed = dedupe_repeated_regions(&mut input);

        // "Header X" repeats on 3 of 4 pages — classified; "Other start" and
        // body lines are untouched.
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].sample, "Header X");
        assert!(input[3].1.contains("Other start"));
    }

    #[test]
    fn mid_page_repeats_are_not_furniture() {
        // The repeated sentence sits in the middle of enough surrounding
        // lines that it never falls in an edge zone.
        let body = |unique: &str| {
            format!("Top line intro.\nSecond line here.\nShared refrain.\n{unique}\nClosing line.")
        };
        let mut input = pages(&[
            &body("alpha"),
            &body("beta"),
            &body("gamma"),
            &body("delta"),
        ]);

        let removed = dedupe_repeated_regions(&mut input);
        // "Top line intro." / "Closing line." repeat at the edges and are
        // classified; the mid-page refrain must not be.
        assert!(
            removed.iter().all(|r| r.sample != "Shared refrain."),
            "mid-page repetition must not classify as furniture: {removed:?}"
        );
        assert!(
            input
                .iter()
                .all(|(_, text)| text.contains("Shared refrain."))
        );
    }

    #[test]
    fn cjk_page_furniture_is_classified() {
        let mut input = pages(&[
            "机密文件\n正文第一页。\n第 1 页",
            "机密文件\n正文第二页。\n第 2 页",
            "机密文件\n正文第三页。\n第 3 页",
        ]);

        let removed = dedupe_repeated_regions(&mut input);

        assert_eq!(removed.len(), 2);
        assert_eq!(input[1].1, "正文第二页。");
        assert_eq!(input[2].1, "正文第三页。");
        assert!(input[0].1.contains("机密文件"));
        assert!(input[0].1.contains("第 1 页"));
    }
}
