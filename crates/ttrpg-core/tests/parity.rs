//! Parity tests: the Rust port against the pinned Python original.
//!
//! Each test builds a throwaway vault from invented fixtures, then runs the same
//! command through `tests/oracle/campaign.py` and through ttrpg-core with an identical
//! environment, and requires identical output and exit code (PORTING.md section 9,
//! items 5 and 6). They need `python3`; without it each test prints SKIPPED.
//!
//! Only two normalisations are applied to the Python output, both for text that
//! names the program or an example id: `python3 campaign.py` becomes `ttrpg-tui`
//! and the original's example campaign id becomes the one used here.
//! The text of a JSON syntax error is masked, because serde_json and Python word it
//! differently.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;
use ttrpg_core::report::{doctor, list_output, paths_output, whisper_device_output};
use ttrpg_core::{Campaign, Settings};

const CLI: &str = "ttrpg-tui";

// -- harness ----------------------------------------------------------------------

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn oracle_dir() -> PathBuf {
    manifest()
        .join("tests/oracle")
        .canonicalize()
        .expect("tests/oracle directory")
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

struct Fixture {
    tmp: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fx = Fixture {
            tmp: TempDir::new().unwrap(),
        };
        fs::create_dir_all(fx.path("home")).unwrap();
        fx
    }

    fn path(&self, rel: &str) -> String {
        self.tmp.path().join(rel).to_string_lossy().into_owned()
    }

    fn write(&self, rel: &str, contents: &str) {
        let p = self.tmp.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    /// Copy a fixture file into the temp tree.
    fn install(&self, rel: &str, source: &Path) {
        self.write(rel, &fs::read_to_string(source).unwrap());
    }

    fn base_env(&self) -> HashMap<String, String> {
        HashMap::from([
            ("HOME".to_string(), self.path("home")),
            ("LC_ALL".to_string(), "C.UTF-8".to_string()),
        ])
    }
}

fn fixture_file(name: &str) -> PathBuf {
    manifest().join("tests/fixtures").join(name)
}

fn example_file() -> PathBuf {
    manifest().join("../../examples/canon_aliases.example.json")
}

fn env_with(fx: &Fixture, extra: &[(&str, String)]) -> HashMap<String, String> {
    let mut env = fx.base_env();
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.clone())));
    env
}

#[derive(Debug, PartialEq, Eq)]
struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

fn mask_json_error(s: &str) -> String {
    const MARK: &str = "is not valid JSON:";
    s.split_inclusive('\n')
        .map(|line| {
            let (body, nl) = match line.strip_suffix('\n') {
                Some(b) => (b, "\n"),
                None => (line, ""),
            };
            match body.find(MARK) {
                Some(i) => format!("{}{nl}", &body[..i + MARK.len()]),
                None => line.to_string(),
            }
        })
        .collect()
}

fn normalise(s: &str) -> String {
    mask_json_error(
        &s.replace("python3 campaign.py", CLI)
            .replace("emberfall/main", "emberfall/main"),
    )
}

fn run_oracle(py: &Path, env: &HashMap<String, String>, args: &[&str]) -> Out {
    let o = Command::new(py)
        .args(["-I", "-B"])
        .arg(oracle_dir().join("campaign.py"))
        .args(args)
        .env_clear()
        .envs(env)
        .output()
        .expect("run python");
    Out {
        stdout: normalise(&String::from_utf8(o.stdout).unwrap()),
        stderr: normalise(&String::from_utf8(o.stderr).unwrap()),
        code: o.status.code().unwrap_or(-1),
    }
}

fn error_out(e: impl std::fmt::Display) -> Out {
    Out {
        stdout: String::new(),
        stderr: mask_json_error(&format!("ERROR: {e}\n")),
        code: 2,
    }
}

fn run_rust(env: &HashMap<String, String>, args: &[&str]) -> Out {
    let out = run_rust_raw(env, args);
    Out {
        stdout: mask_json_error(&out.stdout),
        stderr: mask_json_error(&out.stderr),
        code: out.code,
    }
}

fn run_rust_raw(env: &HashMap<String, String>, args: &[&str]) -> Out {
    let s = Settings::from_env_map(env.clone(), CLI);
    match args {
        ["list"] => Out {
            stdout: list_output(&s),
            stderr: String::new(),
            code: 0,
        },
        ["whisper-device"] => match whisper_device_output(&s) {
            Ok(o) => Out {
                stdout: o,
                stderr: String::new(),
                code: 0,
            },
            Err(e) => error_out(e),
        },
        ["doctor"] => {
            let r = doctor(&s);
            match r.result {
                Ok(code) => Out {
                    stdout: r.out,
                    stderr: String::new(),
                    code,
                },
                Err(e) => Out {
                    stdout: r.out,
                    ..error_out(e)
                },
            }
        }
        ["paths", "--campaign", id, rest @ ..] => {
            let session = match rest {
                ["--session", n] => Some(n.parse::<i64>().unwrap()),
                [] => None,
                other => panic!("unsupported args {other:?}"),
            };
            match paths_output(&s, id, session) {
                Ok(o) => Out {
                    stdout: o,
                    stderr: String::new(),
                    code: 0,
                },
                Err(e) => error_out(e),
            }
        }
        other => panic!("unsupported args {other:?}"),
    }
}

/// Run one command both ways and require identical results.
fn assert_parity(py: &Path, env: &HashMap<String, String>, args: &[&str]) {
    let want = run_oracle(py, env, args);
    let got = run_rust(env, args);
    assert_eq!(got, want, "Rust differs from Python for {args:?}");
}

fn assert_parity_many(py: &Path, env: &HashMap<String, String>, sets: &[&[&str]]) {
    for args in sets {
        assert_parity(py, env, args);
    }
}

// -- a vault per world ------------------------------------------------------------

/// Two worlds, a mix of valid, broken, empty and hidden campaign folders.
fn build_multi_vault(fx: &Fixture) {
    let ex = example_file();
    fx.install("vaults/emberfall/0.2_campaigns/main/canon_aliases.json", &ex);
    fx.install(
        "vaults/emberfall/0.2_campaigns/main-old/canon_aliases.json",
        &fixture_file("players_only.json"),
    );
    fx.install(
        "vaults/emberfall/0.2_campaigns/side/canon_aliases.json",
        &fixture_file("sparse.json"),
    );
    fx.write("vaults/emberfall/0.2_campaigns/broken/canon_aliases.json", "{ not json");
    fs::create_dir_all(fx.path("vaults/emberfall/0.2_campaigns/empty")).unwrap();
    fx.install("vaults/emberfall/0.2_campaigns/.hidden/canon_aliases.json", &ex);
    fx.install(
        "vaults/thornwake/0.2_campaigns/messy/canon_aliases.json",
        &fixture_file("messy.json"),
    );
    fs::create_dir_all(fx.path("vaults/scratch/notes")).unwrap();
    fx.install("vaults/.obsidian/0.2_campaigns/ghost/canon_aliases.json", &ex);
    fx.write("vaults/README.txt", "not a world");
}

fn multi_env(fx: &Fixture) -> HashMap<String, String> {
    env_with(
        fx,
        &[
            ("GM_VAULTS_ROOT", fx.path("vaults")),
            ("GM_AUDIO_ROOT", "~/aud".to_string()),
            ("GM_CHROMA_ROOT", fx.path("chroma//x/../y")),
            ("GM_WORK_ROOT", "rel/work".to_string()),
        ],
    )
}

#[test]
fn multi_vault_list_doctor_and_paths() {
    let py = require_python!();
    let fx = Fixture::new();
    build_multi_vault(&fx);
    let env = multi_env(&fx);
    assert_parity_many(&py, &env, &[&["list"], &["doctor"], &["whisper-device"]]);
    for id in [
        "emberfall/main",
        "emberfall/main-old",
        "emberfall/side",
        "thornwake/messy",
    ] {
        assert_parity(&py, &env, &["paths", "--campaign", id]);
        for s in ["1", "7", "9", "10", "13", "123", "0", "-5", "12345"] {
            assert_parity(&py, &env, &["paths", "--campaign", id, "--session", s]);
        }
    }
}

#[test]
fn multi_vault_lists_in_original_order() {
    // "main" sorts after "main-old" because the sort key is the whole path, where
    // the character after "main" is "-" in one and "/" in the other.
    let py = require_python!();
    let fx = Fixture::new();
    build_multi_vault(&fx);
    let env = multi_env(&fx);
    let want = run_oracle(&py, &env, &["list"]).stdout;
    let order: Vec<&str> = want.lines().map(|l| l.split('\t').next().unwrap()).collect();
    assert_eq!(
        order,
        [
            "emberfall/main-old",
            "emberfall/main",
            "emberfall/side",
            "thornwake/messy"
        ]
    );
    assert_eq!(run_rust(&env, &["list"]).stdout, want);
}

#[test]
fn multi_vault_rejects_bad_ids() {
    let py = require_python!();
    let fx = Fixture::new();
    build_multi_vault(&fx);
    let env = multi_env(&fx);
    for id in [
        "main",
        "a/b/c",
        "emberfall/..",
        "../x",
        "emberfall/",
        "/main",
        "nonexistent/x",
        "emberfall/empty",
        "emberfall/broken",
        "emberfall/.hidden",
        "./x",
        "emberfall/.",
    ] {
        assert_parity(&py, &env, &["paths", "--campaign", id]);
    }
}

// -- single vault, config file ----------------------------------------------------

#[test]
fn single_vault_from_config_file() {
    let py = require_python!();
    let fx = Fixture::new();
    fx.install("home/myvault/0.2_campaigns/main/canon_aliases.json", &example_file());
    fx.install(
        "home/myvault/0.2_campaigns/side/canon_aliases.json",
        &fixture_file("sparse.json"),
    );
    fx.write(
        "cfg/config.json",
        r#"{"_comment": "ignored", "unknown_key": [1, 2], "vault": "~/myvault",
            "audio_root": "audio/rel", "whisper_device": 1, "campaigns_subdir": "0.2_campaigns"}"#,
    );
    let env = env_with(&fx, &[("GM_TRANSCRIBER_CONFIG", fx.path("cfg/config.json"))]);
    assert_parity_many(&py, &env, &[&["list"], &["doctor"], &["whisper-device"]]);
    for id in ["main", "side"] {
        assert_parity(&py, &env, &["paths", "--campaign", id, "--session", "13"]);
    }
    // multi-vault style ids and bad ids are rejected in single-vault mode
    for id in ["emberfall/main", "..", "", "main/"] {
        assert_parity(&py, &env, &["paths", "--campaign", id]);
    }
}

#[test]
fn custom_subdirs_from_env_and_config() {
    let py = require_python!();
    let fx = Fixture::new();
    fx.install("home/v/camps/main/canon_aliases.json", &example_file());
    fx.write(
        "cfg/config.json",
        r#"{"vault": "~/v", "campaigns_subdir": "camps", "transcripts_subdir": "tx"}"#,
    );
    let env = env_with(&fx, &[("GM_TRANSCRIBER_CONFIG", fx.path("cfg/config.json"))]);
    assert_parity_many(
        &py,
        &env,
        &[
            &["list"],
            &["doctor"],
            &["paths", "--campaign", "main", "--session", "4"],
        ],
    );
    let env = env_with(
        &fx,
        &[
            ("GM_TRANSCRIBER_CONFIG", fx.path("cfg/config.json")),
            ("GM_TRANSCRIPTS_SUBDIR", "other".into()),
        ],
    );
    assert_parity(&py, &env, &["paths", "--campaign", "main", "--session", "4"]);
}

#[test]
fn vaults_root_wins_over_single_vault() {
    let py = require_python!();
    let fx = Fixture::new();
    build_multi_vault(&fx);
    fx.install("home/myvault/0.2_campaigns/main/canon_aliases.json", &example_file());
    let mut env = multi_env(&fx);
    env.insert("GM_VAULT".into(), fx.path("home/myvault"));
    assert_parity_many(&py, &env, &[&["list"], &["doctor"]]);
    let mut env = multi_env(&fx);
    env.insert("VAULT".into(), fx.path("home/myvault"));
    assert_parity(&py, &env, &["doctor"]);
}

#[test]
fn unconfigured_vault_gives_instructions_not_an_empty_list() {
    let py = require_python!();
    let fx = Fixture::new();
    let env = fx.base_env();
    assert_parity_many(
        &py,
        &env,
        &[
            &["list"],
            &["doctor"],
            &["whisper-device"],
            &["paths", "--campaign", "x"],
        ],
    );
    let out = run_rust(&env, &["doctor"]);
    assert_eq!(out.code, 2);
    assert!(out.stdout.contains("NOT CONFIGURED"));
    assert!(out.stdout.contains("export GM_VAULTS_ROOT=/path/to/vaults"));
}

#[test]
fn configured_but_empty_vaults_show_the_search_location() {
    let py = require_python!();
    let fx = Fixture::new();
    fs::create_dir_all(fx.path("empty")).unwrap();
    let env = env_with(&fx, &[("GM_VAULTS_ROOT", fx.path("empty"))]);
    assert_parity_many(&py, &env, &[&["list"], &["doctor"]]);
    assert_eq!(run_rust(&env, &["doctor"]).code, 1);
    // a vault root that does not exist at all
    let env = env_with(&fx, &[("GM_VAULT", fx.path("no/such/vault"))]);
    assert_parity_many(&py, &env, &[&["list"], &["doctor"]]);
}

// -- roots and XDG ----------------------------------------------------------------

#[test]
fn xdg_defaults_and_relative_xdg_values() {
    let py = require_python!();
    let fx = Fixture::new();
    fx.install("home/v/0.2_campaigns/main/canon_aliases.json", &example_file());
    fx.write(
        "xdgcfg/ttrpg-transcriber/config.json",
        &format!(r#"{{"vault": "{}"}}"#, fx.path("home/v")),
    );
    // trailing slash kept verbatim; relative XDG_DATA_HOME ignored; ".." in XDG_CACHE_HOME
    // is normalised only after the default is appended
    let env = env_with(
        &fx,
        &[
            ("XDG_CONFIG_HOME", format!("{}/", fx.path("xdgcfg"))),
            ("XDG_DATA_HOME", "relative/data".into()),
            ("XDG_CACHE_HOME", format!("{}/a/../cache", fx.path("xdg"))),
        ],
    );
    assert_parity_many(
        &py,
        &env,
        &[
            &["list"],
            &["doctor"],
            &["paths", "--campaign", "main", "--session", "2"],
        ],
    );
}

#[test]
fn empty_environment_values_count_as_unset() {
    let py = require_python!();
    let fx = Fixture::new();
    fx.install("home/v/0.2_campaigns/main/canon_aliases.json", &example_file());
    let env = env_with(
        &fx,
        &[
            ("GM_VAULT", "".into()),
            ("VAULT", fx.path("home/v")),
            ("GM_AUDIO_ROOT", "".into()),
            ("GM_WHISPER_DEVICE", "".into()),
            ("XDG_DATA_HOME", "".into()),
        ],
    );
    assert_parity_many(
        &py,
        &env,
        &[
            &["list"],
            &["doctor"],
            &["paths", "--campaign", "main", "--session", "1"],
        ],
    );
}

#[test]
fn campaign_audio_root_override() {
    let py = require_python!();
    let fx = Fixture::new();
    build_multi_vault(&fx);
    let env = multi_env(&fx);
    // thornwake/messy sets audio_root: "~/messy_audio"
    assert_parity(&py, &env, &["paths", "--campaign", "thornwake/messy", "--session", "3"]);
}

#[test]
fn whisper_device_values() {
    let py = require_python!();
    let fx = Fixture::new();
    fs::create_dir_all(fx.path("empty")).unwrap();
    let from_env = [
        "0", "1", "007", " 2 ", "+3", "1_0", "abc", "-1", "1.5", "1__0", "_1", "-0", " ",
    ];
    for v in from_env {
        let env = env_with(
            &fx,
            &[("GM_VAULTS_ROOT", fx.path("empty")), ("GM_WHISPER_DEVICE", v.into())],
        );
        assert_parity_many(&py, &env, &[&["whisper-device"], &["doctor"]]);
    }
    let from_config = [
        "0", "2", "\"3\"", "\" 4 \"", "true", "1.5", "null", "-1", "\"\"", "\"x\"", "[1]",
    ];
    for v in from_config {
        fx.write("cfg/config.json", &format!(r#"{{"whisper_device": {v}}}"#));
        let env = env_with(
            &fx,
            &[
                ("GM_VAULTS_ROOT", fx.path("empty")),
                ("GM_TRANSCRIBER_CONFIG", fx.path("cfg/config.json")),
            ],
        );
        assert_parity_many(&py, &env, &[&["whisper-device"], &["doctor"]]);
    }
    // the environment wins over the config file
    fx.write("cfg/config.json", r#"{"whisper_device": 5}"#);
    let env = env_with(
        &fx,
        &[
            ("GM_VAULTS_ROOT", fx.path("empty")),
            ("GM_TRANSCRIBER_CONFIG", fx.path("cfg/config.json")),
            ("GM_WHISPER_DEVICE", "2".into()),
        ],
    );
    assert_parity_many(&py, &env, &[&["whisper-device"], &["doctor"]]);
}

// -- config file problems ---------------------------------------------------------

#[test]
fn invalid_config_json_is_an_error_naming_the_file() {
    let py = require_python!();
    let fx = Fixture::new();
    fx.write("cfg/config.json", "{ nope");
    let env = env_with(&fx, &[("GM_TRANSCRIBER_CONFIG", fx.path("cfg/config.json"))]);
    assert_parity_many(
        &py,
        &env,
        &[
            &["list"],
            &["doctor"],
            &["whisper-device"],
            &["paths", "--campaign", "x"],
        ],
    );
    let out = run_rust(&env, &["whisper-device"]);
    assert_eq!(out.code, 2);
    let named = format!("ERROR: {} is not valid JSON:", fx.path("cfg/config.json"));
    assert!(out.stderr.starts_with(&named), "{out:?}");
}

#[test]
fn config_is_read_only_when_needed() {
    // Every root comes from the environment, so `paths` never touches the broken
    // config. `doctor` still reads it for the whisper device, as the original did.
    let py = require_python!();
    let fx = Fixture::new();
    build_multi_vault(&fx);
    fx.write("cfg/config.json", "{ nope");
    let mut env = multi_env(&fx);
    env.insert("GM_TRANSCRIBER_CONFIG".into(), fx.path("cfg/config.json"));
    env.insert("GM_CAMPAIGNS_SUBDIR".into(), "0.2_campaigns".into());
    env.insert("GM_TRANSCRIPTS_SUBDIR".into(), "08_transcripts".into());
    assert_parity_many(
        &py,
        &env,
        &[
            &["paths", "--campaign", "emberfall/main", "--session", "1"],
            &["list"],
            &["doctor"],
        ],
    );
    assert_eq!(run_rust(&env, &["paths", "--campaign", "emberfall/main"]).code, 0);
}

#[test]
fn json_config_that_is_not_an_object_is_empty() {
    let py = require_python!();
    let fx = Fixture::new();
    fx.write("cfg/config.json", "[1, 2, 3]");
    let env = env_with(&fx, &[("GM_TRANSCRIBER_CONFIG", fx.path("cfg/config.json"))]);
    assert_parity_many(&py, &env, &[&["list"], &["doctor"]]);
}

// -- campaign data ------------------------------------------------------------------

fn rust_dump(settings: &Settings, slug: &str, sessions: &[i64]) -> Value {
    let c = Campaign::load(settings, slug).unwrap();
    let p = c.paths(settings).unwrap();
    let pairs = |m: indexmap::IndexMap<String, Vec<String>>| -> Vec<Value> {
        m.into_iter().map(|(k, v)| json!([k, v])).collect()
    };
    let mut by_session = serde_json::Map::new();
    for &n in sessions {
        let s = c.session_paths(settings, n).unwrap();
        by_session.insert(
            n.to_string(),
            json!({
                "session_dir": s.session_dir, "transcript": s.transcript_md, "chunks": s.chunks_file,
                "audio": s.audio_dir, "work": s.work_dir,
            }),
        );
    }
    json!({
        "slug": c.slug, "root": c.root, "label": c.label(), "system": c.system(), "world": c.world(),
        "aliases_path": c.aliases_path(), "transcripts_dir": p.transcripts_dir, "chunks_dir": p.chunks_dir,
        "audio_dir_base": p.audio_base, "db_path": p.db_path, "seed_prompt": c.seed_prompt(),
        "speaker_map": c.speaker_map().iter().map(|r| json!([r.key, r.player, r.character])).collect::<Vec<_>>(),
        "players": c.players().iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        "pcs": c.pcs(),
        "discord_names": c.discord_names().iter()
            .map(|(k, e)| json!([k, e.player, e.character, e.role])).collect::<Vec<_>>(),
        "names": c.names(),
        "aliases": pairs(c.aliases()),
        "review_only": pairs(c.review_only()),
        "has_review_only": c.has_review_only(),
        "sessions": Value::Object(by_session),
    })
}

fn python_dump(py: &Path, env: &HashMap<String, String>, slug: &str, sessions: &[i64]) -> Value {
    let sessions: Vec<String> = sessions.iter().map(i64::to_string).collect();
    let o = Command::new(py)
        .args(["-I", "-B"])
        .arg(manifest().join("tests/support/dump_campaign.py"))
        .arg(oracle_dir())
        .arg(slug)
        .args(&sessions)
        .env_clear()
        .envs(env)
        .output()
        .expect("run python");
    assert!(
        o.status.success(),
        "dump failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_slice(&o.stdout).unwrap()
}

#[test]
fn campaign_schema_accessors_match_python() {
    let py = require_python!();
    let fx = Fixture::new();
    build_multi_vault(&fx);
    let env = multi_env(&fx);
    let settings = Settings::from_env_map(env.clone(), CLI);
    let sessions = [1, 7, 13, 123, 0];
    for slug in [
        "emberfall/main",
        "emberfall/main-old",
        "emberfall/side",
        "thornwake/messy",
    ] {
        let want = python_dump(&py, &env, slug, &sessions);
        let got = rust_dump(&settings, slug, &sessions);
        assert_eq!(got, want, "campaign data differs for {slug}");
    }
}

#[test]
fn derived_fallbacks_are_what_the_spec_says() {
    // Not a parity check: pins the behaviour the fixtures were written to cover.
    let fx = Fixture::new();
    build_multi_vault(&fx);
    let s = Settings::from_env_map(multi_env(&fx), CLI);
    let key = |c: &Campaign| {
        c.speaker_map()
            .into_iter()
            .map(|r| (r.key, r.character))
            .collect::<Vec<_>>()
    };

    let players_only = Campaign::load(&s, "emberfall/main-old").unwrap();
    assert_eq!(players_only.pcs(), ["Mara_Quill", "Tomas_Ink", "Wern_Hale"]);
    assert_eq!(
        key(&players_only),
        [
            ("mara_quill".to_string(), Some("Mara_Quill".to_string())),
            ("pat".to_string(), Some("Mara_Quill".to_string())),
            ("tomas_ink".to_string(), Some("Tomas_Ink".to_string())),
            ("lee".to_string(), Some("Tomas_Ink".to_string())),
            ("wern_hale".to_string(), Some("Wern_Hale".to_string())),
            ("gm".to_string(), None),
            ("sky".to_string(), None),
        ]
    );

    let sparse = Campaign::load(&s, "emberfall/side").unwrap();
    assert!(sparse.pcs().is_empty());
    assert_eq!(key(&sparse), [("gm".to_string(), None), ("sky".to_string(), None)]);

    let messy = Campaign::load(&s, "thornwake/messy").unwrap();
    assert_eq!(messy.label(), "Messy Display");
    assert_eq!(
        messy.players().into_iter().collect::<Vec<_>>(),
        [("A_B".to_string(), Some("second".to_string())), ("C".to_string(), None),]
    );
    assert!(
        messy.has_review_only(),
        "an entry with an empty variant list still counts"
    );
}

// -- path helpers -----------------------------------------------------------------

#[test]
fn expand_and_abspath_match_python() {
    let py = require_python!();
    let fx = Fixture::new();
    let inputs = [
        "~",
        "~/a",
        "~/a/../b",
        "~/",
        "~//x",
        "~root",
        "~root/x",
        "~definitely_no_such_user_zz/x",
        "a/b",
        "./a",
        "../a",
        "../../..",
        "//a//b",
        "///a",
        "////",
        "/a/./b/",
        "/a/../../b",
        "",
        ".",
        "..",
        "/",
        "a/~",
        "~x~",
        "/a//b/../c",
    ];
    for home in ["/home/a", "/home/a/", "/home/a//", ""] {
        let mut env = fx.base_env();
        env.insert("HOME".into(), home.into());
        let o = Command::new(&py)
            .args(["-I", "-B"])
            .arg(manifest().join("tests/support/expand_paths.py"))
            .arg(serde_json::to_string(&inputs).unwrap())
            .env_clear()
            .envs(&env)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let want: Vec<String> = serde_json::from_slice(&o.stdout).unwrap();
        let got: Vec<String> = inputs
            .iter()
            .map(|i| ttrpg_core::pyish::expand_abs(i, env.get("HOME").map(String::as_str)))
            .collect();
        for ((input, got), want) in inputs.iter().zip(&got).zip(&want) {
            assert_eq!(got, want, "expand_abs({input:?}) with HOME={home:?}");
        }
    }
}
