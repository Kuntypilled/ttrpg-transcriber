//! The alias-candidate report against what the original `transcript_to_chunks.py
//! --report` wrote for the same campaign, transcript and word list
//! (`fixtures/report/`, made by `generate.sh`). Byte for byte, which needs the exact
//! similarity measure (`difflib`) and the same ordering rules.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::TempDir;
use ttrpg_core::alias_report::{alias_report, load_dictionary};
use ttrpg_core::{Campaign, Settings};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/report")
}

fn campaign() -> (TempDir, Campaign) {
    let tmp = TempDir::new().unwrap();
    let camp = tmp.path().join("vault/0.2_campaigns/demo");
    fs::create_dir_all(&camp).unwrap();
    fs::copy(dir().join("canon_aliases.json"), camp.join("canon_aliases.json")).unwrap();
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
    let c = Campaign::load(&Settings::from_env_map(env, "t"), "demo").unwrap();
    (tmp, c)
}

#[test]
fn every_report_matches_the_original_byte_for_byte() {
    let (_tmp, c) = campaign();
    let raw = fs::read_to_string(dir().join("transcript.md")).unwrap();
    let cases: Vec<Value> = serde_json::from_str(&fs::read_to_string(dir().join("cases.json")).unwrap()).unwrap();
    assert!(cases.len() >= 3);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let words = dir().join("words.txt");
        let dictionary = if case["dictionary"].as_bool().unwrap() {
            load_dictionary(&[words.to_str().unwrap()])
        } else {
            load_dictionary(&["/nonexistent/words"])
        };
        let got = alias_report(
            &c,
            3,
            "/TMP/vault/0.2_campaigns/demo/08_transcripts/S03/transcript.md",
            &raw,
            case["threshold"].as_f64().unwrap(),
            case["min_freq"].as_u64().unwrap() as usize,
            &dictionary,
        );
        let want = fs::read_to_string(dir().join(format!("expected_{name}.md"))).unwrap();
        assert_eq!(got, want, "report {name}");
    }
}

#[test]
fn the_fixtures_exercise_what_they_are_for() {
    let default = fs::read_to_string(dir().join("expected_default.md")).unwrap();
    assert!(
        default.contains("| Vashti Corrow | Vashtie Corrow |"),
        "section A1 has rows"
    );
    assert!(default.contains("| Nell | well |"), "section A2 has rows");
    assert!(
        default.contains("| Zorblax |") && default.contains("| Quillhaven |"),
        "section B has rows"
    );
    assert!(default.contains("\"Vashti Corrow\": ["), "the paste blocks are filled");
    let none = fs::read_to_string(dir().join("expected_no_dictionary.md")).unwrap();
    assert!(
        none.contains("None above threshold."),
        "with no dictionary nothing is English"
    );
    let loose = fs::read_to_string(dir().join("expected_loose.md")).unwrap();
    assert!(loose.len() > default.len(), "a looser threshold finds more");
}
