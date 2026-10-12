//! End-to-end tests of the transcription job: the Rust runner, real `ffmpeg`, and a stub
//! `whisper-cli` (`fixtures/run/stub-whisper`) that writes canned JSON, on tone files with
//! invented names. They need bash (for the stub), ffmpeg and pgrep; without them each test
//! prints SKIPPED and passes.
//!
//! `fixtures/run/expected/` was written by the original `transcribe.sh` on the same
//! inputs (see `fixtures/run/generate.sh`), so the full-run test is a byte comparison
//! with the script.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use ttrpg_core::job::{PipelineEvent, RunReport, execute, plan_job, plan_reprocess};
use ttrpg_core::progress::{Phase, ProgressEvent, Tracker};
use ttrpg_core::state::read_state_rows;
use ttrpg_core::{Campaign, Settings, StockRule, resolve_speaker};

const RUN_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/run");

fn tools_present() -> bool {
    ["bash", "ffmpeg", "pgrep"].iter().all(|t| {
        Command::new("sh")
            .args(["-c", &format!("command -v {t}")])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

macro_rules! require_tools {
    () => {
        if !tools_present() {
            eprintln!("SKIPPED: bash, ffmpeg and pgrep are needed");
            return;
        }
    };
}

struct Fixture {
    tmp: TempDir,
    env: HashMap<String, String>,
}

impl Fixture {
    fn new(extra_env: &[(&str, &str)]) -> Self {
        let tmp = TempDir::new().unwrap();
        let p = |rel: &str| tmp.path().join(rel).to_string_lossy().into_owned();
        let camp = tmp.path().join("vault/0.2_campaigns/demo");
        fs::create_dir_all(&camp).unwrap();
        fs::copy(
            format!("{RUN_FIXTURE}/canon_aliases.json"),
            camp.join("canon_aliases.json"),
        )
        .unwrap();
        fs::write(tmp.path().join("model.bin"), "x").unwrap();
        fs::create_dir_all(tmp.path().join("home")).unwrap();
        fs::create_dir_all(tmp.path().join("downloads")).unwrap();

        let mut env = HashMap::from([
            ("HOME".to_string(), p("home")),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("LC_ALL".to_string(), "C.UTF-8".to_string()),
            ("GM_VAULT".to_string(), p("vault")),
            ("GM_AUDIO_ROOT".to_string(), p("audio")),
            ("GM_CHROMA_ROOT".to_string(), p("chroma")),
            ("GM_WORK_ROOT".to_string(), p("work")),
            ("GM_TRANSCRIBER_CONFIG".to_string(), p("none.json")),
            ("GM_WHISPER_BIN".to_string(), format!("{RUN_FIXTURE}/stub-whisper")),
            ("GM_WHISPER_MODEL".to_string(), p("model.bin")),
        ]);
        env.extend(extra_env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Fixture { tmp, env }
    }

    fn root(&self) -> String {
        self.tmp.path().to_string_lossy().into_owned()
    }

    fn settings(&self) -> Settings {
        Settings::from_env_map(self.env.clone(), "ttrpg-tui")
    }

    fn campaign(&self) -> Campaign {
        Campaign::load(&self.settings(), "demo").unwrap()
    }

    /// A one-second tone with the given file name in the downloads folder.
    fn tone(&self, name: &str) -> PathBuf {
        let path = self.tmp.path().join("downloads").join(name);
        let ok = Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
            ])
            .args(["-ar", "16000", "-ac", "1"])
            .arg(&path)
            .status()
            .unwrap()
            .success();
        assert!(ok, "ffmpeg could not make {name}");
        path
    }

    fn five_tracks(&self) -> Vec<PathBuf> {
        [
            "1-gmperson.flac",
            "2-riverone.flac",
            "3-rivertwo.flac",
            "4-plain_0.flac",
            "5-newperson.flac",
        ]
        .iter()
        .map(|n| self.tone(n))
        .collect()
    }

    fn plan(&self, session: i64, files: &[PathBuf]) -> ttrpg_core::JobPlan {
        plan_job(&self.settings(), &self.campaign(), session, files).unwrap()
    }
}

/// Run a job on a thread and collect its events, letting `hook` react to each
/// (for example to cancel).
fn run_job(
    plan: ttrpg_core::JobPlan,
    mut hook: impl FnMut(&PipelineEvent, &AtomicBool),
) -> (Vec<PipelineEvent>, RunReport) {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = Arc::clone(&cancel);
    let handle = thread::spawn(move || execute(plan, tx, c2));
    let mut events = Vec::new();
    let report = loop {
        let ev = rx.recv_timeout(Duration::from_secs(90)).expect("the job stalled");
        hook(&ev, &cancel);
        if let PipelineEvent::Finished(r) = &ev {
            let r = (**r).clone();
            events.push(ev);
            break r;
        }
        events.push(ev);
    };
    handle.join().unwrap();
    (events, report)
}

fn log_text(events: &[PipelineEvent]) -> String {
    events
        .iter()
        .filter_map(|e| {
            if let PipelineEvent::Log(l) = e {
                Some(l.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn transcript_labels(path: &str) -> BTreeSet<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("**["))
        .filter_map(|l| {
            l.split_once("] ")
                .and_then(|(_, rest)| rest.split_once(":**"))
                .map(|(label, _)| label.to_string())
        })
        .collect()
}

/// The log CSV the way `generate.sh` normalises the script's: no timestamps, this
/// machine's paths and the wall-clock numbers replaced. Lines that do not start with a
/// timestamp (raw ffmpeg and whisper stderr) are kept as they are.
fn normalise_log(csv: &str, root: &str) -> String {
    let stamp = |l: &str| {
        let b = l.as_bytes();
        b.len() > 20 && b[4] == b'-' && b[7] == b'-' && b[10] == b' ' && b[13] == b':' && b[16] == b':' && b[19] == b','
    };
    let mut out = String::new();
    for line in csv.lines() {
        let line = if stamp(line) { &line[20..] } else { line };
        let mut line = line.replace(root, "<root>");
        if let (Some(open), true) = (line.rfind(" ("), line.ends_with(" MB)")) {
            let head = line[..open].to_string();
            line = format!("{head} (N MB)");
        }
        if let Some(at) = line.rfind(" in ")
            && line.ends_with('s')
            && line[at + 4..line.len() - 1].chars().all(|c| c.is_ascii_digit())
        {
            line = format!("{} in Ns", &line[..at]);
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn state_summary(path: &Path) -> String {
    read_state_rows(&path.to_string_lossy())
        .iter()
        .map(|r| format!("{}|{}|{}|{}\n", r.file, r.status, r.player, r.error))
        .collect()
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref()).unwrap_or_else(|e| panic!("{}: {e}", path.as_ref().display()))
}

#[test]
fn a_full_run_matches_what_the_script_wrote() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let files = fx.five_tracks();
    let plan = fx.plan(7, &files);
    assert_eq!(plan.track_count(), 5);
    assert!(plan.overwritten().is_empty());
    let (audio_dir, session_dir, transcript) = (
        plan.audio_dir.clone(),
        plan.session_dir.clone(),
        plan.transcript_md.clone(),
    );
    let temp_dir = plan.run.temp_dir.clone();

    let (events, report) = run_job(plan, |_, _| {});

    // The files moved: gone from downloads, present in the session audio folder.
    for f in &files {
        assert!(!f.exists(), "{} should have been moved", f.display());
        assert!(Path::new(&audio_dir).join(f.file_name().unwrap()).is_file());
    }
    assert!(audio_dir.ends_with("/Session 7"), "{audio_dir}");

    // Typed events, in order, ending with Finished. Each track reports how far through it
    // whisper is, from the segment lines the stub prints.
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            PipelineEvent::Moving { .. } => "move",
            PipelineEvent::Progress(ProgressEvent::Start { .. }) => "start",
            PipelineEvent::Progress(ProgressEvent::FileStart { .. }) => "file",
            PipelineEvent::Progress(ProgressEvent::InFile { .. }) => "infile",
            PipelineEvent::Progress(ProgressEvent::FileDone { .. }) => "done",
            PipelineEvent::Progress(ProgressEvent::TranscribeComplete) => "complete",
            PipelineEvent::Progress(ProgressEvent::PostDone) => "postdone",
            PipelineEvent::Log(_) => "log",
            PipelineEvent::Finished(_) => "finished",
            PipelineEvent::Stage { .. } | PipelineEvent::IndexFinished(_) => "index",
        })
        .collect();
    let compact: Vec<&str> = kinds.iter().copied().filter(|k| *k != "log").collect();
    let mut want = vec!["move"; 5];
    want.push("start");
    for _ in 0..5 {
        want.extend(["file", "infile", "infile", "done"]);
    }
    want.extend(["complete", "postdone", "finished"]);
    assert_eq!(compact, want);
    assert!(kinds.contains(&"log"), "whisper's lines reach the log");

    // The bar never goes backwards and ends full.
    let mut tracker = Tracker::default();
    let mut last = 0.0;
    for e in &events {
        if let PipelineEvent::Progress(p) = e {
            tracker.apply(p);
            assert!(tracker.fraction() >= last, "{p:?} moved the bar back");
            last = tracker.fraction();
        }
    }
    assert_eq!((tracker.phase, tracker.fraction()), (Phase::Finished, 1.0));

    // The report.
    assert_eq!(report.succeeded.len(), 5, "{report:?}");
    assert!(report.failed.is_empty() && report.skipped.is_empty());
    assert_eq!((report.moved, report.tracks_found), (5, Some(5)));
    assert!(report.ok());
    assert_eq!(report.headline(), "5 of 5 transcribed");
    assert_eq!(report.transcript_md.as_deref(), Some(transcript.as_str()));

    // Byte for byte what transcribe.sh wrote on the same inputs.
    let want_dir = format!("{RUN_FIXTURE}/expected");
    for file in ["merged_transcript.tsv", "consolidated_transcript.tsv", "transcript.md"] {
        assert_eq!(
            read(format!("{session_dir}/{file}")),
            read(format!("{want_dir}/{file}")),
            "{file}"
        );
    }
    let mut processed: Vec<String> = fs::read_dir(&temp_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".processed.tsv"))
        .collect();
    processed.sort();
    assert_eq!(processed.len(), 5);
    for name in &processed {
        assert_eq!(
            read(format!("{temp_dir}/{name}")),
            read(format!("{want_dir}/processed/{name}")),
            "{name}"
        );
    }
    assert_eq!(
        state_summary(&Path::new(&session_dir).join("transcription_state.csv")),
        read(format!("{want_dir}/state.txt"))
    );

    // The state file is the script's format: header, then every field quoted.
    let state_raw = read(format!("{session_dir}/transcription_state.csv"));
    let mut state_lines = state_raw.lines();
    assert_eq!(
        state_lines.next(),
        Some("FileName,FileSize,ProcessingTime,Status,Timestamp,PlayerName,ErrorMessage")
    );
    for line in state_lines {
        assert!(
            line.starts_with('"') && line.ends_with('"') && line.matches("\",\"").count() == 6,
            "{line}"
        );
    }

    // The log differs from the script's in two intended ways: no "Speaker file" lines (the
    // per-speaker files are gone) and one "Whisper" line saying which build ran.
    let ours = normalise_log(&read(format!("{session_dir}/transcription_log.csv")), &fx.root());
    let theirs: String = read(format!("{want_dir}/log.txt"))
        .lines()
        .filter(|l| !l.starts_with("INFO,Speaker file:"))
        .map(|l| format!("{l}\n"))
        .collect();
    let ours_without_whisper: String = ours
        .lines()
        .filter(|l| !l.starts_with("INFO,Whisper  :"))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(ours_without_whisper, theirs);
    assert!(ours.contains("INFO,Whisper  : "), "{ours}");

    // The archive holds each track's raw whisper output.
    for stem in ["1-gmperson", "2-riverone", "3-rivertwo", "4-plain_0", "5-newperson"] {
        let archived = read(format!("{session_dir}/archive/{stem}.whisper_original.json"));
        assert!(archived.contains("This is track"), "{stem}");
    }
    // No converted audio is left behind.
    assert!(
        !fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().ends_with(".wav"))
    );

    // One speaker-resolution function: the labels the transcript uses are the Rust ones.
    let camp = fx.campaign();
    let labels: BTreeSet<String> = files
        .iter()
        .map(|f| resolve_speaker(&camp, &f.file_name().unwrap().to_string_lossy()).label())
        .collect();
    assert_eq!(
        labels,
        BTreeSet::from(["GM", "Mara_Quill", "Tomas_Ink", "plain", "5-newperson"].map(String::from))
    );
    assert_eq!(transcript_labels(&transcript), labels);
}

#[test]
fn the_run_streams_whispers_lines_and_its_own() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let (events, report) = run_job(fx.plan(1, &[fx.tone("1-gmperson.flac")]), |_, _| {});
    assert!(report.ok());
    let log = log_text(&events);
    for needle in [
        "Session folder:",
        "Moved: 1-gmperson.flac",
        "Session Transcription — demo",
        "  Transcribing: 1-gmperson.flac — speaker: GM (0 MB)",
        "[00:00:00.000 --> 00:00:00.900]   you",
        "  Done: 1-gmperson.flac in",
        "  Created transcript.md",
        "Transcription Summary",
        "  Success         : 1",
    ] {
        assert!(log.contains(needle), "missing {needle:?} in:\n{log}");
    }
}

#[test]
fn a_failing_track_is_named_not_hidden() {
    require_tools!();
    let fx = Fixture::new(&[("STUB_FAIL_MATCH", "rivertwo")]);
    let files = fx.five_tracks();
    let plan = fx.plan(2, &files);
    let session_dir = plan.session_dir.clone();
    let (events, report) = run_job(plan, |_, _| {});
    assert_eq!(report.succeeded.len(), 4, "{report:?}");
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, "3-rivertwo.flac");
    assert_eq!(report.failed[0].1, "whisper-cli exited non-zero");
    assert!(!report.ok());
    assert_eq!(report.headline(), "4 of 5 transcribed, 1 failed");
    assert!(
        report.transcript_md.is_some(),
        "the other four still produce a transcript"
    );
    assert!(log_text(&events).contains("ERROR: whisper-cli failed on: 3-rivertwo.flac"));

    // The state file has the error row, and whisper's own words are in the log file.
    let rows = read_state_rows(&format!("{session_dir}/transcription_state.csv"));
    let bad: Vec<_> = rows.iter().filter(|r| r.status == "Error").collect();
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0].error, "whisper-cli exited non-zero");
    assert!(read(format!("{session_dir}/transcription_log.csv")).contains("stub: refusing"));
}

#[test]
fn rerunning_with_no_new_files_skips_finished_tracks() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let first = fx.plan(3, &fx.five_tracks());
    let (_, r1) = run_job(first, |_, _| {});
    assert!(r1.ok());

    // Nothing chosen: the five tracks already in the session folder are the job.
    let again = fx.plan(3, &[]);
    assert_eq!((again.existing.len(), again.moves.len()), (5, 0));
    let session_dir = again.session_dir.clone();
    let (events, r2) = run_job(again, |_, _| {});
    assert_eq!(
        (r2.succeeded.len(), r2.skipped.len(), r2.failed.len()),
        (0, 5, 0),
        "{r2:?}"
    );
    assert_eq!(r2.headline(), "0 of 5 transcribed, 5 already done");
    assert!(r2.ok());
    assert!(log_text(&events).contains("Skipping (already processed): 1-gmperson.flac"));
    assert_eq!(
        read_state_rows(&format!("{session_dir}/transcription_state.csv")).len(),
        5,
        "a skipped track adds no row"
    );
}

#[test]
fn transcribe_all_again_runs_the_finished_tracks_once_more() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let (_, r1) = run_job(fx.plan(3, &fx.five_tracks()), |_, _| {});
    assert!(r1.ok());

    let plan = fx.plan(3, &[]).with_force(true);
    let session_dir = plan.session_dir.clone();
    assert!(plan.force && plan.run.options.force);
    let (_, r2) = run_job(plan, |_, _| {});
    assert_eq!((r2.succeeded.len(), r2.skipped.len()), (5, 0), "{r2:?}");
    assert_eq!(
        read_state_rows(&format!("{session_dir}/transcription_state.csv")).len(),
        10
    );
}

#[test]
fn a_failed_track_can_be_retried_from_the_session_folder() {
    require_tools!();
    let failing = Fixture::new(&[("STUB_FAIL_MATCH", "riverone")]);
    let (_, r1) = run_job(failing.plan(4, &failing.five_tracks()), |_, _| {});
    assert_eq!(r1.failed.len(), 1);

    // Same audio folder, whisper now healthy.
    let mut healthy_env = failing.env.clone();
    healthy_env.remove("STUB_FAIL_MATCH");
    let s = Settings::from_env_map(healthy_env, "ttrpg-tui");
    let c = Campaign::load(&s, "demo").unwrap();
    let retry = plan_job(&s, &c, 4, &[]).unwrap();
    let (_, r2) = run_job(retry, |_, _| {});
    // Any file with an earlier Success row is skipped; the one without is tried again.
    assert_eq!(r2.succeeded, ["2-riverone.flac"], "{r2:?}");
    assert_eq!(r2.skipped.len(), 4);
    assert!(r2.ok());
}

#[test]
fn a_replaced_file_is_transcribed_again_even_though_its_name_was_done() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let first = fx.tone("1-gmperson.flac");
    let (_, r) = run_job(fx.plan(9, &[first]), |_, _| {});
    assert!(r.ok());

    // A second download with the same name lands on the first.
    let again = fx.tone("1-gmperson.flac");
    let plan = fx.plan(9, &[again]);
    assert_eq!(plan.overwritten(), ["1-gmperson.flac"]);
    assert_eq!(
        plan.existing.len(),
        0,
        "the name being replaced is not also counted as existing"
    );
    let session_dir = plan.session_dir.clone();
    let (events, r2) = run_job(plan, |_, _| {});
    assert_eq!((r2.succeeded.len(), r2.skipped.len()), (1, 0), "{r2:?}");
    assert!(log_text(&events).contains("Transcribing again (the file was replaced): 1-gmperson.flac"));
    assert_eq!(
        read_state_rows(&format!("{session_dir}/transcription_state.csv")).len(),
        2
    );
}

#[test]
fn cancel_stops_whisper_and_every_child() {
    require_tools!();
    let fx = Fixture::new(&[("STUB_SLEEP", "31.7")]);
    let plan = fx.plan(5, &fx.five_tracks());
    let (session_dir, temp_dir) = (plan.session_dir.clone(), plan.run.temp_dir.clone());
    let started = Instant::now();
    let seen_file_start = Mutex::new(false);
    let (events, report) = run_job(plan, |ev, cancel| {
        // Wait until whisper itself is running (it has printed a segment and is now asleep),
        // so the stub and its `sleep` are what must be stopped.
        if matches!(ev, PipelineEvent::Progress(ProgressEvent::InFile { .. })) {
            *seen_file_start.lock().unwrap() = true;
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert!(*seen_file_start.lock().unwrap());
    assert!(report.cancelled, "{report:?}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "cancel took {:?}",
        started.elapsed()
    );
    assert!(!report.ok());
    assert!(report.headline().starts_with("Cancelled"), "{}", report.headline());
    assert!(
        report.succeeded.is_empty(),
        "the in-flight track is not recorded as done"
    );
    assert!(report.failed.is_empty(), "stopping a track is not failing it");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PipelineEvent::Progress(ProgressEvent::PostDone))),
        "a cancelled run does not assemble a transcript"
    );
    assert!(
        read_state_rows(&format!("{session_dir}/transcription_state.csv")).is_empty(),
        "the cancelled track gets no state row, so a rerun does it"
    );
    assert!(
        !fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().ends_with(".wav")),
        "the converted audio is removed"
    );

    // Nothing left running: neither the stub nor its `sleep`.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let running = Command::new("pgrep")
            .args(["-f", "sleep 31.7"])
            .output()
            .unwrap()
            .status
            .success();
        if !running {
            break;
        }
        assert!(Instant::now() < deadline, "a child process survived the cancel");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_missing_whisper_is_a_failure_per_track_with_the_reason_in_the_log() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let mut env = fx.env.clone();
    env.insert(
        "GM_WHISPER_BIN".into(),
        fx.tmp.path().join("no/such/whisper-cli").to_string_lossy().into_owned(),
    );
    let s = Settings::from_env_map(env, "ttrpg-tui");
    let files = fx.five_tracks();
    let plan = plan_job(&s, &fx.campaign(), 6, &files[..2]).unwrap();
    let (events, report) = run_job(plan, |_, _| {});
    assert_eq!(report.failed.len(), 2, "{report:?}");
    assert!(report.succeeded.is_empty());
    assert!(!report.ok());
    assert_eq!(
        report.headline(),
        "0 of 2 transcribed, 2 failed; the transcript was not assembled"
    );
    let log = log_text(&events);
    assert!(
        log.contains("ERROR: Could not start") && log.contains("no/such/whisper-cli"),
        "{log}"
    );
}

#[test]
fn audio_ffmpeg_cannot_read_fails_that_track_only() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let good = fx.tone("1-gmperson.flac");
    let bad = fx.tmp.path().join("downloads/2-riverone.flac");
    fs::write(&bad, "this is not audio").unwrap();
    let (events, report) = run_job(fx.plan(2, &[good, bad]), |_, _| {});
    assert_eq!(report.succeeded, ["1-gmperson.flac"], "{report:?}");
    assert_eq!(
        report.failed,
        [("2-riverone.flac".to_string(), "ffmpeg conversion failed".to_string())]
    );
    assert!(report.transcript_md.is_some());
    assert!(log_text(&events).contains("ERROR: ffmpeg conversion failed for: 2-riverone.flac"));
}

#[test]
fn whisper_that_writes_no_json_is_reported() {
    require_tools!();
    let fx = Fixture::new(&[("STUB_NO_JSON", "1")]);
    let (_, report) = run_job(fx.plan(2, &[fx.tone("1-gmperson.flac")]), |_, _| {});
    assert_eq!(
        report.failed,
        [("1-gmperson.flac".to_string(), "Output JSON missing".to_string())]
    );
    assert!(!report.postprocess_done, "there is nothing to assemble");
    assert!(report.transcript_md.is_none());
}

#[test]
fn a_file_that_cannot_be_moved_stops_the_job_before_the_run() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let files = fx.five_tracks();
    let plan = fx.plan(8, &files);
    // Second file vanishes between planning and starting.
    fs::remove_file(&files[1]).unwrap();
    let (events, report) = run_job(plan, |_, _| {});
    let failure = report.failure.as_deref().expect("a failure");
    assert!(
        failure.contains("2-riverone.flac") && failure.contains("1 of 5 files were moved"),
        "{failure}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, PipelineEvent::Progress(_))),
        "the transcription never started"
    );
}

#[test]
fn a_folder_with_no_audio_says_so() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let plan = fx.plan(2, &[]);
    assert_eq!(plan.track_count(), 0);
    let (_, report) = run_job(plan, |_, _| {});
    assert_eq!(report.headline(), "No audio files found in the session folder.");
    assert!(!report.ok());
}

#[test]
fn planning_rejects_bad_input_before_touching_anything() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let s = fx.settings();
    let c = fx.campaign();
    let good = fx.tone("1-gmperson.flac");
    let err = |session: i64, files: &[PathBuf]| plan_job(&s, &c, session, files).unwrap_err().0;

    assert!(err(0, std::slice::from_ref(&good)).contains("1 or higher"));
    assert!(err(-3, std::slice::from_ref(&good)).contains("1 or higher"));
    assert!(err(1, &[fx.tmp.path().join("downloads/absent.flac")]).contains("no longer exists"));
    let text = fx.tmp.path().join("downloads/notes.txt");
    fs::write(&text, "x").unwrap();
    assert!(err(1, &[text]).contains("not an audio file"));
    fs::create_dir_all(fx.tmp.path().join("downloads/other")).unwrap();
    let twin = fx.tmp.path().join("downloads/other/1-gmperson.flac");
    fs::copy(&good, &twin).unwrap();
    assert!(err(1, &[good.clone(), twin]).contains("Two chosen files are both called 1-gmperson.flac"));
    assert!(good.exists(), "planning moved nothing");
    assert!(
        !Path::new(&fx.plan(1, &[good]).audio_dir).exists(),
        "planning created nothing"
    );
}

fn thanks_in_transcript(report: &RunReport) -> usize {
    read(report.transcript_md.as_deref().unwrap())
        .matches("Thank you.")
        .count()
}

/// The stub gives `rivertwo` a 30 s "Thank you." and a 1.5 s one.
#[test]
fn the_stock_phrase_rule_runs_by_default_and_follows_the_settings() {
    require_tools!();
    let run = |extra: &[(&str, &str)]| {
        let fx = Fixture::new(extra);
        let (_, report) = run_job(fx.plan(1, &[fx.tone("3-rivertwo.flac")]), |_, _| {});
        assert!(report.ok(), "{}", report.headline());
        thanks_in_transcript(&report)
    };
    assert_eq!(run(&[]), 1, "the 1.5 s one stays, the 30 s one is dropped");
    assert_eq!(run(&[("GM_STOCK_PHRASES", "off")]), 2);
    assert_eq!(
        run(&[("GM_STOCK_MIN_SECONDS", "31")]),
        2,
        "a longer limit than the 30 s line keeps it too"
    );
}

#[test]
fn the_rule_can_be_switched_for_one_run() {
    require_tools!();
    let run = |switch: fn(ttrpg_core::JobPlan) -> ttrpg_core::JobPlan| {
        let fx = Fixture::new(&[("GM_STOCK_MIN_SECONDS", "45")]);
        let plan = switch(fx.plan(1, &[fx.tone("3-rivertwo.flac")]));
        let (_, report) = run_job(plan, |_, _| {});
        assert!(report.ok(), "{}", report.headline());
        thanks_in_transcript(&report)
    };
    // 45 s is longer than the 30 s line, so by default both stay.
    assert_eq!(run(|p| p), 2);
    // Switched to the stock default (20 s) for this run, the 30 s one goes.
    assert_eq!(run(|p| p.with_stock_rule(StockRule::default())), 1);
    // Off stays off whatever the limit.
    assert_eq!(
        run(|p| {
            let off = p.stock.off();
            let p = p.with_stock_rule(off);
            assert!(!p.stock.enabled() && p.stock.min_seconds == 45);
            assert!(!p.run.options.stock.enabled(), "the runner gets the switched rule");
            p
        }),
        2
    );
}

#[test]
fn whisper_gets_the_arguments_the_script_gave_it() {
    require_tools!();
    let run = |extra: &[(&str, &str)]| -> Vec<String> {
        let tmp = TempDir::new().unwrap();
        let args = tmp.path().join("args.txt").to_string_lossy().into_owned();
        let mut with_args = vec![("STUB_ARGS_FILE", args.as_str())];
        with_args.extend_from_slice(extra);
        let fx = Fixture::new(&with_args);
        let (_, report) = run_job(fx.plan(1, &[fx.tone("1-gmperson.flac")]), |_, _| {});
        assert!(report.ok(), "{}", report.headline());
        read(&args).lines().map(String::from).collect()
    };
    let default = run(&[]);
    assert_eq!(default[0], "-m");
    assert!(default[1].ends_with("/model.bin"), "{default:?}");
    let pos = |a: &[String], flag: &str| a.iter().position(|x| x == flag);
    assert_eq!(default[pos(&default, "-l").unwrap() + 1], "en");
    assert_eq!(
        default[pos(&default, "-mc").unwrap() + 1],
        "0",
        "no context from earlier segments"
    );
    assert_eq!(default[pos(&default, "--prompt").unwrap() + 1], "Invented demo.");
    assert!(default.contains(&"-oj".to_string()));
    assert!(pos(&default, "-dev").is_none(), "no device unless one is set");
    assert!(default.last().unwrap().ends_with("1-gmperson_input.wav"), "{default:?}");
    let of = &default[pos(&default, "-of").unwrap() + 1];
    assert!(of.ends_with("/temp/1-gmperson"), "{of}");

    let with_device = run(&[("GM_WHISPER_DEVICE", "1"), ("GM_WHISPER_MC", "-1")]);
    assert_eq!(with_device[pos(&with_device, "-dev").unwrap() + 1], "1");
    assert!(pos(&with_device, "-mc").is_none(), "-1 leaves whisper's own default");
}

#[test]
fn the_transcript_can_be_rebuilt_from_the_saved_output_without_audio_or_whisper() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let first = fx.plan(1, &[fx.tone("3-rivertwo.flac"), fx.tone("1-gmperson.flac")]);
    let (audio_dir, session_dir) = (first.audio_dir.clone(), first.session_dir.clone());
    let (_, r1) = run_job(first, |_, _| {});
    assert!(r1.ok());
    assert_eq!(thanks_in_transcript(&r1), 1);
    let rows_before = state_summary(&Path::new(&session_dir).join("transcription_state.csv"));

    // The audio is gone, whisper is gone, and the rule has changed.
    fs::remove_dir_all(&audio_dir).unwrap();
    let mut env = fx.env.clone();
    env.insert("GM_WHISPER_BIN".into(), "/no/such/whisper".into());
    env.insert("GM_STOCK_PHRASES".into(), "off".into());
    let s = Settings::from_env_map(env, "ttrpg-tui");
    let plan = plan_reprocess(&s, &fx.campaign(), 1).unwrap();
    assert!(plan.reprocess && plan.moves.is_empty() && plan.run.options.post_process_only);
    // A leftover from a track that is no longer part of the session must not come back.
    fs::write(
        format!("{}/9-ghost.processed.tsv", plan.run.temp_dir),
        "speaker\tstart\tend\ttext\nghost\t1\t2\tI am a leftover\n",
    )
    .unwrap();
    let (events, r2) = run_job(plan, |_, _| {});
    assert!(r2.ok(), "{}", r2.headline());
    assert!(r2.rebuilt);
    assert!(!read(r2.transcript_md.as_deref().unwrap()).contains("leftover"));
    assert_eq!(r2.headline(), "Rebuilt the transcript from the saved whisper output.");
    assert_eq!(
        thanks_in_transcript(&r2),
        2,
        "the 30 s line is back: the new rule applies to the old output"
    );
    assert!(log_text(&events).contains("Reprocessing: 3-rivertwo.whisper_original.json → speaker: Tomas_Ink"));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PipelineEvent::Progress(ProgressEvent::FileStart { .. })))
    );
    assert_eq!(
        state_summary(&Path::new(&session_dir).join("transcription_state.csv")),
        rows_before,
        "a rebuild adds no state rows"
    );
}

#[test]
fn a_rebuild_with_nothing_saved_is_refused_at_planning() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let err = plan_reprocess(&fx.settings(), &fx.campaign(), 4).unwrap_err().0;
    assert!(err.contains("no saved whisper output for session 4"), "{err}");
    assert!(
        plan_reprocess(&fx.settings(), &fx.campaign(), 0)
            .unwrap_err()
            .0
            .contains("1 or higher")
    );
    assert!(!ttrpg_core::has_saved_output(&fx.settings(), &fx.campaign(), 4));
}

#[test]
fn a_second_segment_ending_in_the_same_place_does_not_repeat_the_progress_event() {
    require_tools!();
    let fx = Fixture::new(&[("STUB_DUP", "1")]);
    let (events, report) = run_job(fx.plan(1, &[fx.tone("1-gmperson.flac")]), |_, _| {});
    assert!(report.ok());
    let marks: Vec<u16> = events
        .iter()
        .filter_map(|e| match e {
            PipelineEvent::Progress(ProgressEvent::InFile { permille, .. }) => Some(*permille),
            _ => None,
        })
        .collect();
    assert_eq!(marks, [900, 1000]);
}

#[test]
fn a_quote_in_a_file_name_cannot_break_the_state_file() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let odd = fx.tone("6-the \"odd\" one.flac");
    let plan = fx.plan(1, &[odd]);
    let state = plan.run.state_path();
    let (events, report) = run_job(plan, |_, _| {});
    assert!(report.ok(), "{}\n{}", report.headline(), log_text(&events));
    let rows = read_state_rows(&state.to_string_lossy());
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].file, "6-the 'odd' one.flac",
        "double quotes become single, as the script did"
    );
    assert_eq!(rows[0].status, "Success");
}
