//! The files in `examples/` are what the README tells people to copy, so they must be
//! accepted by the program as they stand.

use std::collections::HashMap;
use std::path::Path;

use tempfile::TempDir;
use ttrpg_core::{Campaign, Settings, StockRule};

fn examples() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

#[test]
fn the_example_config_is_accepted_and_spells_out_the_defaults() {
    let home = TempDir::new().unwrap();
    let env = HashMap::from([
        ("HOME".to_string(), home.path().to_string_lossy().into_owned()),
        (
            "GM_TRANSCRIBER_CONFIG".to_string(),
            examples().join("config.example.json").to_string_lossy().into_owned(),
        ),
    ]);
    let s = Settings::from_env_map(env, "ttrpg-tui");
    assert_eq!(s.stock_rule().unwrap(), StockRule::default());
    assert_eq!(s.ignore_words().unwrap().len(), 8);
    assert_eq!(s.whisper_max_context().unwrap(), 0);
    assert_eq!(s.effective_whisper_bin().unwrap(), "whisper-cli");
    assert_eq!(s.effective_python().unwrap(), "python3");
    assert_eq!(
        s.effective_model().unwrap(),
        format!(
            "{}/.local/share/whisper.cpp/models/ggml-large-v3-turbo.bin",
            home.path().display()
        )
    );
    assert_eq!(s.campaigns_subdir().unwrap(), "0.2_campaigns");
    assert_eq!(s.transcripts_subdir().unwrap(), "08_transcripts");
}

#[test]
fn the_example_alias_file_loads_as_a_campaign() {
    let tmp = TempDir::new().unwrap();
    let camp = tmp.path().join("vault/0.2_campaigns/example");
    std::fs::create_dir_all(&camp).unwrap();
    std::fs::copy(
        examples().join("canon_aliases.example.json"),
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
    let s = Settings::from_env_map(env, "ttrpg-tui");
    let c = Campaign::load(&s, "example").unwrap();
    assert_eq!(c.label(), "Emberfall");
    assert!(!c.pcs().is_empty());
    assert!(c.has_review_only());
    assert!(!c.discord_names().is_empty());
}
