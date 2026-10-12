//! The transcription job (PORTING.md section 11, milestone 2): move the chosen
//! audio into the session folder, run `legacy/transcribe.sh`, and turn what it
//! prints into typed events.
//!
//! [`plan_job`] works out everything up front, on the caller's thread, so that
//! mistakes surface before any file is touched. [`execute`] is meant to run on its
//! own thread and report through a channel; the UI thread never waits on it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::audio::{is_audio_file_name, list_audio_files};
use crate::campaign::Campaign;
use crate::error::CampaignError;
use crate::proc::run_streaming;
use crate::progress::{ProgressEvent, parse_progress_line};
use crate::settings::{Settings, Sourced};
use crate::state::read_state_rows;
use crate::stock::StockRule;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovePlan {
    pub src: PathBuf,
    pub dst: PathBuf,
    /// A different file with this name is already in the session folder and will be replaced.
    pub overwrites: bool,
}

#[derive(Debug, Clone)]
pub struct JobPlan {
    pub slug: String,
    pub session: i64,
    pub audio_dir: String,
    pub session_dir: String,
    pub transcript_md: String,
    pub state_file: String,
    pub moves: Vec<MovePlan>,
    /// Audio already in the session folder that is not being replaced.
    pub existing: Vec<String>,
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    /// The stock-phrase rule this run will use (already in `args`).
    pub stock: StockRule,
}

impl JobPlan {
    /// The same plan with a different stock-phrase rule, for turning it off or on
    /// for one run from the confirm step.
    pub fn with_stock_rule(mut self, rule: StockRule) -> Self {
        let mut kept = Vec::with_capacity(self.args.len());
        let mut it = std::mem::take(&mut self.args).into_iter();
        while let Some(a) = it.next() {
            if a == "--stock-phrases" || a == "--stock-min-seconds" {
                it.next(); // its value
            } else {
                kept.push(a);
            }
        }
        kept.extend(rule.script_args());
        self.args = kept;
        self.stock = rule;
        self
    }

    /// Tracks the script will find: what is already there plus what is moved in.
    pub fn track_count(&self) -> usize {
        self.existing.len() + self.moves.len()
    }

    /// Names of files in the session folder that moving will replace.
    pub fn overwritten(&self) -> Vec<String> {
        self.moves
            .iter()
            .filter(|m| m.overwrites)
            .filter_map(|m| m.dst.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect()
    }
}

/// Work out the whole job without touching anything.
pub fn plan_job(
    settings: &Settings,
    campaign: &Campaign,
    session: i64,
    files: &[PathBuf],
    legacy: &Sourced,
) -> Result<JobPlan, CampaignError> {
    if session <= 0 {
        return Err(CampaignError::new("The session number must be 1 or higher."));
    }
    let audio_dir = campaign.audio_dir(settings, session)?;
    let session_dir = campaign.session_dir(settings, session)?;

    let mut moves = Vec::new();
    let mut seen: HashMap<String, PathBuf> = HashMap::new();
    for src in files {
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| CampaignError::new(format!("{} is not a file.", src.display())))?;
        if !src.is_file() {
            return Err(CampaignError::new(format!(
                "{} no longer exists or is not a file.",
                src.display()
            )));
        }
        if !is_audio_file_name(&name) {
            return Err(CampaignError::new(format!(
                "{name} is not an audio file the pipeline reads."
            )));
        }
        if let Some(other) = seen.insert(name.clone(), src.clone()) {
            return Err(CampaignError::new(format!(
                "Two chosen files are both called {name}: {} and {}.",
                other.display(),
                src.display()
            )));
        }
        let dst = Path::new(&audio_dir).join(&name);
        let overwrites = dst.exists() && !same_file(src, &dst);
        moves.push(MovePlan {
            src: src.clone(),
            dst,
            overwrites,
        });
    }

    let moved_names: Vec<&String> = seen.keys().collect();
    let existing = list_audio_files(&audio_dir)
        .into_iter()
        .filter(|n| !moved_names.contains(&n))
        .collect();

    let mut args = vec![
        Path::new(&legacy.value)
            .join("transcribe.sh")
            .to_string_lossy()
            .into_owned(),
        "--campaign".into(),
        campaign.slug.clone(),
        "--session".into(),
        session.to_string(),
        "-i".into(),
        audio_dir.clone(),
        "-o".into(),
        session_dir.clone(),
    ];
    if let Some(bin) = settings.whisper_bin()? {
        args.extend(["-w".into(), bin.value]);
    }
    if let Some(model) = settings.whisper_model()? {
        args.extend(["-m".into(), model.value]);
    }
    let stock = settings.stock_rule()?;
    args.extend(stock.script_args());

    Ok(JobPlan {
        slug: campaign.slug.clone(),
        session,
        transcript_md: campaign.transcript_path(settings, session)?,
        state_file: Path::new(&session_dir)
            .join("transcription_state.csv")
            .to_string_lossy()
            .into_owned(),
        audio_dir,
        session_dir,
        moves,
        existing,
        program: "bash".into(),
        args,
        env: settings.env_vars().clone(),
        stock,
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Move one file. Same file is a no-op; across file systems it copies then removes.
/// A file with the destination's name is replaced.
fn move_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if same_file(src, dst) {
        return Ok(());
    }
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            if let Err(e) = std::fs::copy(src, dst) {
                let _ = std::fs::remove_file(dst);
                return Err(e);
            }
            std::fs::remove_file(src)
        }
        Err(e) => Err(e),
    }
}

/// Everything the UI hears from a running job.
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    Moving {
        index: usize,
        total: usize,
        name: String,
    },
    Progress(ProgressEvent),
    Log(String),
    /// A step of the index job began: what to call it and where the bar goes.
    Stage {
        label: String,
        fraction: f64,
    },
    Finished(Box<RunReport>),
    /// The index or alias-report job ended (see [`crate::index`]).
    IndexFinished(Box<crate::index::IndexReport>),
}

/// How a run ended, for the summary line and the log.
#[derive(Debug, Clone, Default)]
pub struct RunReport {
    pub session: i64,
    pub moved: usize,
    /// Audio files the script found (`PROGRESS:start`).
    pub tracks_found: Option<usize>,
    pub succeeded: Vec<String>,
    /// File and the script's error message.
    pub failed: Vec<(String, String)>,
    /// Already transcribed on an earlier run, so the script skipped them.
    pub skipped: Vec<String>,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub postprocess_done: bool,
    pub transcript_md: Option<String>,
    /// Set when the job could not run at all (a move failed, bash would not start).
    pub failure: Option<String>,
    pub elapsed: Duration,
}

impl RunReport {
    /// Everything the script was asked to do worked.
    pub fn ok(&self) -> bool {
        self.failure.is_none()
            && !self.cancelled
            && self.failed.is_empty()
            && self.exit_code == Some(0)
            && self.postprocess_done
    }

    /// The honest one-line result. The GUI said "Complete!" whatever happened.
    pub fn headline(&self) -> String {
        if let Some(f) = &self.failure {
            return f.clone();
        }
        let Some(found) = self.tracks_found else {
            return match self.exit_code {
                Some(0) if !self.cancelled => "No audio files found in the session folder.".to_string(),
                _ if self.cancelled => "Cancelled before the first track.".to_string(),
                Some(code) => {
                    format!("transcribe.sh stopped with exit code {code} before the first track. See the log.")
                }
                None => "transcribe.sh did not finish. See the log.".to_string(),
            };
        };
        let mut parts = vec![format!("{} of {found} transcribed", self.succeeded.len())];
        if !self.failed.is_empty() {
            parts.push(format!("{} failed", self.failed.len()));
        }
        if !self.skipped.is_empty() {
            parts.push(format!("{} already done", self.skipped.len()));
        }
        let mut line = parts.join(", ");
        if self.cancelled {
            line = format!("Cancelled: {line}");
        } else if self.exit_code != Some(0) {
            line.push_str(&format!(
                "; transcribe.sh exit code {}",
                self.exit_code.map_or("?".into(), |c| c.to_string())
            ));
        } else if !self.postprocess_done {
            line.push_str("; the transcript was not assembled");
        }
        line
    }
}

fn send(tx: &Sender<PipelineEvent>, ev: PipelineEvent) {
    let _ = tx.send(ev);
}

/// Run the job to the end and send [`PipelineEvent::Finished`] last. Never panics on
/// I/O trouble: every failure becomes `RunReport::failure`.
pub fn execute(plan: JobPlan, tx: Sender<PipelineEvent>, cancel: Arc<AtomicBool>) {
    let started = Instant::now();
    let mut report = run(&plan, &tx, &cancel);
    report.elapsed = started.elapsed();
    send(&tx, PipelineEvent::Finished(Box::new(report)));
}

fn run(plan: &JobPlan, tx: &Sender<PipelineEvent>, cancel: &AtomicBool) -> RunReport {
    let mut report = RunReport {
        session: plan.session,
        ..RunReport::default()
    };
    let log = |s: String| send(tx, PipelineEvent::Log(s));

    for dir in [&plan.audio_dir, &plan.session_dir] {
        if let Err(e) = std::fs::create_dir_all(dir) {
            report.failure = Some(format!("Could not create {dir}: {e}"));
            return report;
        }
    }
    log(format!("Session folder: {}", plan.session_dir));
    log(format!("Audio folder:   {}", plan.audio_dir));

    let total = plan.moves.len();
    for (i, m) in plan.moves.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            report.failure = None;
            return report;
        }
        let name = m
            .dst
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        send(
            tx,
            PipelineEvent::Moving {
                index: i + 1,
                total,
                name: name.clone(),
            },
        );
        if let Err(e) = move_file(&m.src, &m.dst) {
            report.failure = Some(format!(
                "Could not move {} to {}: {e}. {} of {total} files were moved.",
                m.src.display(),
                m.dst.display(),
                report.moved
            ));
            return report;
        }
        report.moved += 1;
        log(format!(
            "Moved: {name}{}",
            if m.overwrites {
                " (replaced an existing file)"
            } else {
                ""
            }
        ));
    }

    let baseline = read_state_rows(&plan.state_file).len();

    let mut done_names: Vec<String> = Vec::new();
    let outcome = run_streaming(
        &plan.program,
        &plan.args,
        &plan.env,
        cancel,
        |line| match parse_progress_line(&line) {
            Some(ev) => {
                match &ev {
                    ProgressEvent::Start { total } => report.tracks_found = Some(*total),
                    ProgressEvent::FileDone { name, .. } => done_names.push(name.clone()),
                    ProgressEvent::PostDone => report.postprocess_done = true,
                    _ => {}
                }
                send(tx, PipelineEvent::Progress(ev));
            }
            None => send(tx, PipelineEvent::Log(line)),
        },
        || {
            send(
                tx,
                PipelineEvent::Log("Cancelling: stopping the script and its children...".into()),
            )
        },
    );
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => {
            report.failure = Some(format!("Could not start {}: {e}", plan.program));
            return report;
        }
    };
    report.cancelled = outcome.cancelled;
    report.exit_code = outcome.exit_code;

    // What actually happened to each file this run, from the rows it appended.
    let rows = read_state_rows(&plan.state_file);
    for row in rows.into_iter().skip(baseline) {
        if row.succeeded() {
            report.succeeded.push(row.file);
        } else {
            let msg = if row.error.is_empty() { row.status } else { row.error };
            report.failed.push((row.file, msg));
        }
    }
    report.skipped = done_names
        .into_iter()
        .filter(|n| !report.succeeded.contains(n) && !report.failed.iter().any(|(f, _)| f == n))
        .collect();
    if Path::new(&plan.transcript_md).is_file() {
        report.transcript_md = Some(plan.transcript_md.clone());
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> RunReport {
        RunReport {
            session: 1,
            tracks_found: Some(5),
            exit_code: Some(0),
            postprocess_done: true,
            ..Default::default()
        }
    }

    #[test]
    fn headline_is_honest_about_failures() {
        let mut r = report();
        r.succeeded = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        r.failed = vec![("e".into(), "whisper-cli exited non-zero".into())];
        assert_eq!(r.headline(), "4 of 5 transcribed, 1 failed");
        assert!(!r.ok());
    }

    #[test]
    fn headline_counts_skipped_and_flags_a_bad_exit() {
        let mut r = report();
        r.succeeded = vec!["a".into()];
        r.skipped = vec!["b".into(), "c".into(), "d".into(), "e".into()];
        assert_eq!(r.headline(), "1 of 5 transcribed, 4 already done");
        assert!(r.ok());
        r.exit_code = Some(1);
        assert_eq!(
            r.headline(),
            "1 of 5 transcribed, 4 already done; transcribe.sh exit code 1"
        );
        assert!(!r.ok());
    }

    #[test]
    fn headline_covers_the_edges() {
        let mut r = RunReport {
            exit_code: Some(0),
            ..Default::default()
        };
        assert_eq!(r.headline(), "No audio files found in the session folder.");
        r.exit_code = Some(1);
        assert!(r.headline().contains("exit code 1 before the first track"));
        r.cancelled = true;
        assert_eq!(r.headline(), "Cancelled before the first track.");
        let r = RunReport {
            failure: Some("Could not move x".into()),
            ..Default::default()
        };
        assert_eq!(r.headline(), "Could not move x");
        let mut r = report();
        r.succeeded = vec!["a".into(), "b".into()];
        r.cancelled = true;
        assert_eq!(r.headline(), "Cancelled: 2 of 5 transcribed");
        let mut r = report();
        r.postprocess_done = false;
        assert!(r.headline().ends_with("the transcript was not assembled"));
    }
}
