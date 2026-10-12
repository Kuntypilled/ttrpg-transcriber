//! The transcript chunker (PORTING.md sections 5.1 to 5.5): `transcript.md` to the
//! JSONL the Chroma helper ingests, byte for byte what `transcript_to_chunks.py`
//! wrote, because changing the chunk shape invalidates an existing index.
//!
//! Python's `re` is used there for two things: parsing the `**[hh:mm:ss] Speaker:**`
//! entries, which needs a look-ahead the `regex` crate does not have (so it is parsed
//! by hand, with the same results), and the alias substitutions, which are plain
//! case-insensitive whole-word replacements.

use indexmap::IndexMap;
use regex::{NoExpand, Regex, RegexBuilder};

use crate::campaign::{Campaign, SpeakerRow};

/// Target size of a chunk, in words, and how many utterances the next one repeats.
pub const TARGET_WORDS: usize = 280;
pub const OVERLAP: usize = 1;

/// Python's `str.isspace()`: Unicode White_Space plus the four ASCII separators.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// One entry of the transcript, as the chunker sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utterance {
    /// `hh:mm:ss` as written.
    pub ts: String,
    pub gm: bool,
    pub character: Option<String>,
    pub label: String,
    pub text: String,
}

/// `(hh, mm, ss, speaker label, body)` for each entry. The regex of the original is
/// `\*\*\[(\d{2}):(\d{2}):(\d{2})\]\s+([^:]+):\*\*\s*\n(.*?)(?=\n\*\*\[|\Z)` with DOTALL.
pub fn entries(raw: &str) -> Vec<(String, String, String, String, String)> {
    let b = raw.as_bytes();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(rel) = raw[pos..].find("**[") {
        let at = pos + rel;
        match header_at(raw, at) {
            Some((hms, label, body_start)) => {
                let body_end = raw[body_start..].find("\n**[").map_or(raw.len(), |i| body_start + i);
                out.push((
                    hms.0,
                    hms.1,
                    hms.2,
                    label.to_string(),
                    raw[body_start..body_end].to_string(),
                ));
                pos = body_end.max(at + 1);
            }
            None => pos = at + 1,
        }
        if pos >= b.len() {
            break;
        }
    }
    out
}

/// Non-blank lines that belong to no entry: text before the first speaker line, or a line
/// that starts like a header but is not one (a mistyped time, a missing colon). The original
/// dropped these without a word (PORTING.md section 10, item 6); the Index job reports the count.
pub fn uncovered_lines(raw: &str) -> usize {
    let mut covered_to = 0;
    let mut pos = 0;
    let mut count = 0;
    let lines = |text: &str| text.lines().filter(|l| !l.trim().is_empty()).count();
    while let Some(rel) = raw[pos..].find("**[") {
        let at = pos + rel;
        match header_at(raw, at) {
            Some((_, _, body_start)) => {
                count += lines(&raw[covered_to..at]);
                let body_end = raw[body_start..].find("\n**[").map_or(raw.len(), |i| body_start + i);
                covered_to = body_end;
                pos = body_end.max(at + 1);
            }
            None => pos = at + 1,
        }
        if pos >= raw.len() {
            break;
        }
    }
    count + lines(&raw[covered_to.min(raw.len())..])
}

type Hms = (String, String, String);

/// Try to read an entry header at `at` (which holds `**[`): the time, the label, and
/// where the body starts.
fn header_at(raw: &str, at: usize) -> Option<(Hms, &str, usize)> {
    let b = raw.as_bytes();
    let mut i = at + 3;
    let two = |i: usize| -> Option<&str> {
        let s = raw.get(i..i + 2)?;
        s.bytes().all(|c| c.is_ascii_digit()).then_some(s)
    };
    let h = two(i)?;
    i += 2;
    if b.get(i) != Some(&b':') {
        return None;
    }
    let m = two(i + 1)?;
    i += 3;
    if b.get(i) != Some(&b':') {
        return None;
    }
    let s = two(i + 1)?;
    i += 3;
    if b.get(i) != Some(&b']') {
        return None;
    }
    i += 1;
    // `\s+` then `[^:]+`: at least one space, then a label up to the first colon.
    let first_ws = raw[i..].chars().next().filter(|c| is_py_space(*c))?;
    let label_from = i + first_ws.len_utf8();
    let colon = label_from + raw[label_from..].find(':')?;
    if colon == label_from {
        return None;
    }
    let label = &raw[label_from..colon];
    // The original's `\s+` is greedy, so the label group does not start with spaces.
    let label = Some(label.trim_start_matches(is_py_space))
        .filter(|l| !l.is_empty())
        .unwrap_or(label);
    // `:**` then `\s*\n`: whitespace containing a newline; the body starts after its last one.
    if !raw[colon..].starts_with(":**") {
        return None;
    }
    let after = colon + 3;
    let run_len: usize = raw[after..]
        .chars()
        .take_while(|c| is_py_space(*c))
        .map(char::len_utf8)
        .sum();
    let run = &raw[after..after + run_len];
    let last_nl = run.rfind('\n')?;
    Some((
        (h.to_string(), m.to_string(), s.to_string()),
        label,
        after + last_nl + 1,
    ))
}

/// The substitutions of `Campaign.alias_subs`: `\b<variant>\b`, case-insensitive,
/// aliases only (never `review_only`), in file order.
pub fn alias_subs(campaign: &Campaign) -> Vec<(Regex, String)> {
    let mut out = Vec::new();
    for (canon, variants) in campaign.aliases() {
        for v in variants {
            let pattern = format!(r"\b{}\b", regex::escape(&v));
            if let Ok(re) = RegexBuilder::new(&pattern).case_insensitive(true).build() {
                out.push((re, canon.clone()));
            }
        }
    }
    out
}

fn clean(text: &str, subs: &[(Regex, String)]) -> String {
    let mut t = text.to_string();
    for (re, canon) in subs {
        t = re.replace_all(&t, NoExpand(canon)).into_owned();
    }
    t
}

/// `(role is GM, player, character)` for a speaker label, by the campaign's map.
fn resolve(label: &str, map: &[SpeakerRow]) -> (bool, Option<String>, Option<String>) {
    let low = label.to_lowercase();
    let gm = low.contains("gm") || low.contains("-gm-");
    for row in map {
        if low.contains(&row.key) {
            return (row.character.is_none() || gm, row.player.clone(), row.character.clone());
        }
    }
    (gm, None, None)
}

pub fn parse(raw: &str, subs: &[(Regex, String)], map: &[SpeakerRow]) -> Vec<Utterance> {
    entries(raw)
        .into_iter()
        .map(|(h, m, s, label, body)| {
            let (gm, _player, character) = resolve(py_strip(&label), map);
            let character = character.filter(|c| !c.is_empty());
            Utterance {
                ts: format!("{h}:{m}:{s}"),
                gm,
                label: character.clone().unwrap_or_else(|| "GM".to_string()),
                character,
                text: clean(py_strip(&body), subs),
            }
        })
        .filter(|u| !u.text.is_empty())
        .collect()
}

/// A window of utterances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub characters: Vec<String>,
    pub has_gm: bool,
    pub start: String,
    pub end: String,
    pub words: usize,
    pub n_utt: usize,
    pub text: String,
}

fn word_count(s: &str) -> usize {
    s.split(is_py_space).filter(|w| !w.is_empty()).count()
}

pub fn window(utts: &[Utterance], target_words: usize, overlap: usize) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < utts.len() {
        let (mut j, mut w) = (i, 0);
        while j < utts.len() && w < target_words {
            w += word_count(&utts[j].text);
            j += 1;
        }
        let block = &utts[i..j];
        let mut characters: Vec<String> = block.iter().filter_map(|u| u.character.clone()).collect();
        characters.sort();
        characters.dedup();
        chunks.push(Chunk {
            characters,
            has_gm: block.iter().any(|u| u.gm),
            start: block[0].ts.clone(),
            end: block[block.len() - 1].ts.clone(),
            words: w,
            n_utt: block.len(),
            text: block
                .iter()
                .map(|u| format!("[{}] {}: {}", u.ts, u.label, u.text))
                .collect::<Vec<_>>()
                .join("\n"),
        });
        if j >= utts.len() {
            break;
        }
        i = j.saturating_sub(overlap).max(i + 1);
    }
    chunks
}

// -- JSON the way Python's json.dumps writes it (ensure_ascii, ", " and ": ") ---------------

pub fn dumps_ascii_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

/// What a chunking run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRun {
    /// One JSON object per line, each ending in a newline.
    pub jsonl: String,
    pub chunks: usize,
    pub utterances: usize,
    pub alias_rules: usize,
    /// Lines of the transcript that belong to no entry and so were not indexed.
    pub uncovered_lines: usize,
}

impl ChunkRun {
    /// The summary line the script printed to stderr.
    pub fn summary(&self, session: i64) -> String {
        format!(
            "{} chunks from {} utterances (session {session}); {} alias rules applied",
            self.chunks, self.utterances, self.alias_rules
        )
    }
}

/// Chunk a transcript for a campaign. `date` is `YYYY-MM-DD`.
pub fn chunk_transcript(campaign: &Campaign, raw: &str, session: i64, date: &str) -> ChunkRun {
    // Python reads the file in text mode: \r\n and lone \r become \n.
    let raw = raw.replace("\r\n", "\n").replace('\r', "\n");
    let subs = alias_subs(campaign);
    let utts = parse(&raw, &subs, &campaign.speaker_map());
    let chunks = window(&utts, TARGET_WORDS, OVERLAP);
    let pcs = campaign.pcs();
    let mut jsonl = String::new();
    for (k, c) in chunks.iter().enumerate() {
        let mut meta: IndexMap<String, String> = IndexMap::new();
        meta.insert("session".into(), session.to_string());
        meta.insert("date".into(), dumps_ascii_str(date));
        meta.insert("start".into(), dumps_ascii_str(&c.start));
        meta.insert("end".into(), dumps_ascii_str(&c.end));
        meta.insert("has_gm".into(), c.has_gm.to_string());
        meta.insert("n_utt".into(), c.n_utt.to_string());
        meta.insert("characters".into(), dumps_ascii_str(&c.characters.join(",")));
        for name in &pcs {
            meta.insert(format!("has_{name}"), c.characters.contains(name).to_string());
        }
        let meta = meta
            .iter()
            .map(|(k, v)| format!("{}: {v}", dumps_ascii_str(k)))
            .collect::<Vec<_>>()
            .join(", ");
        jsonl.push_str(&format!(
            "{{\"id\": {}, \"document\": {}, \"metadata\": {{{meta}}}}}\n",
            dumps_ascii_str(&format!("s{session:02}_c{k:03}")),
            dumps_ascii_str(&c.text),
        ));
    }
    ChunkRun {
        jsonl,
        chunks: chunks.len(),
        utterances: utts.len(),
        alias_rules: subs.len(),
        uncovered_lines: uncovered_lines(&raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(raw: &str) -> Vec<(String, String)> {
        entries(raw)
            .into_iter()
            .map(|(h, m, s, l, b)| (format!("{h}:{m}:{s} {l}"), b))
            .collect()
    }

    #[test]
    fn entries_are_read_the_way_the_regex_reads_them() {
        let raw = "**[00:00:04] GM:**  \nFirst line.\nSecond line.\n\n**[00:01:10] Mara_Quill:**  \nHello.\n\n";
        assert_eq!(
            e(raw),
            [
                ("00:00:04 GM".to_string(), "First line.\nSecond line.\n".to_string()),
                ("00:01:10 Mara_Quill".to_string(), "Hello.\n\n".to_string()),
            ]
        );
    }

    #[test]
    fn lines_outside_any_entry_are_counted() {
        let good = "**[00:00:04] GM:**  \nFirst.\n\n**[00:01:10] Mara_Quill:**  \nHello.\n\n";
        assert_eq!(uncovered_lines(good), 0, "a clean transcript loses nothing");
        assert_eq!(uncovered_lines(""), 0);
        // Notes above the first speaker line.
        assert_eq!(
            uncovered_lines(&format!("# Session 3\n\nrecorded on a Friday\n\n{good}")),
            2
        );
        // A header with a typo ends the previous entry; it and its text are skipped.
        let broken = "**[00:00:04] GM:**  \nFirst.\n\n**[0:01:10] Mara_Quill:**  \nLost.\nAlso lost.\n\n**[00:02:00] Tomas_Ink:**  \nKept.\n";
        assert_eq!(uncovered_lines(broken), 3);
        assert_eq!(entries(broken).len(), 2);
        // Nothing but prose.
        assert_eq!(uncovered_lines("just notes\nno speakers\n"), 2);
    }

    #[test]
    fn the_odd_shapes_the_regex_accepts_or_refuses() {
        // No newline after the header: not an entry.
        assert!(e("**[00:00:01] GM:**  text on the same line").is_empty());
        // Blank lines after the header: the body starts after the last newline of the run.
        assert_eq!(e("**[00:00:01] GM:**\n\n\n  indented\n")[0].1, "  indented\n");
        // Two headers with no blank line between: the second is swallowed into the first's body.
        let v = e("**[00:00:01] A:**\n**[00:00:02] B:**\ntext\n\n**[00:00:03] C:**\nlast");
        assert_eq!(v.len(), 2);
        assert!(v[0].1.starts_with("**[00:00:02] B:**\ntext"));
        // A line that only looks like a header ends the previous body and is itself skipped.
        let v = e("**[00:00:01] A:**\nbody\n**[note] aside\nmore\n\n**[00:00:09] B:**\nend");
        assert_eq!(
            v.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
            ["00:00:01 A", "00:00:09 B"]
        );
        assert_eq!(v[0].1, "body");
        // A label may contain spaces; it runs to the first colon.
        assert_eq!(e("**[00:00:01]   Two Words:**\nx")[0].0, "00:00:01 Two Words");
    }

    #[test]
    fn json_text_is_written_like_pythons_dumps() {
        assert_eq!(dumps_ascii_str("a\"b\\c\n\t"), "\"a\\\"b\\\\c\\n\\t\"");
        assert_eq!(
            dumps_ascii_str("Zoë 🎲 \u{7f}\u{1}"),
            "\"Zo\\u00eb \\ud83c\\udfb2 \\u007f\\u0001\""
        );
    }

    fn utt(ts: &str, who: Option<&str>, text: &str) -> Utterance {
        Utterance {
            ts: ts.into(),
            gm: who.is_none(),
            character: who.map(str::to_string),
            label: who.unwrap_or("GM").into(),
            text: text.into(),
        }
    }

    #[test]
    fn windows_overlap_by_one_utterance_and_always_advance() {
        let words = |n: usize| vec!["w"; n].join(" ");
        let u: Vec<Utterance> = (0..5)
            .map(|i| utt(&format!("00:00:0{i}"), Some("Mara"), &words(150)))
            .collect();
        let w = window(&u, 280, 1);
        // 150+150 >= 280 -> utterances 0-1; next starts at 1 (overlap): 1-2; then 2-3; then 3-4.
        assert_eq!(
            w.iter()
                .map(|c| (c.start.as_str(), c.end.as_str(), c.n_utt))
                .collect::<Vec<_>>(),
            [
                ("00:00:00", "00:00:01", 2),
                ("00:00:01", "00:00:02", 2),
                ("00:00:02", "00:00:03", 2),
                ("00:00:03", "00:00:04", 2),
            ]
        );
        // One huge utterance: one chunk, and the loop ends.
        assert_eq!(window(&[utt("00:00:00", None, &words(900))], 280, 1).len(), 1);
        assert!(window(&[], 280, 1).is_empty());
    }
}
