//! `append_object_entries` against the original in `legacy/campaign.py`: the same
//! file and the same entries must come out byte for byte the same. The files here
//! use the original's own layout (four-space keys, one-line entries), because the
//! Rust version follows the file's layout and the original always writes four spaces;
//! where they differ on purpose, the second half of this file pins the difference.
//! Needs python3; without it each test prints SKIPPED and passes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value, json};
use tempfile::TempDir;
use ttrpg_core::aliases::{append_object_entries, splice_object_entries};

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn python() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("python3"))
        .find(|p| p.is_file())
}

macro_rules! require_python {
    () => {
        match python() {
            Some(p) => p,
            None => {
                eprintln!("SKIPPED: python3 not found");
                return;
            }
        }
    };
}

fn entries(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

/// What the original does to `text`: the new file contents, or the error it raised.
fn original(py: &Path, text: &str, key: &str, new: &Map<String, Value>) -> Result<(String, Vec<String>), String> {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("canon_aliases.json");
    fs::write(&file, text).unwrap();
    let out = Command::new(py)
        .args(["-I", "-B"])
        .arg(manifest().join("tests/support/append_entries.py"))
        .arg(manifest().join("../../legacy").canonicalize().unwrap())
        .arg(&file)
        .arg(key)
        .arg(Value::Object(new.clone()).to_string())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    match reply.get("error") {
        Some(e) => Err(e.as_str().unwrap().to_string()),
        None => Ok((
            fs::read_to_string(&file).unwrap(),
            reply["added"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect(),
        )),
    }
}

fn rust(text: &str, key: &str, new: &Map<String, Value>) -> Result<(String, Vec<String>), String> {
    splice_object_entries(text, "canon_aliases.json", key, new)
        .map(|s| (s.text, s.added))
        .map_err(|e| e.0)
}

const REALISTIC: &str = r#"{
  "_comment": "Hand-kept. Keys starting with _ are notes.",
  "campaign": "Alpha Run",
  "system": "daggerheart",
  "seed_prompt": "Invented names: Mara Quill, Tomas Ink.",
  "aliases": {
    "Mara Quill": ["Mara Quil", "Marra Quill"],
    "Tomas Ink": ["Thomas Ink"]
  },
  "names": ["Marrow Hall", "Cinder Road"],
  "discord_names": {
    "_comment": "stem -> who",
    "gmperson": {"player": "Sky", "character": null,   "role": "GM"},
    "riverone": {"player": "Pat", "character": "Mara_Quill", "role": "Player"}
  },
  "speaker_map": [
    {"match": "gm", "player": null, "character": null}
  ],
  "players": {"Mara Quill": "Pat"}
}
"#;

fn same_as_original(name: &str, text: &str, key: &str, new: Value) {
    let py = require_python_or_return(name);
    let Some(py) = py else { return };
    let new = entries(new);
    let (want, want_added) =
        original(&py, text, key, &new).unwrap_or_else(|e| panic!("{name}: the original failed: {e}"));
    let (got, got_added) = rust(text, key, &new).unwrap_or_else(|e| panic!("{name}: rust failed: {e}"));
    assert_eq!(got_added, want_added, "{name}: keys added");
    assert_eq!(got, want, "{name}: file text");
}

fn require_python_or_return(name: &str) -> Option<PathBuf> {
    let p = python();
    if p.is_none() {
        eprintln!("SKIPPED ({name}): python3 not found");
    }
    p
}

#[test]
fn a_realistic_file_gets_the_same_bytes() {
    let _ = require_python!();
    same_as_original(
        "one new speaker",
        REALISTIC,
        "discord_names",
        json!({"rivertwo": {"player": "Lee", "character": "Tomas_Ink", "role": "Player"}}),
    );
    same_as_original(
        "several, some already there",
        REALISTIC,
        "discord_names",
        json!({
            "gmperson": {"player": "Other", "character": null, "role": "GM"},
            "newone": {"player": null, "character": null, "role": "GM"},
            "newtwo": {"player": "Zoë", "character": "Tomas_Ink", "role": "Player"}
        }),
    );
    same_as_original(
        "nothing new",
        REALISTIC,
        "discord_names",
        json!({"riverone": {"player": "x", "character": null, "role": "GM"}}),
    );
    same_as_original(
        "another object",
        REALISTIC,
        "aliases",
        json!({"Tomas Ink": ["Tomas Inc"]}),
    );
}

#[test]
fn awkward_layouts_get_the_same_bytes() {
    let _ = require_python!();
    let new = || json!({"zz": {"player": "P", "character": "C_D", "role": "Player"}});
    same_as_original(
        "empty object",
        "{\n    \"discord_names\": {}\n}\n",
        "discord_names",
        new(),
    );
    same_as_original("empty with a space", "{\"discord_names\": { }}", "discord_names", new());
    same_as_original(
        "compact",
        "{\"discord_names\": {\"a\": {\"role\": \"GM\"}}}",
        "discord_names",
        new(),
    );
    same_as_original(
        "trailing blanks before the brace",
        "{\n  \"discord_names\": {\n    \"a\": 1  \n\n  }\n}",
        "discord_names",
        new(),
    );
    same_as_original(
        "no trailing newline, key last",
        "{\n  \"x\": 1,\n  \"discord_names\": {\n    \"a\": 1\n  }\n}",
        "discord_names",
        new(),
    );
    same_as_original(
        "key first",
        "{\"discord_names\": {\n    \"a\": 1\n},\n  \"x\": [1, {\"y\": 2}]}\n",
        "discord_names",
        new(),
    );
}

#[test]
fn text_with_escapes_and_unicode_gets_the_same_bytes() {
    let _ = require_python!();
    same_as_original(
        "escapes",
        "{\n  \"discord_names\": {\n    \"a\": 1\n  }\n}\n",
        "discord_names",
        json!({
            "ünï \"q\" \\ x": {"player": "Zoë 🎲 \"Z\"", "character": "tab\there\nnewline", "role": "Player"},
            "ctl\u{1}\u{1f}": {"player": "\u{7f}", "character": null, "role": "GM"}
        }),
    );
}

#[test]
fn both_refuse_the_same_bad_files() {
    let py = require_python!();
    let new = entries(json!({"n": {"player": null}}));
    for (name, text, key) in [
        ("not json", "{not json", "discord_names"),
        ("not an object", "[1, 2]", "discord_names"),
        ("key missing", "{\"x\": {}}", "discord_names"),
        ("key is a list", "{\"discord_names\": [1]}", "discord_names"),
    ] {
        assert!(original(&py, text, key, &new).is_err(), "{name}: the original refuses");
        assert!(rust(text, key, &new).is_err(), "{name}: rust refuses");
    }
}

// -- differences on purpose ------------------------------------------------------------------

#[test]
fn a_nested_object_with_the_same_name_is_not_mistaken_for_the_top_level_one() {
    let py = require_python!();
    let text = "{\n    \"players\": {\"discord_names\": {\"nested\": 1}},\n    \"discord_names\": {\n        \"a\": 1\n    }\n}\n";
    let new = entries(json!({"b": 2}));
    // The original splices into the first match, the nested one, and its own check
    // then aborts because `players` would change.
    let theirs = original(&py, text, "discord_names", &new);
    assert!(
        theirs.as_ref().is_err_and(|e| e.contains("would have altered")),
        "{theirs:?}"
    );
    let (got, added) = rust(text, "discord_names", &new).unwrap();
    assert_eq!(added, ["b"]);
    assert!(got.contains("\"a\": 1,\n        \"b\": 2\n    }"), "{got}");
}

#[test]
fn windows_line_endings_survive_here_and_not_in_the_original() {
    let py = require_python!();
    let text = "{\r\n    \"discord_names\": {\r\n        \"a\": 1\r\n    }\r\n}\r\n";
    let new = entries(json!({"b": 2}));
    // Python reads in text mode, so the whole file comes back with bare \n.
    let (theirs, _) = original(&py, text, "discord_names", &new).unwrap();
    assert!(!theirs.contains('\r'));
    let (got, _) = rust(text, "discord_names", &new).unwrap();
    assert_eq!(
        got,
        "{\r\n    \"discord_names\": {\r\n        \"a\": 1,\r\n        \"b\": 2\r\n    }\r\n}\r\n"
    );
}

#[test]
fn writing_a_file_leaves_no_temporary_behind() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("canon_aliases.json");
    fs::write(&file, REALISTIC).unwrap();
    let added = append_object_entries(
        file.to_str().unwrap(),
        "discord_names",
        &entries(json!({"n": {"player": null, "character": null, "role": "GM"}})),
    )
    .unwrap();
    assert_eq!(added, ["n"]);
    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
}

#[test]
fn a_file_whose_entries_span_lines_gets_entries_that_look_like_its_own_but_mean_the_same() {
    let py = require_python!();
    let text = "{\n  \"discord_names\": {\n    \"a\": {\n      \"player\": \"P\",\n      \"character\": null,\n      \"role\": \"GM\"\n    }\n  }\n}\n";
    let new = entries(json!({"b": {"player": "Q", "character": "R_S", "role": "Player"}}));
    let (theirs, _) = original(&py, text, "discord_names", &new).unwrap();
    let (got, _) = rust(text, "discord_names", &new).unwrap();
    // The original writes the new entry on one line, whatever the file looks like.
    assert!(theirs.contains("\"b\": {\"player\": \"Q\", \"character\": \"R_S\", \"role\": \"Player\"}"));
    // Here it is laid out like "a".
    assert!(got.contains("    \"b\": {\n      \"player\": \"Q\",\n      \"character\": \"R_S\",\n      \"role\": \"Player\"\n    }\n  }"), "{got}");
    // Same data either way, and the old part is untouched in both.
    let (a, b): (Value, Value) = (
        serde_json::from_str(&theirs).unwrap(),
        serde_json::from_str(&got).unwrap(),
    );
    assert_eq!(a, b);
    assert!(got.starts_with(&text[..text.rfind("    }\n  }").unwrap()]));
}
