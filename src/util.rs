//! Small utility functions shared across modules.

/// Format an integer with comma thousands separators: `1234567` → `"1,234,567"`.
pub fn fmt_count(n: usize) -> String {
    let s = n.to_string();
    let len = s.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in s.chars().enumerate() {
        let from_right = len - 1 - i;
        if i > 0 && from_right % 3 == 2 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Truncate `s` to at most `max_chars` Unicode characters, appending `…` if cut.
/// The returned string is guaranteed to be at most `max_chars` chars wide.
pub fn truncate(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut chars = s.chars();
    let kept: String = (&mut chars).take(max_chars).collect();
    if chars.next().is_some() {
        let prefix: String = kept.chars().take(max_chars - 1).collect();
        format!("{prefix}…")
    } else {
        kept
    }
}

/// Format `n` as a percentage of `total`, with one decimal place.
pub fn fmt_pct(n: usize, total: usize) -> String {
    if total > 0 {
        format!("{:.1}%", n as f64 * 100.0 / total as f64)
    } else {
        "0%".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_count_thousands() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1_000), "1,000");
        assert_eq!(fmt_count(1_234_567), "1,234,567");
        assert_eq!(fmt_count(12_345_678), "12,345,678");
    }

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("ab", 2), "ab");
    }

    #[test]
    fn truncate_long_string_with_ellipsis() {
        assert_eq!(truncate("hello world", 5), "hell…");
        assert_eq!(truncate("abc", 2), "a…");
    }

    #[test]
    fn truncate_zero_max() {
        assert_eq!(truncate("anything", 0), "");
    }

    #[test]
    fn fmt_pct_basic() {
        assert_eq!(fmt_pct(20, 100), "20.0%");
        assert_eq!(fmt_pct(1, 3), "33.3%");
        assert_eq!(fmt_pct(0, 0), "0%");
    }
}
