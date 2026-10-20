//! Helpers for handling untrusted text (transcripts, model output, repository
//! text). Nothing here interprets the text; it only bounds and cleans it.

/// Remove control characters except newline and tab, and normalize CRLF.
pub fn strip_controls(input: &str) -> String {
    input
        .replace("\r\n", "\n")
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// Truncate to at most `max_chars` characters, appending an ellipsis marker.
pub fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Clean untrusted text for storage or display: strip control characters,
/// collapse runs of blank lines and bound the length.
pub fn clean(input: &str, max_chars: usize) -> String {
    let stripped = strip_controls(input);
    let mut out = String::with_capacity(stripped.len().min(max_chars * 4));
    let mut blank_run = 0;
    for line in stripped.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    truncate_chars(out.trim(), max_chars)
}

/// Collapse all whitespace to single spaces and bound the length.
pub fn one_line(input: &str, max_chars: usize) -> String {
    let collapsed = strip_controls(input)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    truncate_chars(&collapsed, max_chars)
}

/// Rough token estimate (~4 characters per token) used for retrieval
/// overhead accounting. Deliberately conservative and model-agnostic.
pub fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count() as u32;
    chars.div_ceil(4)
}

/// Contains a NUL byte or other content we refuse to store.
pub fn contains_nul(input: &str) -> bool {
    input.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_controls_and_bounds() {
        let s = "hello\u{1b}[31m world\r\n\n\n\nnext\u{7}";
        assert_eq!(clean(s, 100), "hello[31m world\n\nnext");
        assert_eq!(truncate_chars("abcdef", 4), "abc…");
        assert_eq!(one_line("a \n b\t c", 10), "a b c");
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }
}
