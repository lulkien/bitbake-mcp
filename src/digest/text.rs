//! Text helpers: byte-capped cuts that respect char boundaries, and the
//! `| ` prefix of a dumped task-log line.

pub(super) const ELLIPSIS: &str = "…";

pub(super) fn strip_bar(text: &str) -> &str {
    text.strip_prefix("| ").unwrap_or(text)
}

/// Largest char boundary of `s` at or below `i`.
pub(super) fn floor_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut i = i;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Smallest char boundary of `s` at or above `i`.
pub(super) fn ceil_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Keep the head of `s` within `cap` bytes, `…` marking the cut.
pub fn cut_head(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let end = floor_boundary(s, cap.saturating_sub(ELLIPSIS.len()));
    format!("{}{ELLIPSIS}", &s[..end])
}

/// Keep the tail of `s` within `cap` bytes, `…` marking the cut, so a path
/// keeps its file name.
pub fn cut_tail(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let start = ceil_boundary(s, s.len() - cap.saturating_sub(ELLIPSIS.len()));
    format!("{ELLIPSIS}{}", &s[start..])
}
