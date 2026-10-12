//! Which session number to offer (`Campaign::suggest_session`).

use std::collections::HashMap;
use std::fs;

use tempfile::TempDir;
use ttrpg_core::{Campaign, Settings};

fn setup() -> (TempDir, Settings, Campaign) {
    let tmp = TempDir::new().unwrap();
    let camp = tmp.path().join("vault/0.2_campaigns/demo");
    fs::create_dir_all(&camp).unwrap();
    fs::write(camp.join("canon_aliases.json"), "{}").unwrap();
    let p = |rel: &str| tmp.path().join(rel).to_string_lossy().into_owned();
    let env = HashMap::from([
        ("HOME".to_string(), p("home")),
        ("GM_VAULT".to_string(), p("vault")),
        ("GM_AUDIO_ROOT".to_string(), p("audio")),
        ("GM_TRANSCRIBER_CONFIG".to_string(), p("none.json")),
    ]);
    let settings = Settings::from_env_map(env, "ttrpg-tui");
    let campaign = Campaign::load(&settings, "demo").unwrap();
    (tmp, settings, campaign)
}

fn mkdir(tmp: &TempDir, rel: &str) {
    fs::create_dir_all(tmp.path().join(rel)).unwrap();
}

#[test]
fn a_new_campaign_starts_at_one() {
    let (_t, s, c) = setup();
    assert_eq!(c.session_numbers(&s).unwrap(), Vec::<i64>::new());
    assert_eq!(c.suggest_session(&s), 1);
}

#[test]
fn after_a_finished_session_offer_the_next() {
    let (t, s, c) = setup();
    for n in ["S01", "S02"] {
        mkdir(&t, &format!("vault/0.2_campaigns/demo/08_transcripts/{n}"));
        fs::write(
            t.path()
                .join(format!("vault/0.2_campaigns/demo/08_transcripts/{n}/transcript.md")),
            "x",
        )
        .unwrap();
    }
    assert_eq!(c.session_numbers(&s).unwrap(), [1, 2]);
    assert_eq!(c.suggest_session(&s), 3);
}

#[test]
fn an_unfinished_latest_session_is_offered_again() {
    let (t, s, c) = setup();
    mkdir(&t, "vault/0.2_campaigns/demo/08_transcripts/S01");
    fs::write(
        t.path()
            .join("vault/0.2_campaigns/demo/08_transcripts/S01/transcript.md"),
        "x",
    )
    .unwrap();
    mkdir(&t, "vault/0.2_campaigns/demo/08_transcripts/S02"); // run started, no transcript
    assert_eq!(c.suggest_session(&s), 2);
}

#[test]
fn audio_folders_count_too_and_odd_names_do_not() {
    let (t, s, c) = setup();
    mkdir(&t, "audio/demo/Session 7"); // audio moved in, nothing transcribed
    mkdir(&t, "audio/demo/Session x");
    mkdir(&t, "audio/demo/Session ");
    mkdir(&t, "vault/0.2_campaigns/demo/08_transcripts/chunks");
    mkdir(&t, "vault/0.2_campaigns/demo/08_transcripts/S3x");
    fs::write(
        t.path().join("vault/0.2_campaigns/demo/08_transcripts/S99"),
        "a file, not a folder",
    )
    .unwrap();
    assert_eq!(c.session_numbers(&s).unwrap(), [7]);
    assert_eq!(c.suggest_session(&s), 7);
}

#[test]
fn numbers_above_nine_sort_numerically() {
    let (t, s, c) = setup();
    for n in ["S2", "S10", "S9"] {
        mkdir(&t, &format!("vault/0.2_campaigns/demo/08_transcripts/{n}"));
    }
    assert_eq!(c.session_numbers(&s).unwrap(), [2, 9, 10]);
}
