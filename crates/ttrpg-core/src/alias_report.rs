//! The alias-candidate report: a read-only scan of a
//! transcript for likely manglings of canon names and for frequent capitalised words
//! that are neither canon nor English. Ported from `transcript_to_chunks.py --report`;
//! the output is the same text byte for byte (`tests/alias_report.rs`), which needs
//! the exact similarity measure in [`crate::difflib`] and the same ordering rules.

use std::collections::HashSet;
use std::path::Path;

use indexmap::IndexMap;

use crate::campaign::Campaign;
use crate::chunker::entries;
use crate::difflib::SequenceMatcher;

/// Where the original looks for a word list, in order; the first that exists is used.
pub const WORDLISTS: [&str; 3] = [
    "/usr/share/dict/words",
    "/usr/share/dict/american-english",
    "/usr/share/dict/british-english",
];

pub const DEFAULT_THRESHOLD: f64 = 0.70;
pub const DEFAULT_MIN_FREQ: usize = 2;

/// Lower-cased dictionary words. Empty when no word list is installed: everything then
/// counts as not English, so every near-miss lands in the "safe" table.
pub fn load_dictionary(paths: &[&str]) -> HashSet<String> {
    for path in paths {
        // Unreadable: the original moves on to nothing (its `continue` skips the break).
        if Path::new(path).is_file()
            && let Ok(bytes) = std::fs::read(path)
        {
            return String::from_utf8_lossy(&bytes)
                .lines()
                .map(|w| w.trim().to_lowercase())
                .filter(|w| !w.is_empty())
                .collect();
        }
    }
    HashSet::new()
}

fn is_py_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// `[A-Za-z][A-Za-z'’\-]*`, as `findall` reads it.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if cur.is_empty() {
            if c.is_ascii_alphabetic() {
                cur.push(c);
            }
        } else if c.is_ascii_alphabetic() || matches!(c, '\'' | '\u{2019}' | '-') {
            cur.push(c);
        } else {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Drop a trailing possessive (`'s` or `’s`).
fn base_form(gram: &str) -> &str {
    gram.strip_suffix("'s")
        .or_else(|| gram.strip_suffix("\u{2019}s"))
        .unwrap_or(gram)
}

/// A near-miss: what was heard, how often, how close.
#[derive(Debug, Clone, PartialEq)]
pub struct Near {
    pub gram: String,
    pub count: usize,
    pub ratio: f64,
}

pub struct Candidates {
    /// Canon term -> variants, best first.
    pub near: IndexMap<String, Vec<Near>>,
    /// Capitalised unknown tokens and their counts, most frequent first.
    pub unknown_caps: Vec<(String, usize)>,
    pub n_words: usize,
}

fn python_split_len(s: &str) -> usize {
    s.split(is_py_space).filter(|w| !w.is_empty()).count()
}

/// Scan `raw` (the transcript text). Newlines are expected to be `\n` already.
pub fn find_candidates(
    campaign: &Campaign,
    raw: &str,
    threshold: f64,
    min_freq: usize,
    dictionary: &HashSet<String>,
) -> Candidates {
    let parsed = entries(raw);
    // `"\n".join(bodies) or raw`: an empty join falls back to the whole text too.
    let joined = parsed.iter().map(|e| e.4.as_str()).collect::<Vec<_>>().join("\n");
    let body = if joined.is_empty() { raw.to_string() } else { joined };
    let words = tokens(&body);

    // Canon terms: names, characters, and the keys of aliases and review_only.
    let aliases = campaign.aliases();
    let review_only = campaign.review_only();
    let mut canon_terms: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let sources = campaign
        .names()
        .into_iter()
        .chain(campaign.pcs())
        .chain(aliases.keys().cloned())
        .chain(review_only.keys().cloned());
    for term in sources {
        let t = py_strip(&term.replace('_', " ")).to_string();
        if t.chars().count() >= 3 && seen.insert(t.to_lowercase()) {
            canon_terms.push(t);
        }
    }

    // Everything already accounted for.
    let mut known: HashSet<String> = seen.clone();
    for table in [&aliases, &review_only] {
        for variants in table.values() {
            known.extend(variants.iter().map(|v| v.to_lowercase()));
        }
    }
    if let Some(serde_json::Value::Object(nicks)) = campaign.data().get("pc_nicknames") {
        for variants in nicks.values() {
            if let serde_json::Value::Array(list) = variants {
                known.extend(list.iter().filter_map(|v| v.as_str()).map(str::to_lowercase));
            }
        }
    }
    known.extend(parsed.iter().map(|e| py_strip(&e.3).to_lowercase().replace('_', " ")));

    // n-gram pools, one per canon term length.
    let max_n = canon_terms
        .iter()
        .map(|t| python_split_len(t))
        .max()
        .unwrap_or(1)
        .min(3);
    let mut pools: Vec<IndexMap<String, usize>> = vec![IndexMap::new(); max_n + 1];
    for (n, pool) in pools.iter_mut().enumerate().skip(1) {
        for i in 0..words.len().saturating_sub(n - 1) {
            if i + n > words.len() {
                break;
            }
            let joined = words[i..i + n].join(" ");
            let gram = base_form(&joined);
            if gram.chars().count() >= 4 {
                *pool.entry(gram.to_string()).or_insert(0) += 1;
            }
        }
    }

    // A. near-misses of known canon.
    let mut near: IndexMap<String, IndexMap<String, (usize, f64)>> = IndexMap::new();
    for term in &canon_terms {
        let n = python_split_len(term).min(max_n);
        let tl = term.to_lowercase();
        for (gram, &count) in &pools[n] {
            let gl = gram.to_lowercase();
            if gl == tl || known.contains(&gl) {
                continue;
            }
            let sm = SequenceMatcher::new(&tl, &gl);
            if sm.real_quick_ratio() < threshold || sm.quick_ratio() < threshold {
                continue;
            }
            let ratio = sm.ratio();
            if ratio >= threshold {
                let entry = near.entry(term.clone()).or_default();
                match entry.get(gram) {
                    Some(prev) if count <= prev.0 => {}
                    _ => {
                        entry.insert(gram.clone(), (count, ratio));
                    }
                }
            }
        }
    }

    // Keep each variant only against its single best canon term.
    let mut best: IndexMap<String, (String, usize, f64)> = IndexMap::new();
    for (term, variants) in &near {
        for (gram, &(count, ratio)) in variants {
            match best.get(gram) {
                Some(b) if ratio <= b.2 => {}
                _ => {
                    best.insert(gram.clone(), (term.clone(), count, ratio));
                }
            }
        }
    }
    let mut grouped: IndexMap<String, Vec<Near>> = IndexMap::new();
    for (gram, (term, count, ratio)) in &best {
        grouped.entry(term.clone()).or_default().push(Near {
            gram: gram.clone(),
            count: *count,
            ratio: *ratio,
        });
    }
    for list in grouped.values_mut() {
        // Stable, like Python's sort on (-ratio, -count).
        list.sort_by(|a, b| {
            b.ratio
                .partial_cmp(&a.ratio)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.count.cmp(&a.count))
        });
    }

    // B. capitalised tokens that are neither canon nor dictionary words.
    let best_lower: HashSet<String> = best.keys().map(|g| g.to_lowercase()).collect();
    let mut caps: IndexMap<String, usize> = IndexMap::new();
    for w in &words {
        let w = base_form(w);
        if !w.chars().next().is_some_and(char::is_uppercase) || w.chars().count() < 4 {
            continue;
        }
        let wl = w.to_lowercase();
        if dictionary.contains(&wl) || known.contains(&wl) || best_lower.contains(&wl) {
            continue;
        }
        *caps.entry(w.to_string()).or_insert(0) += 1;
    }
    let mut unknown_caps: Vec<(String, usize)> = caps.into_iter().collect();
    unknown_caps.sort_by_key(|b| std::cmp::Reverse(b.1)); // stable: ties keep first-seen order
    unknown_caps.retain(|(_, c)| *c >= min_freq);

    Candidates {
        near: grouped,
        unknown_caps,
        n_words: words.len(),
    }
}

/// Every word is a dictionary word. With no dictionary nothing is English.
fn is_english(gram: &str, dictionary: &HashSet<String>) -> bool {
    !dictionary.is_empty()
        && gram.split(is_py_space).filter(|w| !w.is_empty()).all(|w| {
            let w = w.to_lowercase();
            dictionary.contains(w.trim_matches(|c| matches!(c, '\'' | '\u{2019}' | '-')))
        })
}

/// Python's `repr` of a float: shortest form that reads back, with `.0` on whole numbers.
fn py_float(f: f64) -> String {
    format!("{f:?}")
}

/// `json.dumps(table, indent=2, ensure_ascii=False)` for a map of string lists.
fn dumps_table(table: &[(String, Vec<String>)]) -> String {
    if table.is_empty() {
        return "{}".to_string();
    }
    let esc = |s: &str| {
        let mut out = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    };
    let mut s = String::from("{\n");
    for (i, (k, v)) in table.iter().enumerate() {
        s.push_str(&format!("  {}: [\n", esc(k)));
        s.push_str(
            &v.iter()
                .map(|x| format!("    {}", esc(x)))
                .collect::<Vec<_>>()
                .join(",\n"),
        );
        s.push_str("\n  ]");
        s.push_str(if i + 1 < table.len() { ",\n" } else { "\n" });
    }
    s.push('}');
    s
}

/// The Markdown report.
pub fn render_report(
    campaign: &Campaign,
    session: i64,
    transcript_path: &str,
    found: &Candidates,
    threshold: f64,
    min_freq: usize,
    dictionary: &HashSet<String>,
) -> String {
    let mut l: Vec<String> = Vec::new();
    let mut a = |s: &str| l.push(s.to_string());
    a(&format!("# Alias candidates: {} session {session}", campaign.label()));
    a("");
    a(&format!("- transcript: `{transcript_path}`"));
    a(&format!("- words scanned: {}", found.n_words));
    a(&format!(
        "- similarity threshold: {}   minimum frequency for section B: {min_freq}",
        py_float(threshold)
    ));
    let count = |m: &IndexMap<String, Vec<String>>| m.values().map(Vec::len).sum::<usize>();
    a(&format!(
        "- existing rules: {} alias, {} review_only",
        count(&campaign.aliases()),
        count(&campaign.review_only())
    ));
    a("");
    a("Nothing here has been applied. This is a report. Copy what you confirm into");
    a("`canon_aliases.json` by hand.");
    a("");
    a("**A mangling that is also a real English word belongs in `review_only`, never in**");
    a("**`aliases`.** Auto-replacing \"complete\" or \"leave\" would quietly wreck a transcript.");
    a("");

    // Split by whether every word is English, terms in sorted order.
    let mut terms: Vec<&String> = found.near.keys().collect();
    terms.sort();
    let mut safe: IndexMap<String, Vec<String>> = IndexMap::new();
    let mut review: IndexMap<String, Vec<String>> = IndexMap::new();
    let mut safe_rows: Vec<(&str, &Near)> = Vec::new();
    let mut review_rows: Vec<(&str, &Near)> = Vec::new();
    for term in terms {
        for n in &found.near[term] {
            if is_english(&n.gram, dictionary) {
                review.entry(term.clone()).or_default().push(n.gram.clone());
                review_rows.push((term, n));
            } else {
                safe.entry(term.clone()).or_default().push(n.gram.clone());
                safe_rows.push((term, n));
            }
        }
    }

    let table = |l: &mut Vec<String>, rows: &[(&str, &Near)]| {
        if rows.is_empty() {
            l.push("None above threshold.".to_string());
            return;
        }
        l.push("| canon | heard as | times | similarity |".to_string());
        l.push("|---|---|---|---|".to_string());
        let mut rows: Vec<&(&str, &Near)> = rows.iter().collect();
        rows.sort_by(|x, y| {
            y.1.ratio
                .partial_cmp(&x.1.ratio)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(y.1.count.cmp(&x.1.count))
        });
        for (term, n) in rows {
            l.push(format!("| {term} | {} | {} | {:.2} |", n.gram, n.count, n.ratio));
        }
    };

    l.push("## A1. Near-misses that are NOT English words".into());
    l.push(String::new());
    l.push("Candidates for `aliases`. Safe to auto-replace, because nothing else in the".into());
    l.push("language spells them that way.".into());
    l.push(String::new());
    table(&mut l, &safe_rows);
    l.push(String::new());

    l.push("## A2. Near-misses that ARE English words".into());
    l.push(String::new());
    l.push("Candidates for `review_only` only. Expect noise here: short names sit close to".into());
    l.push("common words, so `Wern`/`were` and `Dunwell`/`well` will always show up. That is".into());
    l.push("harmless, because review_only is never applied automatically. Raise --threshold".into());
    l.push("to quiet it down.".into());
    l.push(String::new());
    table(&mut l, &review_rows);
    l.push(String::new());

    l.push("## B. Unrecognised proper nouns".into());
    l.push(String::new());
    l.push("Capitalised, not in the dictionary, not already canon. New names, or manglings".into());
    l.push("too far from the canon spelling for section A to see.".into());
    l.push(String::new());
    if found.unknown_caps.is_empty() {
        l.push("None above the frequency floor.".into());
    } else {
        l.push("| token | times |".into());
        l.push("|---|---|".into());
        for (w, c) in found.unknown_caps.iter().take(60) {
            l.push(format!("| {w} | {c} |"));
        }
        if found.unknown_caps.len() > 60 {
            l.push(String::new());
            l.push(format!("({} more below the cut.)", found.unknown_caps.len() - 60));
        }
    }
    l.push(String::new());

    let as_sorted = |m: &IndexMap<String, Vec<String>>| -> Vec<(String, Vec<String>)> {
        let mut keys: Vec<&String> = m.keys().collect();
        keys.sort();
        keys.into_iter()
            .map(|k| {
                let mut v: Vec<String> = m[k].clone();
                v.sort();
                v.dedup();
                (k.clone(), v)
            })
            .collect()
    };
    l.push("## Ready to paste".into());
    l.push(String::new());
    l.push("Merge these into the existing objects. Do not replace them.".into());
    l.push(String::new());
    l.push("`aliases` (safe, auto-replaced at the chunker):".into());
    l.push(String::new());
    l.push("```json".into());
    l.push(dumps_table(&as_sorted(&safe)));
    l.push("```".into());
    l.push(String::new());
    l.push("`review_only` (NEVER auto-replaced, surfaced in the recap pass):".into());
    l.push(String::new());
    l.push("```json".into());
    l.push(dumps_table(&as_sorted(&review)));
    l.push("```".into());
    l.push(String::new());
    l.join("\n") + "\n"
}

/// Scan and render in one step. `raw` is the transcript text.
pub fn alias_report(
    campaign: &Campaign,
    session: i64,
    transcript_path: &str,
    raw: &str,
    threshold: f64,
    min_freq: usize,
    dictionary: &HashSet<String>,
) -> String {
    // Python reads in text mode: \r\n and lone \r become \n.
    let raw = raw.replace("\r\n", "\n").replace('\r', "\n");
    let found = find_candidates(campaign, &raw, threshold, min_freq, dictionary);
    render_report(
        campaign,
        session,
        transcript_path,
        &found,
        threshold,
        min_freq,
        dictionary,
    )
}
