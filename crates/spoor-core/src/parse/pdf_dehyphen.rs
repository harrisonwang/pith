//! Conservative line-end dehyphenation for PDF text.
//!
//! Justified PDF text breaks words at line ends ("dehyphen-" / "ation"),
//! which splits tokens for retrieval and makes quotes unmatchable. This pass
//! rejoins only the unambiguous cases, character-level (never byte-level):
//!
//! - A line-end `-`/`‐` is a word break only when the characters on *both*
//!   sides are lowercase letters; anything else — digits ("UTF-" / "8"),
//!   capitals, CJK, a free-standing minus — is left exactly as extracted.
//! - A compound broken at one of its own hyphens ("state-of-" / "the-art")
//!   keeps the hyphen and just removes the break; a plain broken word drops
//!   it. The signal is whether the fragment before the break already
//!   contains a hyphen.
//! - Soft hyphens (U+00AD) are typesetting artifacts, never content, and are
//!   removed everywhere.

/// Hyphen characters a justifier may break a word with.
const HYPHENS: [char; 2] = ['-', '\u{2010}'];

fn is_joinable_letter(ch: char) -> bool {
    ch.is_alphabetic() && ch.is_lowercase()
}

enum Join {
    DropHyphen,
    KeepHyphen,
}

/// Decide whether `next` continues a word broken at the end of `previous`.
fn join_mode(previous: &str, next: &str) -> Option<Join> {
    let trimmed = previous.trim_end();
    let hyphen = trimmed
        .chars()
        .next_back()
        .filter(|ch| HYPHENS.contains(ch))?;
    let before = trimmed.chars().rev().nth(1)?;
    if !is_joinable_letter(before) {
        return None;
    }
    let first = next.trim_start().chars().next()?;
    if !is_joinable_letter(first) {
        return None;
    }

    let fragment = trimmed[..trimmed.len() - hyphen.len_utf8()]
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or("");
    if fragment.chars().any(|ch| HYPHENS.contains(&ch)) {
        Some(Join::KeepHyphen)
    } else {
        Some(Join::DropHyphen)
    }
}

/// Rejoin words broken across line ends and strip soft hyphens.
pub(crate) fn dehyphenate(text: &str) -> String {
    let text: String = if text.contains('\u{00AD}') {
        text.chars().filter(|ch| *ch != '\u{00AD}').collect()
    } else {
        text.to_string()
    };

    if !text.contains('\n') {
        return text;
    }

    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let mut current = lines[index].to_string();
        while index + 1 < lines.len() {
            let Some(mode) = join_mode(&current, lines[index + 1]) else {
                break;
            };
            let trimmed = current.trim_end();
            let keep_until = match mode {
                Join::DropHyphen => {
                    let hyphen = trimmed.chars().next_back().expect("hyphen present");
                    trimmed.len() - hyphen.len_utf8()
                }
                Join::KeepHyphen => trimmed.len(),
            };
            current = format!(
                "{}{}",
                &trimmed[..keep_until],
                lines[index + 1].trim_start()
            );
            index += 1;
        }
        out.push(current);
        index += 1;
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::dehyphenate;

    #[test]
    fn broken_word_rejoins_without_hyphen() {
        assert_eq!(
            dehyphenate("applies a conservative dehyphen-\nation pass to line ends."),
            "applies a conservative dehyphenation pass to line ends."
        );
    }

    #[test]
    fn compound_broken_at_inner_hyphen_keeps_it() {
        assert_eq!(
            dehyphenate("reads like state-of-\nthe-art output."),
            "reads like state-of-the-art output."
        );
    }

    #[test]
    fn free_standing_minus_is_not_a_word_break() {
        let text = "total = subtotal -\ndiscount applies here.";
        assert_eq!(dehyphenate(text), text);
    }

    #[test]
    fn uppercase_or_digit_neighbours_stay_split() {
        let acronym = "uses UTF-\n8 style names.";
        assert_eq!(dehyphenate(acronym), acronym);
        let capital = "the Wagner-\nJauregg reaction.";
        assert_eq!(dehyphenate(capital), capital);
    }

    #[test]
    fn cjk_text_is_never_joined() {
        let text = "中文行尾-\n继续的下一行。";
        assert_eq!(dehyphenate(text), text);
    }

    #[test]
    fn soft_hyphens_are_stripped_everywhere() {
        assert_eq!(dehyphenate("soft\u{00AD}ware ships"), "software ships");
    }

    #[test]
    fn cascading_breaks_join_across_lines() {
        assert_eq!(
            dehyphenate("particu-\nlarly in-\nteresting"),
            "particularly interesting"
        );
    }

    #[test]
    fn list_bullet_continuation_stays_split() {
        let text = "items include some-\n- not a continuation";
        assert_eq!(dehyphenate(text), text);
    }
}
