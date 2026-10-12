//! The transcription job (PORTING.md section 11): move the chosen audio into the
//! session folder, then run the transcription ([`crate::transcribe`]) and turn its
//! progress into typed events.
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
use crate::progress::ProgressEvent;
use crate::settings::Settings;
use crate::stock::StockRule;
use crate::transcribe::{self, Options, RunConfig};

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
    /// The stock-phrase rule this run will use.
    pub stock: StockRule,
    /// Transcribe again the tracks that already succeeded.
    pub force: bool,
    /// Rebuild the transcripts from the saved whisper output; no audio, nothing moves.
    pub reprocess: bool,
    pub run: RunConfig,
}

impl JobPlan {
    /// The same plan with a different stock-phrase rule, for turning it off or on
    /// for one run from the confirm step.
    pub fn with_stock_rule(mut self, rule: StockRule) -> Self {
        self.run.options.stock = rule.clone();
        self.stock = rule;
        self
    }

    /// The same plan with "transcribe again" switched.
    pub fn with_force(mut self, force: bool) -> Self {
        self.run.options.force = force;
        self.force = force;
        self
    }

    /// Tracks the run will find: what is already there plus what is moved in.
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

fn options_from(settings: &Settings, reprocess: bool) -> Result<Options, CampaignError> {
    Ok(Options {
        force: false,
        redo: Vec::new(),
        post_process_only: reprocess,
        cleanup: false,
        ignore_words: settings.ignore_words()?,
        stock: settings.stock_rule()?,
        whisper_bin: settings.effective_whisper_bin()?,
        model: settings.effective_model()?,
        device: settings.whisper_device()?.map(|(n, _)| n),
        max_context: settings.whisper_max_context()?,
        ffmpeg: "ffmpeg".into(),
    })
}

fn base_plan(
    settings: &Settings,
    campaign: &Campaign,
    session: i64,
    reprocess: bool,
    moves: Vec<MovePlan>,
    existing: Vec<String>,
) -> Result<JobPlan, CampaignError> {
    let audio_dir = campaign.audio_dir(settings, session)?;
    let session_dir = campaign.session_dir(settings, session)?;
    let options = options_from(settings, reprocess)?;
    let stock = options.stock.clone();
    Ok(JobPlan {
        slug: campaign.slug.clone(),
        session,
        transcript_md: campaign.transcript_path(settings, session)?,
        state_file: Path::new(&session_dir)
            .join("transcription_state.csv")
            .to_string_lossy()
            .into_owned(),
        run: RunConfig {
            campaign: campaign.clone(),
            audio_dir: audio_dir.clone(),
            session_dir: session_dir.clone(),
            temp_dir: Path::new(&campaign.work_dir(settings, session)?)
                .join("temp")
                .to_string_lossy()
                .into_owned(),
            options,
            env: settings.env_vars().clone(),
        },
        audio_dir,
        session_dir,
        moves,
        existing,
        stock,
        force: false,
        reprocess,
    })
}

/// Whether session `session` has saved whisper output to rebuild from.
pub fn has_saved_output(settings: &Settings, campaign: &Campaign, session: i64) -> bool {
    let has = |dir: &Path, suffix: &str| {
        std::fs::read_dir(dir).is_ok_and(|rd| {
            rd.filter_map(Result::ok)
                .any(|e| e.file_name().to_string_lossy().ends_with(suffix))
        })
    };
    let (Ok(session_dir), Ok(work_dir)) = (
        campaign.session_dir(settings, session),
        campaign.work_dir(settings, session),
    ) else {
        return false;
    };
    has(&Path::new(&session_dir).join("archive"), ".whisper_original.json")
        || has(&Path::new(&work_dir).join("temp"), ".json")
}

/// Rebuild the transcripts from the saved whisper output (`archive/`), no audio needed.
pub fn plan_reprocess(settings: &Settings, campaign: &Campaign, session: i64) -> Result<JobPlan, CampaignError> {
    if session <= 0 {
        return Err(CampaignError::new("The session number must be 1 or higher."));
    }
    let plan = base_plan(settings, campaign, session, true, Vec::new(), Vec::new())?;
    if !has_saved_output(settings, campaign, session) {
        return Err(CampaignError::new(format!(
            "There is no saved whisper output for session {session} to rebuild from \
             (looked in {}/archive).",
            plan.session_dir
        )));
    }
    Ok(plan)
}

/// Work out the whole job without touching anything.
pub fn plan_job(
    settings: &Settings,
    campaign: &Campaign,
    session: i64,
    files: &[PathBuf],
) -> Result<JobPlan, CampaignError> {
    if session <= 0 {
        return Err(CampaignError::new("The session number must be 1 or higher."));
    }
    let audio_dir = campaign.audio_dir(settings, session)?;

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

    let mut plan = base_plan(settings, campaign, session, false, moves, existing)?;
    plan.run.options.redo = plan.overwritten();
    Ok(plan)
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
    /// Audio files the run found.
    pub tracks_found: Option<usize>,
    pub succeeded: Vec<String>,
    /// File and the error message.
    pub failed: Vec<(String, String)>,
    /// Already transcribed on an earlier run, so skipped.
    pub skipped: Vec<String>,
    pub cancelled: bool,
    pub postprocess_done: bool,
    /// The run rebuilt the transcripts from saved whisper output and transcribed nothing.
    pub rebuilt: bool,
    pub transcript_md: Option<String>,
    /// Set when the job could not run at all (a move failed, a folder could not be made).
    pub failure: Option<String>,
    pub elapsed: Duration,
}

impl RunReport {
    /// Everything the run was asked to do worked.
    pub fn ok(&self) -> bool {
        self.failure.is_none() && !self.cancelled && self.failed.is_empty() && self.postprocess_done
    }

    /// The honest one-line result. The GUI said "Complete!" whatever happened.
    pub fn headline(&self) -> String {
        if let Some(f) = &self.failure {
            return f.clone();
        }
        let Some(found) = self.tracks_found else {
            return if self.cancelled {
                "Cancelled before the first track.".to_string()
            } else if self.rebuilt && self.postprocess_done {
                "Rebuilt the transcript from the saved whisper output.".to_string()
            } else if self.rebuilt {
                "Nothing to rebuild the transcript from. See the log.".to_string()
            } else {
                "No audio files found in the session folder.".to_string()
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
        rebuilt: plan.reprocess,
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

    transcribe::run(&plan.run, tx, cancel, &mut report);
    if Path::new(&plan.transcript_md).is_file() && report.postprocess_done {
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
    fn headline_counts_skipped() {
        let mut r = report();
        r.succeeded = vec!["a".into()];
        r.skipped = vec!["b".into(), "c".into(), "d".into(), "e".into()];
        assert_eq!(r.headline(), "1 of 5 transcribed, 4 already done");
        assert!(r.ok());
    }

    #[test]
    fn headline_covers_the_edges() {
        let mut r = RunReport::default();
        assert_eq!(r.headline(), "No audio files found in the session folder.");
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
        assert!(!r.ok());
    }

    #[test]
    fn a_rebuild_says_what_it_did() {
        let mut r = RunReport {
            rebuilt: true,
            postprocess_done: true,
            ..Default::default()
        };
        assert_eq!(r.headline(), "Rebuilt the transcript from the saved whisper output.");
        assert!(r.ok());
        r.postprocess_done = false;
        assert_eq!(r.headline(), "Nothing to rebuild the transcript from. See the log.");
        assert!(!r.ok());
    }
}
