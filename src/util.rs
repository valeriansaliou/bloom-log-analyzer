// Bloom Log Analyzer
//
// Log analysis CLI for the Bloom HTTP REST API caching middleware
// Copyright: 2026, Valerian Saliou <valerian@valeriansaliou.name>
// License: Mozilla Public License v2.0 (MPL v2.0)

//! Small utility functions shared across modules.

/// Format an integer with comma thousands separators: `1234567` → `"1,234,567"`.
///
/// Single-allocation: writes digits right-to-left into a fixed-capacity buffer,
/// inserting `,` after every 3 digits, then returns the buffer.  Avoids the
/// `n.to_string()` + char-by-char rebuild pattern that allocates twice.
pub fn fmt_count(n: usize) -> String {
    // 20 digits covers usize::MAX (2^64 ≈ 1.8e19) plus 6 commas = 26 chars max.
    let mut buf = [0u8; 26];
    let mut pos = buf.len();
    let mut n = n;
    let mut digit_count = 0usize;
    loop {
        // Insert a comma every 3 digits (but not at the very front).
        if digit_count > 0 && digit_count.is_multiple_of(3) {
            pos -= 1;
            buf[pos] = b',';
        }
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
        digit_count += 1;
        if n == 0 {
            break;
        }
    }
    // SAFETY: only ASCII digits and commas were written.
    unsafe { std::str::from_utf8_unchecked(&buf[pos..]) }.to_owned()
}

/// Truncate `s` to at most `max_chars` Unicode characters, appending `…` if cut.
/// The returned string is guaranteed to be at most `max_chars` chars wide.
///
/// Single-pass: walks the char iterator once, remembers the byte offset of the
/// `(max_chars - 1)`th character, and slices `s` directly there if we then
/// see a `max_chars`th character (meaning truncation is needed).  Only one
/// `String` allocation in either branch.
pub fn truncate(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut cut_at: Option<usize> = None;
    for (count, (byte_idx, _)) in s.char_indices().enumerate() {
        // Reaching the `max_chars`th char (0-indexed `max_chars`) means s is
        // longer than max_chars — cut at the recorded boundary and append `…`.
        if count >= max_chars {
            let cut = cut_at.unwrap_or(byte_idx);
            let mut out = String::with_capacity(cut + '…'.len_utf8());
            out.push_str(&s[..cut]);
            out.push('…');
            return out;
        }
        // The byte offset where the (max_chars - 1)th char begins is the
        // candidate cut point.  We'd keep s[..byte_idx] (i.e. the first
        // max_chars - 1 chars) plus a `…`, fitting in exactly max_chars chars.
        if count + 1 == max_chars {
            cut_at = Some(byte_idx);
        }
    }
    // s fits in max_chars chars — return as-is.
    s.to_owned()
}

/// Format a byte count as a human-readable size (SI units, one decimal place).
pub fn fmt_bytes(n: u64) -> String {
    const GB: u64 = 1_000_000_000;
    const MB: u64 = 1_000_000;
    const KB: u64 = 1_000;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else {
        format!("{} B", n)
    }
}

/// Divide `data` into up to `n` contiguous byte slices, each ending on a `\n`
/// boundary so no log line is split across chunks.
pub(crate) fn split_into_chunks(data: &[u8], n: usize) -> Vec<&[u8]> {
    let n = n.max(1);
    if data.is_empty() {
        return vec![];
    }
    let chunk_size = data.len().div_ceil(n);
    let mut chunks = Vec::with_capacity(n);
    let mut start = 0;
    while start < data.len() {
        let raw_end = (start + chunk_size).min(data.len());
        let end = if raw_end >= data.len() {
            data.len()
        } else {
            data[raw_end..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(data.len(), |off| raw_end + off + 1)
        };
        chunks.push(&data[start..end]);
        start = end;
    }
    chunks
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
    fn split_into_chunks_covers_all_bytes() {
        let data = b"line1\nline2\nline3\n";
        let chunks = split_into_chunks(data, 3);
        let combined: Vec<u8> = chunks.iter().flat_map(|c| c.iter().copied()).collect();
        assert_eq!(combined.as_slice(), data.as_slice());
    }

    #[test]
    fn split_into_chunks_empty() {
        assert!(split_into_chunks(b"", 4).is_empty());
    }

    #[test]
    fn split_into_chunks_single_chunk_when_small() {
        let data = b"abc\ndef\n";
        let chunks = split_into_chunks(data, 1);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], data.as_slice());
    }

    #[test]
    fn fmt_pct_basic() {
        assert_eq!(fmt_pct(20, 100), "20.0%");
        assert_eq!(fmt_pct(1, 3), "33.3%");
        assert_eq!(fmt_pct(0, 0), "0%");
    }
}
