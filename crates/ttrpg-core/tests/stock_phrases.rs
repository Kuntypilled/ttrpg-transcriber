//! The stock-phrase rule: whisper invents "Thank you." on a silent stretch of a track, one
//! per 30 s window, and the collapse step stretches the repeats into one long line. The
//! rule drops a line that is exactly a stock phrase only when it lasts 20 s or more. These
//! tests replay a hand-made whisper output through the post-processing, so no audio and
//! no whisper are needed. (`pipeline.rs` compares the same stage with the original
//! script's output byte for byte; this file is about the rule's edges.)

use ttrpg_core::StockRule;
use ttrpg_core::pipeline::{PostOptions, Processed, RawSegment, process_segments, segments_from_json};

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

fn segments() -> Vec<RawSegment> {
    let entries: Vec<String> = TRACK
        .iter()
        .map(|(from, to, text, _)| format!(r#"{{"offsets":{{"from":{from},"to":{to}}},"text":"{text}"}}"#))
        .collect();
    segments_from_json(&format!(r#"{{"transcription":[{}]}}"#, entries.join(","))).unwrap()
}

fn replay(stock: StockRule) -> Processed {
    process_segments(
        "mara",
        &segments(),
        &PostOptions {
            stock,
            ..PostOptions::default()
        },
    )
}

fn rule(phrases: &[&str], min_seconds: u32) -> StockRule {
    StockRule {
        phrases: phrases.iter().map(|p| (*p).to_string()).collect(),
        min_seconds,
        set_by: None,
    }
}

/// The kept rows as (start_ms, end_ms, text).
fn kept(p: &Processed) -> Vec<(i64, i64, String)> {
    p.rows
        .iter()
        .map(|r| {
            assert_eq!(r.speaker, "mara");
            (r.start, r.end, r.text.clone())
        })
        .collect()
}

#[test]
fn by_default_only_long_stock_lines_are_dropped_and_nothing_else_moves() {
    let out = replay(StockRule::default());
    let want: Vec<(i64, i64, String)> = TRACK
        .iter()
        .filter(|(_, _, _, kept)| *kept)
        .map(|(a, b, t, _)| (*a as i64, *b as i64, t.trim().to_string()))
        .collect();
    assert_eq!(kept(&out), want, "kept lines keep their own start and end times");

    // The 60 s run counts once: it was one line by then.
    assert_eq!(
        out.log_line("1-mara_0.processed.tsv"),
        "Processed: 1-mara_0.processed.tsv — filtered: 0, collapsed: 1, stock phrases dropped: 4"
    );
    let text: Vec<String> = kept(&out).into_iter().map(|r| r.2).collect();
    assert!(text.contains(&"Thank you.".to_string()) && text.contains(&"Thank you very much.".to_string()));
    assert!(!text.iter().any(|t| t.contains("THANK YOU") || t.contains("right back")));
}

#[test]
fn an_empty_list_turns_the_rule_off_and_the_log_line_is_the_original_one() {
    let out = replay(StockRule::default().off());
    // Seventeen lines in, the two silent windows collapsed into one.
    assert_eq!(out.rows.len(), TRACK.len() - 1);
    assert_eq!(
        out.log_line("1-mara_0.processed.tsv"),
        "Processed: 1-mara_0.processed.tsv — filtered: 0, collapsed: 1"
    );
}

#[test]
fn the_list_and_the_length_can_be_changed() {
    let out = replay(rule(&["thanks for watching", "thank you very much"], 20));
    let texts: Vec<String> = kept(&out).into_iter().map(|r| r.2).collect();
    assert!(!texts.iter().any(|t| t == "Thanks for watching."));
    assert!(!texts.iter().any(|t| t == "Thank you very much."));
    assert!(texts.iter().any(|t| t == "THANK YOU!"), "not on this list");

    // Dropping from 1 s up also takes the 1.5 s one and the 19.999 s one.
    let out = replay(rule(&["thank you", "we'll be right back"], 1));
    let thanks = kept(&out).into_iter().filter(|r| r.2 == "Thank you.").count();
    assert_eq!(thanks, 0);
}
