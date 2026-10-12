//! The post-processing stages against what the original `transcribe.sh` wrote for the
//! same whisper output (`fixtures/pipeline/`, made by `generate.sh` with
//! `--post-process-only`). Every file must match byte for byte.

use std::fs;
use std::path::{Path, PathBuf};

use ttrpg_core::pipeline::{PostOptions, assemble, process_segments, rows_to_tsv, segments_from_json};
use ttrpg_core::stock::StockRule;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pipeline")
}

/// The speaker label the script gave each track (from `aliases.json`: the character
/// as written, `GM`, or the name in `N-name_M`).
const LABELS: [(&str, &str); 4] = [
    ("1-gmperson", "GM"),
    ("2-riverone", "Mara_Quill"),
    ("3-rivertwo", "Tomas Ink"),
    ("4-plain_0", "plain"),
];

fn options(case: &str) -> PostOptions {
    match case {
        "custom" => PostOptions {
            ignore_words: vec!["hmm".into(), "okay".into()],
            stock: StockRule {
                phrases: vec!["okay".into(), "thank you".into()],
                min_seconds: 5,
                set_by: None,
            },
        },
        "no_stock" => PostOptions {
            stock: StockRule::default().off(),
            ..PostOptions::default()
        },
        _ => PostOptions::default(),
    }
}

fn run_case(case: &str) {
    let opts = options(case);
    let expected = dir().join(case).join("expected");
    let mut processed_files: Vec<String> = Vec::new();
    let mut log: Vec<String> = Vec::new();
    for (base, label) in LABELS {
        let json = fs::read_to_string(dir().join(format!("archive/{base}.whisper_original.json"))).unwrap();
        let segments = segments_from_json(&json).unwrap();
        let p = process_segments(label, &segments, &opts);
        let file = format!("{base}.processed.tsv");
        let tsv = rows_to_tsv(&p.rows);
        assert_eq!(
            tsv,
            fs::read_to_string(expected.join("processed").join(&file)).unwrap(),
            "{case}: {file}"
        );
        log.push(format!("  {}", p.log_line(&file)));
        processed_files.push(tsv);
    }
    let out = assemble(&processed_files);
    for (name, got) in [
        ("merged_transcript.tsv", &out.merged),
        ("consolidated_transcript.tsv", &out.consolidated),
        ("transcript.md", &out.markdown),
    ] {
        assert_eq!(got, &fs::read_to_string(expected.join(name)).unwrap(), "{case}: {name}");
    }
    log.extend(out.log_lines(processed_files.len()));
    log.sort();
    let want: Vec<String> = fs::read_to_string(expected.join("log_lines.txt"))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(log, want, "{case}: the log lines");
}

#[test]
fn default_options_match_the_original_byte_for_byte() {
    run_case("default");
}

#[test]
fn other_filter_words_and_stock_settings_match_too() {
    run_case("custom");
    run_case("no_stock");
}

#[test]
fn the_goldens_exercise_what_they_are_for() {
    // If the fixtures were ever regenerated into something trivial, say so here.
    let md = fs::read_to_string(dir().join("default/expected/transcript.md")).unwrap();
    for needle in [
        "Tomas Ink",
        "plain",
        "Café ñandú — it's \"quoted\".",
        "tab\\there",
        "line1\\nline2",
        "back\\\\slash",
        "[25:00:00]",
        "[01:02:05]",
    ] {
        assert!(md.contains(needle), "default transcript lacks {needle:?}");
    }
    let custom = fs::read_to_string(dir().join("custom/expected/transcript.md")).unwrap();
    assert!(!custom.contains("Okay."), "the custom run filters okay");
    let no_stock = fs::read_to_string(dir().join("no_stock/expected/transcript.md")).unwrap();
    assert!(
        no_stock.matches("Thank you.").count() >= 3,
        "no_stock keeps the long ones"
    );
    assert!(!md.contains("We'll be right back"), "the default run drops it");
}
