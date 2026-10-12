//! The compiled binary: argument parsing, exit codes and stream routing, and its
//! plain-text commands against the pinned Python original.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_ttrpg-tui");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

struct Vault {
    tmp: TempDir,
}

impl Vault {
    /// Two campaigns in one world, from the invented example and a fixture.
    fn new() -> Self {
        let v = Vault {
            tmp: TempDir::new().unwrap(),
        };
        let root = repo_root();
        for (rel, src) in [
            (
                "vaults/emberfall/0.2_campaigns/main/canon_aliases.json",
                "examples/canon_aliases.example.json",
            ),
            (
                "vaults/emberfall/0.2_campaigns/side/canon_aliases.json",
                "crates/ttrpg-core/tests/fixtures/sparse.json",
            ),
        ] {
            let dest = v.tmp.path().join(rel);
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::copy(root.join(src), dest).unwrap();
        }
        fs::create_dir_all(v.tmp.path().join("home")).unwrap();
        v
    }

    fn env(&self) -> HashMap<String, String> {
        let p = |rel: &str| self.tmp.path().join(rel).to_string_lossy().into_owned();
        HashMap::from([
            ("HOME".into(), p("home")),
            ("LC_ALL".into(), "C.UTF-8".into()),
            ("GM_VAULTS_ROOT".into(), p("vaults")),
            ("GM_AUDIO_ROOT".into(), p("audio")),
            ("GM_CHROMA_ROOT".into(), p("chroma")),
            ("GM_WORK_ROOT".into(), p("work")),
        ])
    }
}

fn run(env: &HashMap<String, String>, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).unwrap()
}

#[test]
fn list_prints_one_tab_separated_line_per_campaign() {
    let v = Vault::new();
    let out = run(&v.env(), &["list"]);
    assert!(out.status.success());
    assert_eq!(
        text(&out.stdout),
        "emberfall/main\tEmberfall\tdaggerheart\nemberfall/side\tTidewatch\tdnd-5e\n"
    );
}

#[test]
fn user_errors_go_to_stderr_with_exit_code_2() {
    let v = Vault::new();
    let out = run(&v.env(), &["paths", "--campaign", "emberfall/../x"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(
        text(&out.stderr).starts_with("ERROR: 'emberfall/../x' is not a campaign id"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn doctor_exit_codes() {
    let v = Vault::new();
    assert_eq!(run(&v.env(), &["doctor"]).status.code(), Some(0));

    let mut empty = v.env();
    empty.insert(
        "GM_VAULTS_ROOT".into(),
        v.tmp.path().join("home").to_string_lossy().into_owned(),
    );
    assert_eq!(
        run(&empty, &["doctor"]).status.code(),
        Some(1),
        "configured, no campaigns"
    );

    let bare = HashMap::from([(
        "HOME".to_string(),
        v.tmp.path().join("home").to_string_lossy().into_owned(),
    )]);
    let out = run(&bare, &["doctor"]);
    assert_eq!(out.status.code(), Some(2), "not configured");
    assert!(text(&out.stdout).contains("NOT CONFIGURED"));
    assert!(text(&out.stdout).contains("Then run:  ttrpg-tui doctor"));
}

#[test]
fn the_tui_refuses_to_start_without_a_terminal() {
    let v = Vault::new();
    let out = run(&v.env(), &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("needs an interactive terminal"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn negative_and_zero_sessions_parse() {
    let v = Vault::new();
    for s in ["-5", "0", "123"] {
        let out = run(&v.env(), &["paths", "--campaign", "emberfall/main", "--session", s]);
        assert!(out.status.success(), "--session {s}: {}", text(&out.stderr));
    }
    let out = run(&v.env(), &["paths", "--campaign", "emberfall/main", "--session", "abc"]);
    assert!(!out.status.success());
}

/// Same command through `tests/oracle/campaign.py` and the binary: identical stdout.
#[test]
fn plain_commands_match_the_python_original() {
    let Some(py) = std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("python3"))
            .find(|c| c.is_file())
    }) else {
        eprintln!("SKIPPED: python3 not found");
        return;
    };
    let v = Vault::new();
    let env = v.env();
    let legacy = repo_root().join("crates/ttrpg-core/tests/oracle/campaign.py");
    for args in [
        &["list"][..],
        &["paths", "--campaign", "emberfall/main"],
        &["paths", "--campaign", "emberfall/side", "--session", "7"],
        &["paths", "--campaign", "emberfall/main", "--session", "-5"],
        &["whisper-device"],
    ] {
        let want = Command::new(&py)
            .args(["-I", "-B"])
            .arg(&legacy)
            .args(args)
            .env_clear()
            .envs(&env)
            .output()
            .unwrap();
        let got = run(&env, args);
        assert_eq!(text(&got.stdout), text(&want.stdout), "stdout for {args:?}");
        assert_eq!(got.status.code(), want.status.code(), "exit code for {args:?}");
    }
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
            out.push(e.path());
            if e.path().is_dir() {
                stack.push(e.path());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn check_names_what_is_missing_exits_1_and_creates_nothing() {
    let v = Vault::new();
    let mut env = v.env();
    env.insert(
        "PATH".into(),
        v.tmp.path().join("empty-bin").to_string_lossy().into_owned(),
    );
    let before = files_under(v.tmp.path());
    let out = run(&env, &["check"]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = text(&out.stdout);
    for want in [
        "[MISSING] ffmpeg",
        "[MISSING] whisper-cli",
        "[MISSING] whisper model",
        "Missing: ffmpeg, whisper-cli, whisper model",
    ] {
        assert!(stdout.contains(want), "missing {want:?} in:\n{stdout}");
    }
    for gone in ["bash", "jq", "python3"] {
        assert!(
            !stdout.contains(&format!("[MISSING] {gone}")),
            "{gone} is not needed:\n{stdout}"
        );
    }
    assert!(stdout.contains("[optional] ChromaDB for Index"), "{stdout}");
    assert_eq!(
        files_under(v.tmp.path()),
        before,
        "unlike the old transcribe.sh --check, this creates no files"
    );
}

#[test]
fn check_passes_when_the_tools_are_there() {
    use std::os::unix::fs::PermissionsExt;
    let v = Vault::new();
    let bin = v.tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    for t in ["ffmpeg", "whisper-cli"] {
        fs::write(bin.join(t), "#!/bin/sh\n").unwrap();
        fs::set_permissions(bin.join(t), fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(v.tmp.path().join("model.bin"), "x").unwrap();
    let mut env = v.env();
    env.insert("PATH".into(), bin.to_string_lossy().into_owned());
    env.insert(
        "GM_WHISPER_MODEL".into(),
        v.tmp.path().join("model.bin").to_string_lossy().into_owned(),
    );
    let out = run(&env, &["check"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    assert!(text(&out.stdout).contains("[OK] whisper-cli"));
    assert!(!text(&out.stdout).contains("Missing:"));
}

fn python_present() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A store holding one chunk from session 4, made by the real helper against the stand-in chromadb.
fn memory_env(v: &Vault) -> HashMap<String, String> {
    let mut env = v.env();
    env.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
    env.insert(
        "PYTHONPATH".into(),
        repo_root()
            .join("crates/ttrpg-core/tests/fixtures/fake_chroma")
            .to_string_lossy()
            .into_owned(),
    );
    env
}

fn seed_store(v: &Vault, env: &HashMap<String, String>) {
    let jsonl = v.tmp.path().join("chunks.jsonl");
    fs::write(
        &jsonl,
        r#"{"id": "s04_c000", "document": "Vashti_Corrow: we take the dark road.", "metadata": {"session": 4, "start": "00:01:00", "end": "00:02:00", "characters": "Vashti_Corrow", "has_Vashti_Corrow": true}}"#.to_owned() + "\n",
    )
    .unwrap();
    let db = v.tmp.path().join("chroma/emberfall/main");
    let out = Command::new("python3")
        .arg(repo_root().join("python/chroma_memory.py"))
        .args(["ingest", "--db"])
        .arg(&db)
        .arg(&jsonl)
        .env_clear()
        .envs(env)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
}

#[test]
fn info_names_the_campaign_and_shows_what_is_in_its_store() {
    if !python_present() {
        eprintln!("SKIPPED: python3 not found");
        return;
    }
    let v = Vault::new();
    let env = memory_env(&v);
    let out = run(&env, &["info", "emberfall/main"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let before = text(&out.stdout);
    assert!(
        before.starts_with("campaign   : emberfall/main (Emberfall, daggerheart)\n"),
        "{before}"
    );
    assert!(
        before.contains("exists     : False") && before.contains("chunks     : 0 (no store yet)"),
        "{before}"
    );
    assert!(before.contains("pcs        : Vashti_Corrow, Bram_Ollander"), "{before}");

    seed_store(&v, &env);
    let after = text(&run(&env, &["info", "emberfall/main"]).stdout);
    assert!(
        after.contains("chunks     : 1") && after.contains("sessions   : 4"),
        "{after}"
    );
    assert!(
        after.contains(&format!(
            "db path    : {}",
            v.tmp.path().join("chroma/emberfall/main").display()
        )),
        "{after}"
    );
}

#[test]
fn query_passes_the_question_and_the_filters_to_the_store() {
    if !python_present() {
        eprintln!("SKIPPED: python3 not found");
        return;
    }
    let v = Vault::new();
    let env = memory_env(&v);
    seed_store(&v, &env);
    let out = run(
        &env,
        &[
            "query",
            "emberfall/main",
            "--from",
            "3",
            "--char",
            "Vashti Corrow",
            "-k",
            "2",
            "--",
            "the dark road",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("=== s4 00:01:00-00:02:00 [Vashti_Corrow] cos_dist="),
        "{stdout}"
    );
    assert!(stdout.contains("Vashti_Corrow: we take the dark road."), "{stdout}");

    let none = text(&run(&env, &["query", "emberfall/main", "road", "--contains", "dragon"]).stdout);
    assert!(none.starts_with("No results in "), "{none}");
}

#[test]
fn info_and_query_say_so_for_an_unknown_campaign_or_a_missing_python() {
    let v = Vault::new();
    let out = run(&v.env(), &["info", "emberfall/nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).starts_with("ERROR: "), "{}", text(&out.stderr));

    let mut env = v.env();
    env.insert("GM_PYTHON".into(), "/no/such/python".into());
    let out = run(&env, &["query", "emberfall/main", "anything"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("Could not start /no/such/python"),
        "{}",
        text(&out.stderr)
    );
}
