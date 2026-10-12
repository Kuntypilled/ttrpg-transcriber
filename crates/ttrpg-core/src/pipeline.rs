//! The post-processing stages of `transcribe.sh` (PORTING.md sections 4.6 and 4.7) in
//! Rust: whisper JSON to filtered, collapsed rows per track, then the merged,
//! consolidated and Markdown transcripts. Pure functions on text, so they can be
//! checked byte for byte against what the script wrote (`tests/pipeline.rs`).
//!
//! Where this differs from the script on purpose it is listed in `ERRATA.md`.

use std::cmp::Ordering;

use serde_json::Value;

use crate::stock::StockRule;

/// Default filter words, exactly as the script has them.
pub const DEFAULT_IGNORE_WORDS: [&str; 8] = ["you", "silence", "um", "uh", "ah", "like", "right", "well"];

/// One whisper segment. Times are milliseconds. `text` has its one leading space
/// removed and is jq `@tsv` escaped, as in the script's `raw.tsv` (so a tab, newline,
/// carriage return or backslash inside a segment shows up as `\t`, `\n`, `\r`, `\\`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSegment {
    pub start: i64,
    pub end: i64,
    pub text: String,
}

/// One kept row of a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub speaker: String,
    pub start: i64,
    pub end: i64,
    pub text: String,
}

pub const TSV_HEADER: &str = "speaker\tstart\tend\ttext\n";

/// What post-processing needs besides the segments.
#[derive(Debug, Clone)]
pub struct PostOptions {
    pub ignore_words: Vec<String>,
    pub stock: StockRule,
}

impl Default for PostOptions {
    fn default() -> Self {
        Self {
            ignore_words: DEFAULT_IGNORE_WORDS.iter().map(|w| (*w).to_string()).collect(),
            stock: StockRule::default(),
        }
    }
}

/// jq's `@tsv` escaping of one string.
fn tsv_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out
}

/// `.transcription[] | [offsets.from, offsets.to, (.text | ltrimstr(" "))]`
pub fn segments_from_json(json: &str) -> Result<Vec<RawSegment>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("not valid JSON: {e}"))?;
    let list = v
        .get("transcription")
        .and_then(Value::as_array)
        .ok_or_else(|| "no \"transcription\" list".to_string())?;
    let mut out = Vec::with_capacity(list.len());
    for (i, seg) in list.iter().enumerate() {
        let ms = |key: &str| {
            seg.get("offsets")
                .and_then(|o| o.get(key))
                .and_then(|n| n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)))
                .ok_or_else(|| format!("segment {i} has no offsets.{key}"))
        };
        let text = seg.get("text").and_then(Value::as_str).unwrap_or("");
        let text = text.strip_prefix(' ').unwrap_or(text);
        out.push(RawSegment {
            start: ms("from")?,
            end: ms("to")?,
            text: tsv_escape(text),
        });
    }
    Ok(out)
}

/// bash's `[[:space:]]` in a UTF-8 glibc locale.
fn is_bash_space(c: char) -> bool {
    matches!(c,
        '\t'..='\r' | ' ' | '\u{1680}' | '\u{2000}'..='\u{2006}' | '\u{2008}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{205f}' | '\u{3000}')
}

/// The script's `should_filter`: the whole line, or the line without one trailing
/// `.,!?;:`, equals a filter word, ignoring case.
fn should_filter(text: &str, words: &[String]) -> bool {
    let lower = text.to_lowercase();
    let stripped = text
        .strip_suffix(['.', ',', '!', '?', ';', ':'])
        .map(str::to_lowercase)
        .unwrap_or_else(|| lower.clone());
    words.iter().any(|w| {
        let w = w.to_lowercase();
        lower == w || stripped == w
    })
}

/// The script's `is_stock_segment` (see `ERRATA.md`).
fn is_stock(row: &Row, rule: &StockRule) -> bool {
    if !rule.enabled() || row.end - row.start < i64::from(rule.min_seconds) * 1000 {
        return false;
    }
    let lower = row.text.to_lowercase();
    let stripped = row
        .text
        .strip_suffix(['.', ',', '!', '?', ';', ':'])
        .map(str::to_lowercase)
        .unwrap_or_else(|| lower.clone());
    rule.phrases.iter().any(|p| {
        let p = p.to_lowercase();
        lower == p || stripped == p
    })
}

/// What `process_whisper_tsv` counted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Processed {
    pub rows: Vec<Row>,
    pub filtered: usize,
    pub collapsed: usize,
    pub stock_dropped: usize,
}

impl Processed {
    /// The line the script logs: the stock count is added only when it is not zero.
    pub fn log_line(&self, file: &str) -> String {
        let mut s = format!(
            "Processed: {file} — filtered: {}, collapsed: {}",
            self.filtered, self.collapsed
        );
        if self.stock_dropped > 0 {
            s.push_str(&format!(", stock phrases dropped: {}", self.stock_dropped));
        }
        s
    }
}

/// Filter, collapse repeats, and drop long stock phrases, for one track.
pub fn process_segments(speaker: &str, segments: &[RawSegment], opts: &PostOptions) -> Processed {
    let mut out = Processed::default();
    let mut prev: Option<Row> = None;
    let flush = |row: Option<Row>, out: &mut Processed| {
        if let Some(row) = row {
            if is_stock(&row, &opts.stock) {
                out.stock_dropped += 1;
            } else {
                out.rows.push(row);
            }
        }
    };
    for seg in segments {
        let text = seg.text.trim_matches(is_bash_space).to_string();
        if should_filter(&text, &opts.ignore_words) {
            out.filtered += 1;
            continue;
        }
        match &mut prev {
            Some(p) if text.to_lowercase() == p.text.to_lowercase() => {
                p.end = seg.end;
                out.collapsed += 1;
            }
            _ => {
                let done = prev.replace(Row {
                    speaker: speaker.to_string(),
                    start: seg.start,
                    end: seg.end,
                    text,
                });
                flush(done, &mut out);
            }
        }
    }
    flush(prev, &mut out);
    out
}

/// A track's rows as the `*.processed.tsv` file.
pub fn rows_to_tsv(rows: &[Row]) -> String {
    let mut s = String::from(TSV_HEADER);
    for r in rows {
        s.push_str(&format!("{}\t{}\t{}\t{}\n", r.speaker, r.start, r.end, r.text));
    }
    s
}

// -- aggregation ------------------------------------------------------------------------

/// `sort -k2,2n`'s view of a number: the leading integer, or 0.
fn numeric_key(field: &str) -> i128 {
    let t = field.trim_start_matches([' ', '\t']);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let n: String = digits.chars().take_while(char::is_ascii_digit).collect();
    let v: i128 = n.parse().unwrap_or(0);
    if neg { -v } else { v }
}

/// Order of two merged lines: by start time, then by the whole line as bytes (what GNU
/// `sort -k2,2n` falls back to, in the C locale).
fn merged_order(a: &str, b: &str) -> Ordering {
    let key = |l: &str| numeric_key(l.split('\t').nth(1).unwrap_or(""));
    key(a).cmp(&key(b)).then_with(|| a.as_bytes().cmp(b.as_bytes()))
}

/// The three transcripts and the counts the script logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembled {
    pub merged: String,
    pub consolidated: String,
    pub markdown: String,
    pub merged_lines: usize,
    pub consolidated_lines: usize,
    pub entries: usize,
    pub reduction_percent: usize,
}

impl Assembled {
    /// The lines the script logs while it assembles the transcripts, in its order.
    pub fn log_lines(&self, files: usize) -> Vec<String> {
        vec![
            format!("Aggregating {files} processed files..."),
            "Created merged_transcript.tsv".to_string(),
            "Consolidating consecutive same-speaker lines...".to_string(),
            format!(
                "Created consolidated_transcript.tsv ({} → {} lines, {}% reduction)",
                self.merged_lines, self.consolidated_lines, self.reduction_percent
            ),
            "Creating transcript.md...".to_string(),
            format!("Created transcript.md ({} entries)", self.entries),
        ]
    }
}

/// `HH:MM:SS` from milliseconds, floored to seconds; hours are not wrapped at 24.
pub fn format_time(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    format!("{:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// Merge the per-track files (given in file-name order, each with its header) into the
/// merged, consolidated and Markdown transcripts.
pub fn assemble<S: AsRef<str>>(processed: &[S]) -> Assembled {
    let mut lines: Vec<&str> = processed.iter().flat_map(|t| t.as_ref().lines().skip(1)).collect();
    lines.sort_by(|a, b| merged_order(a, b));

    let mut merged = String::from(TSV_HEADER);
    for l in &lines {
        merged.push_str(l);
        merged.push('\n');
    }

    // Consecutive rows by the same speaker become one.
    let mut consolidated_rows: Vec<Row> = Vec::new();
    let mut original = 0usize;
    for l in &lines {
        let mut f = l.splitn(4, '\t');
        let (speaker, start, end, text) = (
            f.next().unwrap_or(""),
            f.next().unwrap_or(""),
            f.next().unwrap_or(""),
            f.next().unwrap_or(""),
        );
        if speaker.is_empty() {
            continue;
        }
        original += 1;
        let (start, end) = (numeric_key(start) as i64, numeric_key(end) as i64);
        match consolidated_rows.last_mut() {
            Some(prev) if prev.speaker == speaker => {
                prev.end = end;
                prev.text.push(' ');
                prev.text.push_str(text);
            }
            _ => consolidated_rows.push(Row {
                speaker: speaker.to_string(),
                start,
                end,
                text: text.to_string(),
            }),
        }
    }
    let consolidated = rows_to_tsv(&consolidated_rows);
    let mut markdown = String::new();
    for r in &consolidated_rows {
        markdown.push_str(&format!(
            "**[{}] {}:**  \n{}\n\n",
            format_time(r.start),
            r.speaker,
            r.text
        ));
    }
    let reduction_percent = ((original - consolidated_rows.len()) * 100)
        .checked_div(original)
        .unwrap_or(0);
    Assembled {
        merged,
        consolidated,
        markdown,
        merged_lines: lines.len(),
        consolidated_lines: consolidated_rows.len(),
        entries: consolidated_rows.len(),
        reduction_percent,
    }
}

/// Rows of a `*.processed.tsv` file (the header skipped).
pub fn parse_rows(tsv: &str) -> Vec<Row> {
    tsv.lines()
        .skip(1)
        .filter_map(|l| {
            let mut f = l.splitn(4, '\t');
            let speaker = f.next()?;
            let start = f.next()?;
            let end = f.next()?;
            let text = f.next().unwrap_or("");
            Some(Row {
                speaker: speaker.to_string(),
                start: numeric_key(start) as i64,
                end: numeric_key(end) as i64,
                text: text.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: i64, end: i64, text: &str) -> RawSegment {
        RawSegment {
            start,
            end,
            text: text.into(),
        }
    }

    #[test]
    fn json_becomes_segments_with_one_leading_space_removed_and_tsv_escaping() {
        let json = r#"{"transcription":[
            {"offsets":{"from":0,"to":900},"text":" you"},
            {"offsets":{"from":1000,"to":2000},"text":"  two spaces"},
            {"offsets":{"from":2000,"to":3000},"text":" tab\there\nnew\\line\r"}]}"#;
        let s = segments_from_json(json).unwrap();
        assert_eq!(s[0], seg(0, 900, "you"));
        assert_eq!(s[1].text, " two spaces");
        assert_eq!(s[2].text, "tab\\there\\nnew\\\\line\\r");
        assert!(segments_from_json("{}").unwrap_err().contains("transcription"));
        assert!(
            segments_from_json(r#"{"transcription":[{"text":"x"}]}"#)
                .unwrap_err()
                .contains("offsets.from")
        );
        assert!(segments_from_json("nope").unwrap_err().contains("not valid JSON"));
    }

    #[test]
    fn filtering_collapsing_and_the_stock_rule_work_in_the_scripts_order() {
        let opts = PostOptions::default();
        let p = process_segments(
            "GM",
            &[
                seg(0, 900, "you"),
                seg(1000, 2000, "The lantern is lit."),
                seg(2000, 3000, "the LANTERN is lit."),
                seg(3000, 4000, "Um."),
                seg(4000, 5000, "The lantern is lit."),
                seg(6000, 36_000, "Thank you."),
                seg(36_000, 66_000, "Thank you."),
                seg(67_000, 68_000, "Okay."),
                seg(70_000, 71_500, "Thank you."),
                seg(72_000, 73_000, "Well, then."),
            ],
            &opts,
        );
        assert_eq!((p.filtered, p.collapsed, p.stock_dropped), (2, 3, 1));
        let texts: Vec<&str> = p.rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["The lantern is lit.", "Okay.", "Thank you.", "Well, then."]);
        assert_eq!(
            (p.rows[0].start, p.rows[0].end),
            (1000, 5000),
            "collapsing extends the end"
        );
        assert_eq!(
            (p.rows[2].start, p.rows[2].end),
            (70_000, 71_500),
            "the short one is kept"
        );
        assert_eq!(
            p.log_line("a.processed.tsv"),
            "Processed: a.processed.tsv — filtered: 2, collapsed: 3, stock phrases dropped: 1"
        );
        // A short one right after a long run is collapsed into it and goes with it: the
        // limitation noted in ERRATA.md.
        let glued = process_segments(
            "GM",
            &[seg(0, 30_000, "Thank you."), seg(31_000, 32_000, "Thank you.")],
            &opts,
        );
        assert!(glued.rows.is_empty() && glued.stock_dropped == 1);
        let none = process_segments("GM", &[seg(0, 1, "hello")], &opts);
        assert_eq!(none.log_line("b"), "Processed: b — filtered: 0, collapsed: 0");
    }

    #[test]
    fn empty_text_is_kept_like_the_script_keeps_it() {
        let p = process_segments("GM", &[seg(0, 10, "   "), seg(20, 30, "hi")], &PostOptions::default());
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].text, "");
    }

    #[test]
    fn merging_sorts_by_start_then_by_whole_line_and_consolidates_neighbours() {
        let a = rows_to_tsv(&[
            Row {
                speaker: "GM".into(),
                start: 5000,
                end: 6000,
                text: "one".into(),
            },
            Row {
                speaker: "GM".into(),
                start: 90_000_000,
                end: 90_001_000,
                text: "late".into(),
            },
        ]);
        let b = rows_to_tsv(&[
            Row {
                speaker: "Mara".into(),
                start: 5000,
                end: 5500,
                text: "tie".into(),
            },
            Row {
                speaker: "Mara".into(),
                start: 7000,
                end: 8000,
                text: "two".into(),
            },
            Row {
                speaker: "GM".into(),
                start: 9000,
                end: 9500,
                text: "three".into(),
            },
        ]);
        let out = assemble(&[a, b]);
        assert_eq!(out.merged_lines, 5);
        let order: Vec<String> = parse_rows(&out.merged).iter().map(|r| r.text.clone()).collect();
        assert_eq!(
            order,
            ["one", "tie", "two", "three", "late"],
            "the tie at 5000 is broken by the line itself"
        );
        assert_eq!(
            out.consolidated_lines, 3,
            "GM, Mara, Mara, GM, GM: the Mara rows join, the last GMs join"
        );
        assert_eq!(
            out.markdown,
            "**[00:00:05] GM:**  \none\n\n**[00:00:05] Mara:**  \ntie two\n\n**[00:00:09] GM:**  \nthree late\n\n"
        );
    }

    #[test]
    fn counts_and_the_reduction_percentage_use_integer_division() {
        let t = rows_to_tsv(&[
            Row {
                speaker: "A".into(),
                start: 0,
                end: 1,
                text: "x".into(),
            },
            Row {
                speaker: "A".into(),
                start: 2,
                end: 3,
                text: "y".into(),
            },
            Row {
                speaker: "A".into(),
                start: 4,
                end: 5,
                text: "z".into(),
            },
        ]);
        let out = assemble(&[t]);
        assert_eq!(
            (out.merged_lines, out.consolidated_lines, out.reduction_percent),
            (3, 1, 66)
        );
        let empty = assemble::<String>(&[]);
        assert_eq!(
            (empty.merged, empty.consolidated.as_str(), empty.markdown.as_str()),
            (TSV_HEADER.to_string(), TSV_HEADER, "")
        );
        assert_eq!(empty.reduction_percent, 0);
    }

    #[test]
    fn times_are_floored_and_hours_do_not_wrap() {
        assert_eq!(format_time(0), "00:00:00");
        assert_eq!(format_time(999), "00:00:00");
        assert_eq!(format_time(3_725_000), "01:02:05");
        assert_eq!(format_time(90_000_000), "25:00:00");
    }
}
