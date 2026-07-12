//! PDF outline → Markdown heading promotion.
//!
//! The document outline (bookmarks) is the PDF's own structure declaration —
//! deterministic data, not layout inference — so it is the only heading
//! source spoor uses for PDFs today. A page line is promoted to a heading
//! only when an outline entry points at that page *and* the entry's title is
//! the whole line (modulo whitespace); a title that cannot be found promotes
//! nothing rather than fabricating structure. Font-size-based heading
//! inference stays out until it can carry a confidence signal.
//!
//! `## Page N` markers are spoor's page structure (h2), so outline level 1
//! renders as `###`, level 2 as `####`, deeper levels capping at `######`.

use crate::locate::Locator;

/// Markdown heading level for outline depth `level` (1-based): two below the
/// `## Page N` markers, capped at h6.
fn hashes(level: usize) -> String {
    "#".repeat((level + 2).min(6))
}

/// Promote each outline title on this page to a Markdown heading. Entries are
/// applied in outline order; each promotes at most one line, and a line
/// already promoted is not promoted twice (a duplicate title claims the next
/// occurrence instead).
pub(crate) fn apply_headings(text: &str, headings: &[(usize, String)]) -> String {
    let mut current = text.to_string();
    for (level, title) in headings {
        current = promote_one(&current, *level, title);
    }
    current
}

fn promote_one(text: &str, level: usize, title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        return text.to_string();
    }
    let locator = Locator::new(text);
    for (start, end) in locator.all_occurrences(title) {
        let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
        let line_end = text[end..].find('\n').map_or(text.len(), |at| end + at);
        // Only a line that *is* the title (modulo whitespace) becomes a
        // heading; a paragraph merely containing the words stays prose.
        let hit_is_whole_line =
            text[line_start..start].trim().is_empty() && text[end..line_end].trim().is_empty();
        if !hit_is_whole_line || text[line_start..line_end].trim_start().starts_with('#') {
            continue;
        }

        let mut out = String::with_capacity(text.len() + 8);
        out.push_str(&text[..line_start]);
        out.push_str(&hashes(level));
        out.push(' ');
        out.push_str(text[line_start..line_end].trim());
        out.push_str(&text[line_end..]);
        return out;
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::apply_headings;

    #[test]
    fn whole_line_title_is_promoted_at_outline_depth() {
        let text = "Introduction\nOpening prose follows.\nBackground\nMore prose.";
        let promoted = apply_headings(
            text,
            &[
                (1, "Introduction".to_string()),
                (2, "Background".to_string()),
            ],
        );

        assert_eq!(
            promoted,
            "### Introduction\nOpening prose follows.\n#### Background\nMore prose."
        );
    }

    #[test]
    fn title_inside_a_paragraph_is_not_promoted() {
        let text = "The Introduction chapter explains the goal.";
        let promoted = apply_headings(text, &[(1, "Introduction".to_string())]);
        assert_eq!(promoted, text);
    }

    #[test]
    fn missing_title_promotes_nothing() {
        let text = "Some page text.";
        let promoted = apply_headings(text, &[(1, "Missing Section".to_string())]);
        assert_eq!(promoted, text);
    }

    #[test]
    fn duplicate_titles_claim_successive_occurrences() {
        let text = "Summary\nFirst block.\nSummary\nSecond block.";
        let promoted = apply_headings(
            text,
            &[(1, "Summary".to_string()), (1, "Summary".to_string())],
        );

        assert_eq!(
            promoted,
            "### Summary\nFirst block.\n### Summary\nSecond block."
        );
    }

    #[test]
    fn depth_caps_at_h6() {
        let text = "Deep";
        let promoted = apply_headings(text, &[(9, "Deep".to_string())]);
        assert_eq!(promoted, "###### Deep");
    }

    #[test]
    fn title_with_surrounding_whitespace_still_matches_line() {
        let text = "  Introduction  \nBody text.";
        let promoted = apply_headings(text, &[(1, "Introduction".to_string())]);
        assert_eq!(promoted, "### Introduction\nBody text.");
    }
}
