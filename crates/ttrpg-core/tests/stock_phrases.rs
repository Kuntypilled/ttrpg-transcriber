//! The stock-phrase rule in `legacy/transcribe.sh`: whisper invents "Thank you."
//! on a silent stretch of a track, one per 30 s window, and the collapse step
//! stretches the repeats into one long line. The rule drops a line that is exactly
//! a stock phrase only when it lasts 20 s or more. These tests replay hand-made
//! whisper JSON (`--post-process-only`), so no audio and no whisper are needed,
//! only bash, python3 and jq; without them each test prints SKIPPED and passes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn tools_present() -> bool {
    ["bash", "python3", "jq"].iter().all(|t| {
        Command::new("sh")
            .args(["-c", &format!("command -v {t}")])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

macro_rules! require_tools {
    () => {
        if !tools_present() {
            eprintln!("SKIPPED: bash, python3 and jq are needed");
            return;
        }
    };
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../legacy/transcribe.sh")
}

/// One track, `1-mara_0`, as whisper would have archived it. Times are in ms.
/// Each entry is (from, to, text, kept by default).
const TRACK: &[(u64, u64, &str, bool)] = &[
    (0, 3_000, " The gate is shut.", true),
    // Two silent windows: the collapse step joins them into one 60 s line.
    (3_000, 33_000, " Thank you.", false),
    (33_000, 63_000, " Thank you.", false),
    (64_000, 66_000, " We move at dawn.", true),
    // Someone actually saying it: 1.5 s.
    (66_500, 68_000, " Thank you.", true),
    (69_000, 71_000, " Bring the rope.", true),
    // One millisecond under the limit stays, exactly the limit goes.
    (72_000, 91_999, " Thank you.", true),
    (93_000, 95_000, " Hold the line.", true),
    (96_000, 116_000, " Thank you.", false),
    (117_000, 119_000, " Light the torch.", true),
    // Case and one trailing mark are ignored, as in the filler-word filter.
    (120_000, 150_000, " THANK YOU!", false),
    (151_000, 153_000, " Rest now.", true),
    // Only a line that is the whole phrase counts.
    (154_000, 184_000, " Thank you very much.", true),
    (185_000, 187_000, " Go.", true),
    (188_000, 218_000, " We'll be right back.", false),
    (219_000, 221_000, " Later.", true),
    // Not in the default list.
    (222_000, 252_000, " Thanks for watching.", true),
];

fn session() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("archive")).unwrap();
    fs::create_dir_all(dir.path().join("home")).unwrap();
    let segments: Vec<String> = TRACK
        .iter()
        .map(|(from, to, text, _)| format!(r#"{{"offsets":{{"from":{from},"to":{to}}},"text":"{text}"}}"#))
        .collect();
    fs::write(
        dir.path().join("archive/1-mara_0.whisper_original.json"),
        format!(r#"{{"transcription":[{}]}}"#, segments.join(",")),
    )
    .unwrap();
    dir
}

fn replay(dir: &Path, extra: &[&str]) -> Output {
    Command::new("bash")
        .arg(script())
        .args(["-p", "-o"])
        .arg(dir)
        .arg("--temp-dir")
        .arg(dir.join("temp"))
        .args(extra)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("home/.config"))
        .output()
        .unwrap()
}

/// The merged transcript as (start_ms, end_ms, text) rows.
fn merged(dir: &Path) -> Vec<(u64, u64, String)> {
    fs::read_to_string(dir.join("merged_transcript.tsv"))
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f[0], "mara");
            (f[1].parse().unwrap(), f[2].parse().unwrap(), f[3].to_string())
        })
        .collect()
}

fn processed_log_line(dir: &Path) -> String {
    fs::read_to_string(dir.join("transcription_log.csv"))
        .unwrap()
        .lines()
        .rfind(|l| l.contains("Processed:"))
        .unwrap()
        .to_string()
}

#[test]
fn by_default_only_long_stock_lines_are_dropped_and_nothing_else_moves() {
    require_tools!();
    let dir = session();
    let out = replay(dir.path(), &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let rows = merged(dir.path());
    let want: Vec<(u64, u64, String)> = TRACK
        .iter()
        .filter(|(_, _, _, kept)| *kept)
        .map(|(a, b, t, _)| (*a, *b, t.trim().to_string()))
        .collect();
    assert_eq!(rows, want, "kept lines keep their own start and end times");

    // The 60 s run counts once: it was one line by then.
    let line = processed_log_line(dir.path());
    assert!(
        line.ends_with("filtered: 0, collapsed: 1, stock phrases dropped: 4"),
        "{line}"
    );
    let md = fs::read_to_string(dir.path().join("transcript.md")).unwrap();
    assert!(md.contains("Thank you.") && md.contains("Thank you very much."));
    assert!(!md.contains("THANK YOU") && !md.contains("right back"));
}

#[test]
fn an_empty_list_turns_the_rule_off_and_the_log_line_is_the_original_one() {
    require_tools!();
    let dir = session();
    let out = replay(dir.path(), &["--stock-phrases", ""]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // Seventeen lines in, the two silent windows collapsed into one.
    assert_eq!(merged(dir.path()).len(), TRACK.len() - 1);
    let line = processed_log_line(dir.path());
    assert!(line.ends_with("filtered: 0, collapsed: 1"), "{line}");
}

#[test]
fn the_list_and_the_length_can_be_changed() {
    require_tools!();
    let dir = session();

    let out = replay(
        dir.path(),
        &["--stock-phrases", "thanks for watching,thank you very much"],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let texts: Vec<String> = merged(dir.path()).into_iter().map(|r| r.2).collect();
    assert!(!texts.iter().any(|t| t == "Thanks for watching."));
    assert!(!texts.iter().any(|t| t == "Thank you very much."));
    assert!(texts.iter().any(|t| t == "THANK YOU!"), "not on this list");

    // Dropping from 1 s up also takes the 1.5 s one and the 19.999 s one.
    let out = replay(dir.path(), &["--stock-min-seconds", "1"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let thanks = merged(dir.path()).into_iter().filter(|r| r.2 == "Thank you.").count();
    assert_eq!(thanks, 0);
}

#[test]
fn a_bad_length_is_refused_before_anything_runs() {
    require_tools!();
    let dir = session();
    let out = replay(dir.path(), &["--stock-min-seconds", "soon"]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--stock-min-seconds must be a whole number"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dir.path().join("merged_transcript.tsv").exists());
}
