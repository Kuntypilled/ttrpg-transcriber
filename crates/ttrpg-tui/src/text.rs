//! Small text helpers for drawing: widths, truncation, wrapping, and making
//! program output safe to show.

use std::time::Duration;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// At most `w` columns; a cut ends in `…`.
pub fn truncate(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw >= w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

/// At most `w` columns, keeping the end and marking a cut with a leading `…`.
/// For paths, whose last part matters most.
pub fn truncate_start(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1; // the ellipsis
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w {
            break;
        }
        kept.push(c);
        used += cw;
    }
    kept.reverse();
    format!("…{}", kept.into_iter().collect::<String>())
}

/// Exactly `w` columns: truncated or padded with spaces.
pub fn fit(s: &str, w: usize) -> String {
    let t = truncate(s, w);
    let pad = w.saturating_sub(width(&t));
    format!("{t}{}", " ".repeat(pad))
}

/// Program output must not move the cursor or recolour the screen. Tabs become
/// spaces and every other control character is dropped.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\t' => Some("    ".to_string()),
            c if c.is_control() => None,
            c => Some(c.to_string()),
        })
        .collect()
}

/// `640 KB`, `22 MB`, `1.4 GB`.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.0} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

/// Word-wrap every line to `w` columns. Blank lines stay as blank lines.
pub fn wrap<'a>(lines: impl IntoIterator<Item = &'a str>, w: usize) -> Vec<String> {
    let w = w.max(1);
    let mut out = Vec::new();
    for line in lines {
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        out.extend(
            textwrap::wrap(line, textwrap::Options::new(w).break_words(true))
                .into_iter()
                .map(|c| c.into_owned()),
        );
    }
    out
}

/// `0:42`, `12:07`, `1:02:03`.
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    let (h, m, sec) = (s / 3600, s % 3600 / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_and_padding_count_columns() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 8), "hello w…");
        assert_eq!(truncate("hello", 0), "");
        assert_eq!(truncate("hello", 1), "…");
        assert_eq!(truncate_start("/a/b/session.flac", 10), "…sion.flac");
        assert_eq!(truncate_start("short", 10), "short");
        assert_eq!(fit("ab", 5), "ab   ");
        assert_eq!(fit("abcdefgh", 5), "abcd…");
        assert_eq!(
            width(&fit("日本語テキスト", 9)),
            9,
            "wide characters count as two columns"
        );
    }

    #[test]
    fn output_is_made_safe() {
        assert_eq!(sanitize("a\tb"), "a    b");
        assert_eq!(sanitize("\x1b[31mred\x1b[0m"), "[31mred[0m");
        assert_eq!(sanitize("bell\x07\r"), "bell");
        assert_eq!(sanitize("It’s fine ✓"), "It’s fine ✓");
    }

    #[test]
    fn sizes_and_clocks() {
        assert_eq!(human_size(500), "500 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(22 * 1024 * 1024), "22 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
        assert_eq!(clock(Duration::from_secs(42)), "0:42");
        assert_eq!(clock(Duration::from_secs(727)), "12:07");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn wrapping_keeps_blank_lines_and_breaks_long_words() {
        let w = wrap(["one two three four", "", "x".repeat(25).as_str()], 10);
        assert_eq!(w[0], "one two");
        assert!(w.contains(&String::new()));
        assert!(w.iter().all(|l| width(l) <= 10), "{w:?}");
    }
}
