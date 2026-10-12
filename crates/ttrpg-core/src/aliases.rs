//! Surgical edits of `canon_aliases.json`.
//!
//! The file is hand-maintained and its key order, column alignment and `_comment`
//! fields are documentation, so a serde round trip (which would reformat the whole
//! file) is not an option. New entries are spliced in as text, and the result is
//! checked to parse and to leave every pre-existing key as it was. Ported from
//! `append_object_entries` in `tests/oracle/campaign.py`, which the parity tests run on
//! the same files; where this differs it is on purpose and listed in `docs/differences.md`.

use serde_json::{Map, Value};

use crate::campaign::Campaign;
use crate::error::CampaignError;
use crate::pyish;

/// The text after a splice and the keys that were actually added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub text: String,
    pub added: Vec<String>,
}

fn err(msg: impl Into<String>) -> CampaignError {
    CampaignError::new(msg)
}

fn repr_str(s: &str) -> String {
    pyish::repr(&Value::String(s.to_string()))
}

// -- Python-style JSON text -------------------------------------------------------------------

/// `json.dumps(v, ensure_ascii=False)`: `", "` and `": "` separators, one line.
fn py_dumps(v: &Value) -> String {
    match v {
        Value::Array(a) => format!("[{}]", a.iter().map(py_dumps).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", py_dumps(&Value::String(k.clone())), py_dumps(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// One value, one line like `json.dumps`, or an object spread over lines in the
/// file's own style when `step` (the extra indent of a nested key) is known.
fn render_value(v: &Value, indent: &str, step: Option<&str>, nl: &str) -> String {
    match (v, step) {
        (Value::Object(m), Some(step)) if !m.is_empty() => {
            let inner = format!("{indent}{step}");
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{inner}{}: {}",
                        py_dumps(&Value::String(k.clone())),
                        render_value(v, &inner, Some(step), nl)
                    )
                })
                .collect();
            format!("{{{nl}{}{nl}{indent}}}", parts.join(&format!(",{nl}")))
        }
        _ => py_dumps(v),
    }
}

// -- finding the object -----------------------------------------------------------------------

/// Index of the closing quote of the string that opens at `start`.
fn string_end(b: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// Index of the `}` that matches the `{` at `open`, skipping strings.
fn matching_brace(b: &[u8], open: usize) -> Option<usize> {
    let (mut depth, mut i) = (0usize, open);
    while i < b.len() {
        match b[i] {
            b'"' => i = string_end(b, i)?,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The keys of the object that opens at `open`, with the index of each key's opening
/// quote and of its value: only that object's own keys, not those of nested objects.
fn own_keys(text: &str, open: usize) -> Vec<(String, usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let (mut depth, mut i) = (0usize, open);
    while i < b.len() {
        match b[i] {
            b'"' => {
                let Some(end) = string_end(b, i) else { break };
                if depth == 1 {
                    let colon = skip_ws(b, end + 1);
                    if b.get(colon) == Some(&b':')
                        && let Ok(name) = serde_json::from_str::<String>(&text[i..=end])
                    {
                        out.push((name, i, skip_ws(b, colon + 1)));
                    }
                }
                i = end;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// `(index of '{', index of the matching '}')` for the top-level `key`.
///
/// The original matched the first `"key": {` anywhere in the text, which can be a
/// nested object of the same name. This finds the top-level one, and refuses a file
/// that has the key twice.
fn object_span(text: &str, key: &str) -> Result<(usize, usize), CampaignError> {
    let b = text.as_bytes();
    let start = skip_ws(b, 0);
    if b.get(start) != Some(&b'{') {
        return Err(err(format!("no \"{key}\" object found to append to")));
    }
    let mut hits = own_keys(text, start).into_iter().filter(|(k, _, _)| k == key);
    let Some((_, _, value)) = hits.next() else {
        return Err(err(format!("no \"{key}\" object found to append to")));
    };
    if hits.next().is_some() {
        return Err(err(format!(
            "\"{key}\" appears twice at the top level; fix the file by hand first"
        )));
    }
    if b.get(value) != Some(&b'{') {
        return Err(err(format!("no \"{key}\" object found to append to")));
    }
    let close = matching_brace(b, value).ok_or_else(|| err(format!("\"{key}\" object is not closed")))?;
    Ok((value, close))
}

// -- following the file's own style -----------------------------------------------------------

/// How the entries already in the object are laid out.
#[derive(Debug, PartialEq, Eq)]
struct Style {
    /// Whitespace before each key.
    indent: String,
    /// Extra indent of the keys inside an entry, when entries span several lines.
    step: Option<String>,
}

impl Default for Style {
    /// The original's: four spaces, entries on one line.
    fn default() -> Self {
        Self {
            indent: "    ".into(),
            step: None,
        }
    }
}

fn detect_style(text: &str, open: usize) -> Style {
    let b = text.as_bytes();
    let keys = own_keys(text, open);
    let Some((_, key_at, value_at)) = keys.last().cloned() else {
        return Style::default();
    };
    let line_start = text[..key_at].rfind('\n').map_or(0, |n| n + 1);
    let lead = &text[line_start..key_at];
    // The key must start its line, or there is no indent to follow.
    if line_start == 0 || !lead.chars().all(|c| c == ' ' || c == '\t') {
        return Style::default();
    }
    let indent = lead.to_string();
    let mut step = None;
    if b.get(value_at) == Some(&b'{')
        && let Some(close) = matching_brace(b, value_at)
        && let Some(at) = text[value_at..close].find('\n')
    {
        let rest = &text[value_at..close][at + 1..];
        let nested: String = rest.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        if nested.len() > indent.len() && nested.starts_with(&indent) && rest[nested.len()..].starts_with('"') {
            step = Some(nested[indent.len()..].to_string());
        }
    }
    Style { indent, step }
}

// -- the splice -------------------------------------------------------------------------------

/// Splice `entries` into the top-level object `key` of `text`. Entries whose key is
/// already there are left alone. Everything outside the object, and every byte of it
/// before the new entries, stays exactly as it was. `path` is only for messages.
pub fn splice_object_entries(
    text: &str,
    path: &str,
    key: &str,
    entries: &Map<String, Value>,
) -> Result<Splice, CampaignError> {
    let original: Value = serde_json::from_str(text).map_err(|e| err(format!("{path} is not valid JSON: {e}")))?;
    let Some(root) = original.as_object() else {
        return Err(err(format!("{path} must contain a JSON object")));
    };
    let Some(existing) = root.get(key).and_then(Value::as_object) else {
        return Err(err(format!("\"{key}\" is not an object in {path}")));
    };
    let todo: Vec<(&String, &Value)> = entries.iter().filter(|(k, _)| !existing.contains_key(*k)).collect();
    if todo.is_empty() {
        return Ok(Splice {
            text: text.to_string(),
            added: Vec::new(),
        });
    }

    let (open, close) = object_span(text, key)?;
    // New lines use the file's own line ending.
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let body = &text[open + 1..close];
    let trimmed = body.trim_end();
    let tail = if trimmed.len() == body.len() {
        nl
    } else {
        &body[trimmed.len()..]
    };
    let style = detect_style(text, open);
    let mut content = trimmed.to_string();
    if !content.is_empty() && !content.ends_with(',') {
        content.push(',');
    }
    for (k, v) in &todo {
        content.push_str(&format!(
            "{nl}{}{}: {},",
            style.indent,
            py_dumps(&Value::String((*k).clone())),
            render_value(v, &style.indent, style.step.as_deref(), nl)
        ));
    }
    let content = content.trim_end_matches(',');
    let new_text = format!("{}{}{}{}", &text[..=open], content, tail, &text[close..]);

    // Verify before anything is written.
    let updated: Value = serde_json::from_str(&new_text)
        .map_err(|e| err(format!("append would have broken the JSON ({e}); aborted")))?;
    let updated_root = updated
        .as_object()
        .ok_or_else(|| err("append changed the top level; aborted"))?;
    for (k, v) in root {
        if k != key && updated_root.get(k) != Some(v) {
            return Err(err(format!("append would have altered {}; aborted", repr_str(k))));
        }
    }
    let updated_obj = updated_root.get(key).and_then(Value::as_object);
    for (k, v) in existing {
        if updated_obj.and_then(|o| o.get(k)) != Some(v) {
            return Err(err(format!("append would have altered {key}.{}; aborted", repr_str(k))));
        }
    }
    for (k, v) in &todo {
        if updated_obj.and_then(|o| o.get(*k)) != Some(*v) {
            return Err(err(format!(
                "append did not produce {key}.{} as given; aborted",
                repr_str(k)
            )));
        }
    }
    Ok(Splice {
        text: new_text,
        added: todo.iter().map(|(k, _)| (*k).clone()).collect(),
    })
}

/// [`splice_object_entries`] on a file: read, splice, write. Nothing is written
/// when there is nothing to add or when verification fails. Returns the keys added.
pub fn append_object_entries(
    path: &str,
    key: &str,
    entries: &Map<String, Value>,
) -> Result<Vec<String>, CampaignError> {
    let text = std::fs::read_to_string(path).map_err(|e| err(format!("cannot read {path}: {e}")))?;
    let splice = splice_object_entries(&text, path, key, entries)?;
    if !splice.added.is_empty() {
        // Same directory, so the rename stays on one file system and a crash never
        // leaves half a file.
        let tmp = format!("{path}.ttrpg-tui.tmp");
        std::fs::write(&tmp, &splice.text).map_err(|e| err(format!("cannot write {tmp}: {e}")))?;
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err(format!("cannot replace {path}: {e}")));
        }
    }
    Ok(splice.added)
}

// -- unknown speakers -------------------------------------------------------------------------

/// One row of the resolve-speakers form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeakerInput {
    /// The `discord_names` key: the Craig file name without number and extension.
    pub stem: String,
    pub player: String,
    pub character: String,
}

/// The `discord_names` entries for the rows the person filled in. A row with neither
/// player nor character is skipped; a blank character means the GM. The character is
/// normalised the way the rest of the system does (`Mara Quill` -> `Mara_Quill`).
/// A stem given twice keeps the last row, as the original dialog did.
pub fn discord_entries(rows: &[SpeakerInput]) -> Map<String, Value> {
    let mut out = Map::new();
    for row in rows {
        let (player, character) = (row.player.trim(), row.character.trim());
        if player.is_empty() && character.is_empty() {
            continue;
        }
        let mut entry = Map::new();
        entry.insert(
            "player".into(),
            if player.is_empty() {
                Value::Null
            } else {
                Value::String(player.to_string())
            },
        );
        entry.insert(
            "character".into(),
            if character.is_empty() {
                Value::Null
            } else {
                Value::String(pyish::normalize_character(character))
            },
        );
        entry.insert(
            "role".into(),
            Value::String(if character.is_empty() { "GM" } else { "Player" }.into()),
        );
        out.insert(row.stem.clone(), Value::Object(entry));
    }
    out
}

/// What saving the form did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SaveReport {
    pub added: Vec<String>,
    /// Stems that were already in `discord_names`, so were left alone. They stay
    /// unresolved when the existing entry has neither a character nor a GM role.
    pub left_alone: Vec<String>,
}

/// Write the filled-in rows to the campaign's `canon_aliases.json`.
pub fn save_speakers(campaign: &Campaign, rows: &[SpeakerInput]) -> Result<SaveReport, CampaignError> {
    let entries = discord_entries(rows);
    if entries.is_empty() {
        return Ok(SaveReport::default());
    }
    let added = append_object_entries(&campaign.aliases_path(), "discord_names", &entries)?;
    let left_alone = entries.keys().filter(|k| !added.contains(k)).cloned().collect();
    Ok(SaveReport { added, left_alone })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entries(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn splice(text: &str, key: &str, new: Value) -> Splice {
        splice_object_entries(text, "t.json", key, &entries(new)).unwrap()
    }

    const PLAIN: &str = "{\n    \"name\": \"x\",\n    \"discord_names\": {\n        \"riverone\": {\"player\": \"Pat\", \"character\": \"Mara_Quill\", \"role\": \"Player\"}\n    },\n    \"after\": [1, 2]\n}\n";

    #[test]
    fn an_added_entry_is_the_only_change() {
        let s = splice(
            PLAIN,
            "discord_names",
            json!({"newuser": {"player": "Lee", "character": null, "role": "GM"}}),
        );
        assert_eq!(s.added, ["newuser"]);
        let want = PLAIN.replace(
            "\"Player\"}\n    },",
            "\"Player\"},\n        \"newuser\": {\"player\": \"Lee\", \"character\": null, \"role\": \"GM\"}\n    },",
        );
        assert_eq!(s.text, want);
        // Line by line: the old lines are all still there, in order, and one line is new.
        let (old, new): (Vec<&str>, Vec<&str>) = (PLAIN.lines().collect(), s.text.lines().collect());
        assert_eq!(new.len(), old.len() + 1);
    }

    #[test]
    fn entries_already_there_are_never_overwritten_and_nothing_changes() {
        let s = splice(
            PLAIN,
            "discord_names",
            json!({"riverone": {"player": "Other", "character": null, "role": "GM"}}),
        );
        assert!(s.added.is_empty());
        assert_eq!(s.text, PLAIN);
    }

    #[test]
    fn a_new_entry_follows_the_files_indent_and_one_line_per_entry_layout() {
        let two_spaces = "{\n  \"discord_names\": {\n    \"a\": {\"player\": \"P\", \"character\": \"C\", \"role\": \"Player\"}\n  }\n}\n";
        let s = splice(
            two_spaces,
            "discord_names",
            json!({"b": {"player": null, "character": null, "role": "GM"}}),
        );
        assert!(
            s.text
                .contains("\"Player\"},\n    \"b\": {\"player\": null, \"character\": null, \"role\": \"GM\"}\n  }"),
            "{}",
            s.text
        );
    }

    #[test]
    fn a_new_entry_follows_a_file_whose_entries_span_several_lines() {
        let expanded = "{\n  \"discord_names\": {\n    \"a\": {\n      \"player\": \"P\",\n      \"character\": \"C\",\n      \"role\": \"Player\"\n    }\n  },\n  \"z\": 1\n}\n";
        let s = splice(
            expanded,
            "discord_names",
            json!({"b": {"player": "Q", "character": "D_E", "role": "Player"}}),
        );
        let want = "{\n  \"discord_names\": {\n    \"a\": {\n      \"player\": \"P\",\n      \"character\": \"C\",\n      \"role\": \"Player\"\n    },\n    \"b\": {\n      \"player\": \"Q\",\n      \"character\": \"D_E\",\n      \"role\": \"Player\"\n    }\n  },\n  \"z\": 1\n}\n";
        assert_eq!(s.text, want);
    }

    #[test]
    fn a_file_with_windows_line_endings_keeps_them_and_new_lines_match() {
        let t = "{\r\n  \"d\": {\r\n    \"a\": {\r\n      \"x\": 1\r\n    }\r\n  }\r\n}\r\n";
        let s = splice(t, "d", json!({"b": {"x": 2}}));
        assert_eq!(
            s.text,
            "{\r\n  \"d\": {\r\n    \"a\": {\r\n      \"x\": 1\r\n    },\r\n    \"b\": {\r\n      \"x\": 2\r\n    }\r\n  }\r\n}\r\n"
        );
        assert!(!s.text.replace("\r\n", "").contains('\n'), "no bare newline slipped in");
    }

    #[test]
    fn tabs_are_followed_too() {
        let t = "{\n\t\"d\": {\n\t\t\"a\": {\n\t\t\t\"x\": 1\n\t\t}\n\t}\n}\n";
        let s = splice(t, "d", json!({"b": {"x": 2}}));
        assert!(
            s.text.contains("\t\t},\n\t\t\"b\": {\n\t\t\t\"x\": 2\n\t\t}\n\t}"),
            "{}",
            s.text
        );
    }

    #[test]
    fn an_empty_or_compact_object_gets_the_originals_defaults() {
        let s = splice("{\"d\": {}}", "d", json!({"a": {"x": "y"}}));
        assert_eq!(s.text, "{\"d\": {\n    \"a\": {\"x\": \"y\"}\n}}");
        let s = splice("{\"d\": {\"k\": 1}}", "d", json!({"a": 2}));
        assert_eq!(s.text, "{\"d\": {\"k\": 1,\n    \"a\": 2\n}}");
        let s = splice("{\n  \"d\": { }\n}", "d", json!({"a": null}));
        assert_eq!(s.text, "{\n  \"d\": {\n    \"a\": null }\n}");
    }

    #[test]
    fn text_that_needs_escaping_comes_out_the_way_python_writes_it() {
        let s = splice(
            "{\"d\": {}}",
            "d",
            json!({"ü \"q\" \\ \n x": {"p": "Zoë 🎲", "c": "tab\there", "ctl": "\u{1}"}}),
        );
        assert!(
            s.text
                .contains(r#""ü \"q\" \\ \n x": {"p": "Zoë 🎲", "c": "tab\there", "ctl": "\u0001"}"#),
            "{}",
            s.text
        );
    }

    #[test]
    fn the_top_level_object_is_found_even_when_a_nested_one_has_the_same_name() {
        let t = "{\n  \"players\": {\"discord_names\": {\"nested\": 1}},\n  \"_comment\": \"see \\\"discord_names\\\": { below\",\n  \"discord_names\": {\n    \"a\": 1\n  }\n}\n";
        let s = splice(t, "discord_names", json!({"b": 2}));
        assert_eq!(s.text, t.replace("\"a\": 1\n  }", "\"a\": 1,\n    \"b\": 2\n  }"));
    }

    #[test]
    fn braces_inside_strings_do_not_confuse_the_search() {
        let t = "{\"d\": {\"a\": \"} {\", \"b\": \"\\\"}\"}}";
        let s = splice(t, "d", json!({"c": 3}));
        assert!(serde_json::from_str::<Value>(&s.text).is_ok(), "{}", s.text);
        assert_eq!(s.added, ["c"]);
    }

    #[test]
    fn bad_files_are_refused_with_the_reason() {
        let e = |t: &str, key: &str| {
            splice_object_entries(t, "t.json", key, &entries(json!({"n": 1})))
                .unwrap_err()
                .0
        };
        assert!(e("{not json", "d").contains("t.json is not valid JSON"));
        assert!(e("[1]", "d").contains("must contain a JSON object"));
        assert!(e("{\"d\": [1]}", "d").contains("\"d\" is not an object in t.json"));
        assert!(e("{\"x\": {}}", "d").contains("\"d\" is not an object in t.json"));
        assert!(e("{\"d\": {}, \"d\": {}}", "d").contains("appears twice"));
    }

    #[test]
    fn a_file_on_disk_is_replaced_whole_or_not_at_all() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("canon_aliases.json");
        std::fs::write(&p, PLAIN).unwrap();
        let path = p.to_str().unwrap();
        let added = append_object_entries(path, "discord_names", &entries(json!({"n": {"player": null}}))).unwrap();
        assert_eq!(added, ["n"]);
        assert!(
            std::fs::read_to_string(&p)
                .unwrap()
                .contains("\"n\": {\"player\": null}")
        );
        assert!(!dir.path().join("canon_aliases.json.ttrpg-tui.tmp").exists());
        // Nothing to add: the file is not touched.
        let before = std::fs::metadata(&p).unwrap().modified().unwrap();
        let added = append_object_entries(path, "discord_names", &entries(json!({"n": {"player": "z"}}))).unwrap();
        assert!(added.is_empty());
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), before);
    }

    fn row(stem: &str, player: &str, character: &str) -> SpeakerInput {
        SpeakerInput {
            stem: stem.into(),
            player: player.into(),
            character: character.into(),
        }
    }

    #[test]
    fn rows_become_entries_by_the_original_dialogs_rules() {
        let e = discord_entries(&[
            row("blank", "  ", ""),
            row("pc", " Pat ", "Mara  Quill"),
            row("gm", "Sky", ""),
            row("noplayer", "", "Tomas Ink"),
            row("dup", "First", ""),
            row("dup", "Second", "Zed"),
        ]);
        assert_eq!(e.keys().collect::<Vec<_>>(), ["pc", "gm", "noplayer", "dup"]);
        assert_eq!(
            e["pc"],
            json!({"player": "Pat", "character": "Mara_Quill", "role": "Player"})
        );
        assert_eq!(e["gm"], json!({"player": "Sky", "character": null, "role": "GM"}));
        assert_eq!(
            e["noplayer"],
            json!({"player": null, "character": "Tomas_Ink", "role": "Player"})
        );
        assert_eq!(
            e["dup"],
            json!({"player": "Second", "character": "Zed", "role": "Player"}),
            "the last row wins"
        );
    }
}
