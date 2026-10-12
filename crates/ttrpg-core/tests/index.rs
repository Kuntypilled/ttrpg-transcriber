//! The index job and the alias report: the real `legacy/transcript_to_chunks.py` (and
//! `campaign.py` behind it) on an invented campaign, with a stand-in for the Python
//! that has ChromaDB. They need python3; without it each test prints SKIPPED and passes.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tempfile::TempDir;
use ttrpg_core::{Campaign, IndexKind, IndexReport, PipelineEvent, Settings, execute_index, legacy, plan_index};

/// Runs the real Python for the chunker and pretends to be ChromaDB for the rest.
const FAKE_PYTHON: &str = r#"#!/usr/bin/env bash
case "$1" in
  *chroma_memory.py)
    [[ -n "${FAKE_SLEEP:-}" ]] && sleep "$FAKE_SLEEP"
    if [[ -n "${FAKE_NO_CHROMA:-}" ]]; then
      echo "Traceback (most recent call last):" >&2
      echo "ModuleNotFoundError: No module named 'chromadb'" >&2
      exit 1
    fi
    echo "Upserted 2 chunks into /fake/db. Collection now holds 5."
    ;;
  *) exec python3 "$@" ;;
esac
"#;

const TRANSCRIPT: &str = "**[00:00:04] GM:**  \nThe Cinder Road runs out at the gates of Marrow Hall.\n\n\
**[00:00:20] Mara_Quill:**  \nWe go in slowly. Bram, keep your hand off that sword.\n\n\
**[00:01:10] Tomas_Ink:**  \nI am not touching the sword.\n\n";

fn python_present() -> bool {
    Command::new("sh")
        .args(["-c", "command -v python3"])
        .output()
        .is_ok_and(|o| o.status.success())
}

macro_rules! require_python {
    () => {
        if !python_present() {
            eprintln!("SKIPPED: python3 is needed");
            return;
        }
    };
}

struct Fixture {
    tmp: TempDir,
    env: HashMap<String, String>,
}

impl Fixture {
    fn new(extra: &[(&str, &str)], transcript: Option<&str>) -> Self {
        let tmp = TempDir::new().unwrap();
        let p = |rel: &str| tmp.path().join(rel).to_string_lossy().into_owned();
        let camp = tmp.path().join("vault/0.2_campaigns/demo");
        fs::create_dir_all(&camp).unwrap();
        fs::write(
            camp.join("canon_aliases.json"),
            r#"{"campaign": "Demo", "system": "daggerheart",
                "names": ["Marrow Hall"],
                "aliases": {"Marrow Hall": ["Marro Hall"]},
                "players": {"Mara Quill": "Pat", "Tomas Ink": "Lee"},
                "speaker_map": [
                    {"match": "gm", "player": "Sky", "character": null},
                    {"match": "mara", "player": "Pat", "character": "Mara_Quill"},
                    {"match": "tomas", "player": "Lee", "character": "Tomas_Ink"}]}"#,
        )
        .unwrap();
        if let Some(t) = transcript {
            let dir = camp.join("08_transcripts/S01");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("transcript.md"), t).unwrap();
        }
        fs::create_dir_all(tmp.path().join("bin")).unwrap();
        let fake = tmp.path().join("bin/fake-python");
        fs::write(&fake, FAKE_PYTHON).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let mut env = HashMap::from([
            ("HOME".to_string(), p("home")),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("LC_ALL".to_string(), "C.UTF-8".to_string()),
            ("GM_VAULT".to_string(), p("vault")),
            ("GM_AUDIO_ROOT".to_string(), p("audio")),
            ("GM_CHROMA_ROOT".to_string(), p("chroma")),
            ("GM_WORK_ROOT".to_string(), p("work")),
            ("GM_TRANSCRIBER_CONFIG".to_string(), p("none.json")),
            ("GM_PYTHON".to_string(), fake.to_string_lossy().into_owned()),
        ]);
        env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Fixture { tmp, env }
    }

    fn settings(&self) -> Settings {
        Settings::from_env_map(self.env.clone(), "ttrpg-tui")
    }

    fn run(&self, kind: IndexKind) -> (Vec<PipelineEvent>, IndexReport) {
        self.run_with(kind, |_, _| {})
    }

    fn run_with(
        &self,
        kind: IndexKind,
        mut hook: impl FnMut(&PipelineEvent, &AtomicBool),
    ) -> (Vec<PipelineEvent>, IndexReport) {
        let s = self.settings();
        let c = Campaign::load(&s, "demo").unwrap();
        let scripts = legacy::locate(&s).unwrap();
        let plan = plan_index(&s, &c, 1, kind, &scripts).unwrap();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = Arc::clone(&cancel);
        let handle = thread::spawn(move || execute_index(plan, tx, c2));
        let mut events = Vec::new();
        let report = loop {
            let ev = rx.recv_timeout(Duration::from_secs(60)).expect("the index job stalled");
            hook(&ev, &cancel);
            if let PipelineEvent::IndexFinished(r) = &ev {
                let r = (**r).clone();
                events.push(ev);
                break r;
            }
            events.push(ev);
        };
        handle.join().unwrap();
        (events, report)
    }

    fn chunks_file(&self) -> PathBuf {
        self.tmp
            .path()
            .join("vault/0.2_campaigns/demo/08_transcripts/chunks/s01_chunks.jsonl")
    }
}

fn stages(events: &[PipelineEvent]) -> Vec<(String, f64)> {
    events
        .iter()
        .filter_map(|e| match e {
            PipelineEvent::Stage { label, fraction } => Some((label.clone(), *fraction)),
            _ => None,
        })
        .collect()
}

fn logs(events: &[PipelineEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            PipelineEvent::Log(l) => Some(l.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn indexing_chunks_then_stores_and_reports_what_the_helpers_said() {
    require_python!();
    let fx = Fixture::new(&[], Some(TRANSCRIPT));
    let (events, report) = fx.run(IndexKind::Index);
    assert!(report.ok(), "{}", report.headline());
    assert_eq!(
        stages(&events).iter().map(|(_, f)| *f).collect::<Vec<_>>(),
        [0.1, 0.5, 1.0],
        "the bar goes 0.1 chunking, 0.5 storing, 1.0 done"
    );
    assert_eq!((report.chunks, report.utterances), (Some(1), Some(3)));
    assert_eq!((report.upserted, report.collection), (Some(2), Some(5)));
    assert_eq!(
        report.headline(),
        "Indexed 2 chunks of session 1; the store now holds 5."
    );
    let jsonl = fs::read_to_string(fx.chunks_file()).unwrap();
    assert_eq!(jsonl.lines().count(), 1);
    assert!(jsonl.contains("\"id\": \"s01_c000\""), "{jsonl}");
    assert!(
        logs(&events).contains("Upserted 2 chunks"),
        "the helper's words reach the log"
    );
    assert_eq!(report.output.as_deref(), Some(fx.chunks_file().to_str().unwrap()));
}

#[test]
fn the_alias_report_is_written_and_nothing_else_changes() {
    require_python!();
    let fx = Fixture::new(&[], Some(TRANSCRIPT));
    let aliases = fx.tmp.path().join("vault/0.2_campaigns/demo/canon_aliases.json");
    let before = fs::read_to_string(&aliases).unwrap();
    let (_, report) = fx.run(IndexKind::Report);
    assert!(report.ok(), "{}", report.headline());
    let out = fx
        .tmp
        .path()
        .join("vault/0.2_campaigns/demo/08_transcripts/S01/alias_candidates.md");
    assert!(
        fs::read_to_string(&out)
            .unwrap()
            .starts_with("# Alias candidates: Demo session 1")
    );
    assert_eq!(report.output.as_deref(), Some(out.to_str().unwrap()));
    assert_eq!(
        fs::read_to_string(&aliases).unwrap(),
        before,
        "canon_aliases.json untouched"
    );
    assert!(!fx.chunks_file().exists(), "a report does not chunk");
}

#[test]
fn a_missing_chromadb_says_how_to_install_it_and_keeps_the_chunk_file() {
    require_python!();
    let fx = Fixture::new(&[("FAKE_NO_CHROMA", "1")], Some(TRANSCRIPT));
    let (events, report) = fx.run(IndexKind::Index);
    assert!(!report.ok());
    let why = report.failure.as_deref().unwrap();
    assert!(why.starts_with("Storing in ChromaDB failed (exit code 1)."), "{why}");
    assert!(
        why.contains("ChromaDB is not installed for") && why.contains("-m pip install -r"),
        "{why}"
    );
    assert!(why.contains("requirements.txt") && why.contains("\"python\""), "{why}");
    assert!(fx.chunks_file().is_file(), "the text survives; only the store failed");
    assert_eq!(stages(&events).len(), 2, "never reached done");
    assert!(logs(&events).contains("ModuleNotFoundError"));
}

#[test]
fn a_python_that_cannot_start_is_a_clear_failure() {
    require_python!();
    let fx = Fixture::new(&[("GM_PYTHON", "/nonexistent/python")], Some(TRANSCRIPT));
    let (_, report) = fx.run(IndexKind::Index);
    let why = report.failure.unwrap();
    assert!(
        why.contains("Could not start /nonexistent/python") && why.contains("\"python\""),
        "{why}"
    );
}

#[test]
fn a_transcript_with_no_speaker_lines_is_refused_before_storing() {
    require_python!();
    let fx = Fixture::new(&[], Some("just some notes, no speaker lines\n"));
    let (events, report) = fx.run(IndexKind::Index);
    let why = report.failure.unwrap();
    assert!(why.contains("chunk file is empty"), "{why}");
    assert_eq!(stages(&events).len(), 1, "stopped after chunking");
}

#[test]
fn planning_needs_a_transcript() {
    require_python!();
    let fx = Fixture::new(&[], None);
    let s = fx.settings();
    let c = Campaign::load(&s, "demo").unwrap();
    let err = plan_index(&s, &c, 1, IndexKind::Index, &legacy::locate(&s).unwrap()).unwrap_err();
    assert!(err.0.contains("no transcript for session 1"), "{err}");
    let err = plan_index(&s, &c, 0, IndexKind::Index, &legacy::locate(&s).unwrap()).unwrap_err();
    assert!(err.0.contains("1 or higher"));
}

#[test]
fn cancel_stops_the_helper_and_nothing_is_reported_as_stored() {
    require_python!();
    let fx = Fixture::new(&[("FAKE_SLEEP", "30")], Some(TRANSCRIPT));
    let start = std::time::Instant::now();
    let (_, report) = fx.run_with(IndexKind::Index, |ev, cancel| {
        if matches!(ev, PipelineEvent::Stage { fraction, .. } if (*fraction - 0.5).abs() < 1e-9) {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert!(report.cancelled && !report.ok());
    assert!(report.headline().contains("cancelled") && report.upserted.is_none());
    assert!(start.elapsed() < Duration::from_secs(15), "did not wait out the sleep");
}
