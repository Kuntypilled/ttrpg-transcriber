//! The chunker against what the original `transcript_to_chunks.py` wrote for the same
//! campaign file and transcript (`fixtures/chunker/`, made by `generate.sh`). The JSONL
//! must match byte for byte: a different chunk shape would invalidate an existing index.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use ttrpg_core::chunker::chunk_transcript;
use ttrpg_core::{Campaign, Settings};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chunker")
}

fn campaign(case: &str) -> (TempDir, Campaign) {
    let tmp = TempDir::new().unwrap();
    let camp = tmp.path().join("vault/0.2_campaigns/demo");
    fs::create_dir_all(&camp).unwrap();
    fs::copy(
        dir().join(case).join("canon_aliases.json"),
        camp.join("canon_aliases.json"),
    )
    .unwrap();
    let env = HashMap::from([
        ("HOME".to_string(), tmp.path().to_string_lossy().into_owned()),
        (
            "GM_VAULT".to_string(),
            tmp.path().join("vault").to_string_lossy().into_owned(),
        ),
        (
            "GM_TRANSCRIBER_CONFIG".to_string(),
            tmp.path().join("none.json").to_string_lossy().into_owned(),
        ),
    ]);
    let settings = Settings::from_env_map(env, "t");
    let c = Campaign::load(&settings, "demo").unwrap();
    (tmp, c)
}

fn check(case: &str) {
    let (_tmp, c) = campaign(case);
    let raw = fs::read_to_string(dir().join(case).join("transcript.md")).unwrap();
    let run = chunk_transcript(&c, &raw, 3, "2026-01-02");
    let want = fs::read_to_string(dir().join(case).join("expected.jsonl")).unwrap();
    assert_eq!(run.jsonl, want, "{case}: the JSONL");
    let summary = fs::read_to_string(dir().join(case).join("expected_summary.txt")).unwrap();
    assert_eq!(run.summary(3), summary.trim_end(), "{case}: the summary line");
}

#[test]
fn an_ordinary_transcript_chunks_the_same_bytes() {
    check("basic");
}

#[test]
fn the_odd_shapes_chunk_the_same_bytes() {
    check("edges");
}

#[test]
fn the_fixtures_exercise_what_they_are_for() {
    let basic = fs::read_to_string(dir().join("basic/expected.jsonl")).unwrap();
    assert!(
        basic.lines().count() >= 5,
        "several windows, so the overlap is exercised"
    );
    assert!(
        basic.contains("\\u00eb") && basic.contains("\\ud83c\\udfb2"),
        "ensure_ascii escapes"
    );
    assert!(
        basic.contains("Marrow Hall") && !basic.contains("marro hall"),
        "aliases applied"
    );
    assert!(basic.contains("borrow hall"), "review_only is never applied");
    let first = basic.lines().next().unwrap();
    let t = first.find("has_Tomas_Ink").unwrap();
    let m = first.find("has_Mara_Quill").unwrap();
    assert!(t < m, "has_<PC> follows the players key order, not the alphabet");
}
