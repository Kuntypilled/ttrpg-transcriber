//! End-to-end tests of the transcription job: the real `legacy/transcribe.sh`, real
//! `ffmpeg` and `jq`, and a stub `whisper-cli` that writes canned JSON, on tone
//! files with invented names. They need bash, python3, ffmpeg and jq; without them
//! each test prints SKIPPED and passes.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use ttrpg_core::job::{PipelineEvent, RunReport, execute, plan_job};
use ttrpg_core::progress::{Phase, ProgressEvent, Tracker};
use ttrpg_core::{Campaign, Settings, legacy, resolve_speaker};

const STUB: &str = r#"#!/usr/bin/env bash
# Stub whisper-cli: finds -of and the wav, writes <of>.json. STUB_SLEEP delays,
# STUB_FAIL_MATCH makes it fail for any wav whose path contains the text.
of=""; wav=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -of) of="$2"; shift 2 ;;
    -m|-l|--prompt|-dev|-mc) shift 2 ;;
    -oj) shift ;;
    *) wav="$1"; shift ;;
  esac
done
[[ -n "${STUB_SLEEP:-}" ]] && sleep "$STUB_SLEEP"
if [[ -n "${STUB_FAIL_MATCH:-}" && "$wav" == *"$STUB_FAIL_MATCH"* ]]; then exit 3; fi
if [[ -n "${STUB_THANKS:-}" ]]; then
cat > "${of}.json" <<'J'
{"transcription":[
 {"offsets":{"from":1000,"to":2000},"text":" The lantern is lit."},
 {"offsets":{"from":10000,"to":40000},"text":" Thank you."},
 {"offsets":{"from":41000,"to":42000},"text":" We go on."},
 {"offsets":{"from":43000,"to":44500},"text":" Thank you."}
]}
J
exit 0
fi
cat > "${of}.json" <<'J'
{"transcription":[
 {"offsets":{"from":0,"to":900},"text":" you"},
 {"offsets":{"from":1000,"to":2000},"text":" The lantern is lit."},
 {"offsets":{"from":2000,"to":3000},"text":" the lantern is lit."},
 {"offsets":{"from":4000,"to":5000},"text":" It’s the Cinder Road."}
]}
J
"#;

fn tools_present() -> bool {
    ["bash", "python3", "ffmpeg", "jq"].iter().all(|t| {
        Command::new("sh")
            .args(["-c", &format!("command -v {t}")])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

macro_rules! require_tools {
    () => {
        if !tools_present() {
            eprintln!("SKIPPED: bash, python3, ffmpeg and jq are needed");
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
        fs::write(
            camp.join("canon_aliases.json"),
            r#"{"campaign": "Demo", "system": "daggerheart", "seed_prompt": "Invented demo.",
                "discord_names": {
                    "gmperson":  {"player": "Sky", "character": null, "role": "GM"},
                    "riverone":  {"player": "Pat", "character": "Mara_Quill", "role": "Player"},
                    "rivertwo":  {"player": "Lee", "character": "Tomas_Ink", "role": "Player"}
                }}"#,
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("bin")).unwrap();
        let stub = tmp.path().join("bin/stub-whisper");
        fs::write(&stub, STUB).unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
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
            ("GM_WHISPER_BIN".to_string(), stub.to_string_lossy().into_owned()),
            ("GM_WHISPER_MODEL".to_string(), p("model.bin")),
        ]);
        env.extend(extra_env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Fixture { tmp, env }
    }

    fn settings(&self) -> Settings {
        Settings::from_env_map(self.env.clone(), "ttrpg-tui")
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
        let s = self.settings();
        let c = Campaign::load(&s, "demo").unwrap();
        let legacy = legacy::locate(&s).unwrap();
        plan_job(&s, &c, session, files, &legacy).unwrap()
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

#[test]
fn a_full_run_moves_files_runs_the_script_and_reports_honestly() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let files = fx.five_tracks();
    let plan = fx.plan(7, &files);
    assert_eq!(plan.track_count(), 5);
    assert!(plan.overwritten().is_empty());
    let (audio_dir, transcript) = (plan.audio_dir.clone(), plan.transcript_md.clone());

    let (events, report) = run_job(plan, |_, _| {});

    // The files moved: gone from downloads, present in the session audio folder.
    for f in &files {
        assert!(!f.exists(), "{} should have been moved", f.display());
        assert!(Path::new(&audio_dir).join(f.file_name().unwrap()).is_file());
    }
    assert!(audio_dir.ends_with("/Session 7"), "{audio_dir}");

    // Typed events, in order, ending with Finished.
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            PipelineEvent::Moving { .. } => "move",
            PipelineEvent::Progress(ProgressEvent::Start { .. }) => "start",
            PipelineEvent::Progress(ProgressEvent::FileStart { .. }) => "file",
            PipelineEvent::Progress(ProgressEvent::FileDone { .. }) => "done",
            PipelineEvent::Progress(ProgressEvent::TranscribeComplete) => "complete",
            PipelineEvent::Progress(ProgressEvent::PostDone) => "postdone",
            PipelineEvent::Log(_) => "log",
            PipelineEvent::Finished(_) => "finished",
        })
        .collect();
    let compact: Vec<&str> = kinds.iter().copied().filter(|k| *k != "log").collect();
    assert_eq!(
        compact,
        [
            "move", "move", "move", "move", "move", "start", "file", "done", "file", "done", "file", "done", "file",
            "done", "file", "done", "complete", "postdone", "finished"
        ]
    );
    assert!(kinds.contains(&"log"), "script output reaches the log");

    // The bar ends full.
    let mut tracker = Tracker::default();
    for e in &events {
        if let PipelineEvent::Progress(p) = e {
            tracker.apply(p);
        }
    }
    assert_eq!((tracker.phase, tracker.fraction()), (Phase::Finished, 1.0));

    // The report.
    assert_eq!(report.succeeded.len(), 5, "{report:?}");
    assert!(report.failed.is_empty() && report.skipped.is_empty());
    assert_eq!(
        (report.moved, report.tracks_found, report.exit_code),
        (5, Some(5), Some(0))
    );
    assert!(report.ok());
    assert_eq!(report.headline(), "5 of 5 transcribed");
    assert_eq!(report.transcript_md.as_deref(), Some(transcript.as_str()));

    // One speaker-resolution function: the Rust labels are the ones the script wrote.
    let s = fx.settings();
    let camp = Campaign::load(&s, "demo").unwrap();
    let want: BTreeSet<String> = files
        .iter()
        .map(|f| resolve_speaker(&camp, &f.file_name().unwrap().to_string_lossy()).label())
        .collect();
    assert_eq!(
        want,
        BTreeSet::from(["GM", "Mara_Quill", "Tomas_Ink", "plain", "5-newperson"].map(String::from))
    );
    assert_eq!(transcript_labels(&transcript), want);
    let text = fs::read_to_string(&transcript).unwrap();
    assert!(text.contains("The lantern is lit."));
    assert_eq!(
        text.matches("The lantern is lit.").count(),
        5,
        "duplicate lines collapsed within each track"
    );
    assert!(!text.contains("] GM:**  \nyou\n"), "filler words are filtered");
}

#[test]
fn a_failing_track_is_named_not_hidden() {
    require_tools!();
    let fx = Fixture::new(&[("STUB_FAIL_MATCH", "rivertwo")]);
    let files = fx.five_tracks();
    let plan = fx.plan(2, &files);
    let (_, report) = run_job(plan, |_, _| {});
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
    let (_, r2) = run_job(again, |_, _| {});
    assert_eq!(
        (r2.succeeded.len(), r2.skipped.len(), r2.failed.len()),
        (0, 5, 0),
        "{r2:?}"
    );
    assert_eq!(r2.headline(), "0 of 5 transcribed, 5 already done");
    assert!(r2.ok());
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
    let legacy = legacy::locate(&s).unwrap();
    let retry = plan_job(&s, &c, 4, &[], &legacy).unwrap();
    let (_, r2) = run_job(retry, |_, _| {});
    // The script skips any file with an earlier Success row, and retries the one without.
    assert_eq!(r2.succeeded, ["2-riverone.flac"], "{r2:?}");
    assert_eq!(r2.skipped.len(), 4);
    assert!(r2.ok());
}

#[test]
fn cancel_stops_the_script_and_every_child() {
    require_tools!();
    let fx = Fixture::new(&[("STUB_SLEEP", "31.7")]);
    let plan = fx.plan(5, &fx.five_tracks());
    let started = Instant::now();
    let seen_file_start = Mutex::new(false);
    let (_, report) = run_job(plan, |ev, cancel| {
        if matches!(ev, PipelineEvent::Progress(ProgressEvent::FileStart { .. })) {
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
fn a_missing_script_is_a_clear_failure_with_the_shell_message_in_the_log() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let mut plan = fx.plan(6, &fx.five_tracks());
    plan.args[0] = fx
        .tmp
        .path()
        .join("no/such/transcribe.sh")
        .to_string_lossy()
        .into_owned();
    let (events, report) = run_job(plan, |_, _| {});
    assert_eq!(report.tracks_found, None);
    assert_eq!(report.exit_code, Some(127));
    assert!(
        report.headline().contains("exit code 127 before the first track"),
        "{}",
        report.headline()
    );
    let log: String = events
        .iter()
        .filter_map(|e| {
            if let PipelineEvent::Log(l) = e {
                Some(l.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(log.contains("No such file"), "{log}");
}

#[test]
fn a_file_that_cannot_be_moved_stops_the_job_before_the_script() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let files = fx.five_tracks();
    let mut plan = fx.plan(8, &files);
    // Second file vanishes between planning and starting.
    fs::remove_file(&files[1]).unwrap();
    let (events, report) = run_job(plan.clone(), |_, _| {});
    let failure = report.failure.as_deref().expect("a failure");
    assert!(
        failure.contains("2-riverone.flac") && failure.contains("1 of 5 files were moved"),
        "{failure}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, PipelineEvent::Progress(_))),
        "the script never ran"
    );
    plan.moves.clear();
}

#[test]
fn planning_rejects_bad_input_before_touching_anything() {
    require_tools!();
    let fx = Fixture::new(&[]);
    let s = fx.settings();
    let c = Campaign::load(&s, "demo").unwrap();
    let legacy = legacy::locate(&s).unwrap();
    let good = fx.tone("1-gmperson.flac");
    let err = |session: i64, files: &[PathBuf]| plan_job(&s, &c, session, files, &legacy).unwrap_err().0;

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

#[test]
fn replacing_a_same_name_file_is_flagged_in_the_plan() {
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
}

#[test]
fn the_stock_phrase_rule_runs_by_default_and_the_settings_reach_the_script() {
    require_tools!();
    let count = |report: &RunReport| {
        fs::read_to_string(report.transcript_md.as_deref().unwrap())
            .unwrap()
            .matches("Thank you.")
            .count()
    };

    // Nothing set: no flags are passed, the script's own default drops the 30 s one.
    let fx = Fixture::new(&[("STUB_THANKS", "1")]);
    let plan = fx.plan(1, &[fx.tone("1-gmperson.flac")]);
    assert!(plan.args.iter().all(|a| !a.starts_with("--stock")), "{:?}", plan.args);
    assert!(plan.stock.enabled());
    let (_, report) = run_job(plan, |_, _| {});
    assert!(report.ok(), "{}", report.headline());
    assert_eq!(count(&report), 1, "the 1.5 s one stays, the 30 s one is dropped");

    // Turned off in the settings: the flag is passed with an empty list, both stay.
    let fx = Fixture::new(&[("STUB_THANKS", "1"), ("GM_STOCK_PHRASES", "off")]);
    let plan = fx.plan(1, &[fx.tone("1-gmperson.flac")]);
    let at = plan.args.iter().position(|a| a == "--stock-phrases").unwrap();
    assert_eq!(plan.args[at + 1], "");
    let (_, report) = run_job(plan, |_, _| {});
    assert_eq!(count(&report), 2);

    // A longer limit than the 30 s line keeps it too.
    let fx = Fixture::new(&[("STUB_THANKS", "1"), ("GM_STOCK_MIN_SECONDS", "31")]);
    let plan = fx.plan(1, &[fx.tone("1-gmperson.flac")]);
    assert!(plan.args.windows(2).any(|w| w == ["--stock-min-seconds", "31"]));
    let (_, report) = run_job(plan, |_, _| {});
    assert_eq!(count(&report), 2);
}

#[test]
fn the_rule_can_be_switched_for_one_run_without_leaving_stale_flags() {
    require_tools!();
    let fx = Fixture::new(&[("GM_STOCK_MIN_SECONDS", "45")]);
    let plan = fx.plan(1, &[fx.tone("1-gmperson.flac")]);
    let flags = |p: &ttrpg_core::JobPlan| {
        p.args
            .iter()
            .filter(|a| a.starts_with("--stock"))
            .cloned()
            .collect::<Vec<_>>()
    };
    let others = |p: &ttrpg_core::JobPlan| {
        let mut out = Vec::new();
        let mut it = p.args.iter();
        while let Some(a) = it.next() {
            if a.starts_with("--stock") {
                it.next();
            } else {
                out.push(a.clone());
            }
        }
        out
    };
    assert_eq!(flags(&plan), ["--stock-min-seconds"]);

    let off = plan.clone().with_stock_rule(plan.stock.off());
    assert_eq!(
        flags(&off),
        ["--stock-phrases", "--stock-min-seconds"],
        "once each, no duplicates"
    );
    assert!(!off.stock.enabled() && off.stock.min_seconds == 45);
    assert_eq!(others(&off), others(&plan), "every other argument is untouched");

    let back = off.with_stock_rule(ttrpg_core::StockRule::default());
    assert!(flags(&back).is_empty(), "the default needs no flags: {:?}", back.args);
    assert_eq!(others(&back), others(&plan));
}
