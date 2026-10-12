//! Index to Memory and the alias report (PORTING.md sections 5, 6 and 7.3).
//!
//! The chunker and the alias report run in this program ([`crate::chunker`],
//! [`crate::alias_report`]). Storing the chunks is the one Python step: the Chroma
//! helper (`python/chroma_memory.py`) embeds and stores them. ChromaDB and
//! sentence-transformers have no practical Rust equivalent that reads the existing
//! store (PORTING.md section 6), so that half stays Python for good; which Python is
//! the `python` setting. The helper is built into the program and run with
//! `python -c`, so there is no file to find.
//!
//! [`plan_index`] checks everything up front. [`execute_index`] runs on its own thread
//! and reports through the same event channel as the transcription job.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::alias_report;
use crate::campaign::Campaign;
use crate::chunker;
use crate::error::CampaignError;
use crate::job::PipelineEvent;
use crate::proc::run_streaming;
use crate::pyish;
use crate::settings::Settings;

/// The Chroma helper, built into the program.
pub const CHROMA_HELPER: &str = include_str!("../../../python/chroma_memory.py");

/// What to pass to Python to run the helper with `rest` as its arguments.
pub fn helper_args(rest: &[String]) -> Vec<String> {
    let mut args = vec!["-c".to_string(), CHROMA_HELPER.to_string()];
    args.extend(rest.iter().cloned());
    args
}

/// The environment the helper runs in: the process's, with Python's output unbuffered
/// so the log shows lines as they come.
pub fn helper_env(settings: &Settings) -> HashMap<String, String> {
    let mut env = settings.env_vars().clone();
    env.insert("PYTHONUNBUFFERED".into(), "1".into());
    env.insert("PYTHONIOENCODING".into(), "utf-8".into());
    env
}

/// Where the campaign's store is. `$TTRPG_DB` wins if it is set, as it always did for
/// the helper (the original GUI passed no `--db`, so the helper read the variable);
/// otherwise the campaign's own folder under the Chroma root.
pub fn store_path(settings: &Settings, campaign: &Campaign) -> Result<String, CampaignError> {
    match settings.env("TTRPG_DB") {
        Some(p) => Ok(pyish::abspath(
            &pyish::expanduser(p, settings.env_raw("HOME")),
            &pyish::cwd(),
        )),
        None => campaign.db_path(settings),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexKind {
    /// Chunk the transcript, then embed and store the chunks.
    Index,
    /// The read-only alias-candidate report.
    Report,
}

#[derive(Debug, Clone)]
pub struct IndexPlan {
    pub kind: IndexKind,
    pub slug: String,
    pub session: i64,
    pub program: String,
    pub env: HashMap<String, String>,
    pub transcript_md: String,
    pub chunks_path: String,
    pub report_path: String,
    pub db_path: String,
}

/// Work out the job without running anything.
pub fn plan_index(
    settings: &Settings,
    campaign: &Campaign,
    session: i64,
    kind: IndexKind,
) -> Result<IndexPlan, CampaignError> {
    if session <= 0 {
        return Err(CampaignError::new("The session number must be 1 or higher."));
    }
    let transcript_md = campaign.transcript_path(settings, session)?;
    if !Path::new(&transcript_md).is_file() {
        return Err(CampaignError::new(format!(
            "There is no transcript for session {session} yet: {transcript_md} does not exist."
        )));
    }
    Ok(IndexPlan {
        kind,
        slug: campaign.slug.clone(),
        session,
        program: settings.effective_python()?,
        env: helper_env(settings),
        transcript_md,
        chunks_path: campaign.chunks_path(settings, session)?,
        report_path: Path::new(&campaign.session_dir(settings, session)?)
            .join("alias_candidates.md")
            .to_string_lossy()
            .into_owned(),
        db_path: store_path(settings, campaign)?,
    })
}

/// How an index or report run ended.
#[derive(Debug, Clone)]
pub struct IndexReport {
    pub kind: IndexKind,
    pub session: i64,
    pub cancelled: bool,
    /// Why it did not work, with the helper's last words and what to do about it.
    pub failure: Option<String>,
    pub chunks: Option<usize>,
    pub utterances: Option<usize>,
    pub upserted: Option<usize>,
    pub collection: Option<usize>,
    /// The chunk file or the report, when written.
    pub output: Option<String>,
    pub db_path: String,
    pub elapsed: Duration,
}

impl IndexReport {
    fn new(plan: &IndexPlan) -> Self {
        Self {
            kind: plan.kind,
            session: plan.session,
            cancelled: false,
            failure: None,
            chunks: None,
            utterances: None,
            upserted: None,
            collection: None,
            output: None,
            db_path: plan.db_path.clone(),
            elapsed: Duration::ZERO,
        }
    }

    /// A run that could not happen at all.
    pub fn failed(kind: IndexKind, session: i64, why: impl Into<String>) -> Self {
        Self {
            kind,
            session,
            cancelled: false,
            failure: Some(why.into()),
            chunks: None,
            utterances: None,
            upserted: None,
            collection: None,
            output: None,
            db_path: String::new(),
            elapsed: Duration::ZERO,
        }
    }

    pub fn ok(&self) -> bool {
        self.failure.is_none() && !self.cancelled
    }

    /// The honest one-line result.
    pub fn headline(&self) -> String {
        if let Some(f) = &self.failure {
            return f.lines().next().unwrap_or(f).to_string();
        }
        if self.cancelled {
            return match self.kind {
                IndexKind::Index => "Indexing was cancelled. Nothing was added to the store.".into(),
                IndexKind::Report => "The report was cancelled.".into(),
            };
        }
        match self.kind {
            IndexKind::Report => "Alias report ready. Nothing was written to canon_aliases.json.".into(),
            IndexKind::Index => {
                let mut s = match (self.upserted, self.chunks) {
                    (Some(n), _) | (None, Some(n)) => format!("Indexed {n} chunks of session {}", self.session),
                    _ => format!("Indexed session {}", self.session),
                };
                if let Some(m) = self.collection {
                    s.push_str(&format!("; the store now holds {m}"));
                }
                s.push('.');
                s
            }
        }
    }
}

/// `Upserted 3 chunks into /db. Collection now holds 7.`
pub fn parse_upsert_summary(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("Upserted ")?;
    let (n, rest) = rest.split_once(" chunks into ")?;
    let (_, held) = rest.rsplit_once("Collection now holds ")?;
    Some((n.trim().parse().ok()?, held.trim().trim_end_matches('.').parse().ok()?))
}

/// What to tell the person when the helper's output points at a known cause.
fn hint_for(output: &[String], plan: &IndexPlan) -> Option<String> {
    let text = output.join("\n");
    if text.contains("No module named 'chromadb'")
        || text.contains("No module named 'sentence_transformers'")
        || text.contains("No module named \"chromadb\"")
    {
        return Some(format!(
            "ChromaDB is not installed for {}. Install it with: {} -m pip install chromadb sentence-transformers \
             (or set \"python\" in the config file to a Python that has it).",
            plan.program, plan.program
        ));
    }
    if text.contains("No module named") {
        return Some(format!(
            "A Python module is missing for {}. The Chroma helper needs chromadb and sentence-transformers.",
            plan.program
        ));
    }
    None
}

fn send(tx: &Sender<PipelineEvent>, ev: PipelineEvent) {
    let _ = tx.send(ev);
}

/// Run to the end and send [`PipelineEvent::IndexFinished`] last.
pub fn execute_index(plan: IndexPlan, tx: Sender<PipelineEvent>, cancel: Arc<AtomicBool>) {
    let started = Instant::now();
    let mut report = IndexReport::new(&plan);
    run(&plan, &tx, &cancel, &mut report);
    report.elapsed = started.elapsed();
    send(&tx, PipelineEvent::IndexFinished(Box::new(report)));
}

/// One script step: returns the last lines of its output, or ends the run.
fn step(
    plan: &IndexPlan,
    tx: &Sender<PipelineEvent>,
    cancel: &AtomicBool,
    report: &mut IndexReport,
    args: Vec<String>,
    what: &str,
    mut on_line: impl FnMut(&str, &mut IndexReport),
) -> bool {
    let mut tail: Vec<String> = Vec::new();
    let outcome = run_streaming(
        &plan.program,
        &args,
        &plan.env,
        cancel,
        |_, line| {
            on_line(&line, report);
            tail.push(line.clone());
            if tail.len() > 12 {
                tail.remove(0);
            }
            send(tx, PipelineEvent::Log(line));
        },
        || send(tx, PipelineEvent::Log("Cancelling: stopping the helper...".into())),
    );
    match outcome {
        Err(e) => {
            report.failure = Some(format!(
                "Could not start {}: {e}. Set \"python\" in the config file to the Python that has ChromaDB.",
                plan.program
            ));
            false
        }
        Ok(o) if o.cancelled => {
            report.cancelled = true;
            false
        }
        Ok(o) if o.exit_code != Some(0) => {
            let code = o.exit_code.map_or("none".to_string(), |c| c.to_string());
            let mut msg = format!("{what} failed (exit code {code}).");
            if let Some(h) = hint_for(&tail, plan) {
                msg.push('\n');
                msg.push_str(&h);
            } else if let Some(last) = tail.iter().rev().find(|l| !l.trim().is_empty()) {
                msg.push_str(&format!(" Last line: {last}"));
            }
            report.failure = Some(msg);
            false
        }
        Ok(_) => true,
    }
}

fn read_transcript(path: &str) -> Result<String, String> {
    std::fs::read(path)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| format!("Could not read {path}: {e}"))
}

fn write_file(path: &str, text: &str) -> Result<(), String> {
    if let Some(dir) = Path::new(path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("Could not write {path}: {e}"))
}

fn run(plan: &IndexPlan, tx: &Sender<PipelineEvent>, cancel: &AtomicBool, report: &mut IndexReport) {
    let log = |s: String| send(tx, PipelineEvent::Log(s));
    let stage = |label: &str, fraction: f64| {
        send(
            tx,
            PipelineEvent::Stage {
                label: label.into(),
                fraction,
            },
        )
    };
    // The campaign again, from disk, as this run's settings see it.
    let settings = Settings::from_env_map(plan.env.clone(), "ttrpg-tui");
    let campaign = match Campaign::load(&settings, &plan.slug) {
        Ok(c) => c,
        Err(e) => {
            report.failure = Some(e.0);
            return;
        }
    };
    let raw = match read_transcript(&plan.transcript_md) {
        Ok(r) => r,
        Err(e) => {
            report.failure = Some(e);
            return;
        }
    };
    match plan.kind {
        IndexKind::Report => {
            stage("Scanning the transcript for alias candidates...", 0.3);
            let dictionary = alias_report::load_dictionary(&alias_report::WORDLISTS);
            let text = alias_report::alias_report(
                &campaign,
                plan.session,
                &plan.transcript_md,
                &raw,
                alias_report::DEFAULT_THRESHOLD,
                alias_report::DEFAULT_MIN_FREQ,
                &dictionary,
            );
            if let Err(e) = write_file(&plan.report_path, &text) {
                report.failure = Some(e);
                return;
            }
            log(format!("Alias candidate report written to {}", plan.report_path));
            report.output = Some(plan.report_path.clone());
            stage("Report ready", 1.0);
        }
        IndexKind::Index => {
            stage("Chunking the transcript...", 0.1);
            let run = chunker::chunk_transcript(&campaign, &raw, plan.session, &crate::timefmt::today_local());
            log(run.summary(plan.session));
            if run.uncovered_lines > 0 {
                log(format!(
                    "WARNING: {} line(s) of the transcript are not part of any speaker entry and were not indexed \
                     (text above the first speaker line, or a line that starts like a header but is not \
                     **[hh:mm:ss] Name:**).",
                    run.uncovered_lines
                ));
            }
            (report.chunks, report.utterances) = (Some(run.chunks), Some(run.utterances));
            if run.chunks == 0 {
                // Written anyway, as the original did, so the file is not stale.
                let _ = write_file(&plan.chunks_path, "");
                report.failure = Some(format!(
                    "The chunk file is empty: {}. Nothing to index; check that the transcript has speaker lines.",
                    plan.chunks_path
                ));
                return;
            }
            if let Err(e) = write_file(&plan.chunks_path, &run.jsonl) {
                report.failure = Some(e);
                return;
            }
            log(format!("Wrote {}", plan.chunks_path));
            report.output = Some(plan.chunks_path.clone());
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                report.cancelled = true;
                return;
            }

            stage(
                "Embedding and storing (the first run loads the model, which takes a while)...",
                0.5,
            );
            let ingest = helper_args(&[
                "ingest".into(),
                "--db".into(),
                plan.db_path.clone(),
                plan.chunks_path.clone(),
            ]);
            let ok = step(plan, tx, cancel, report, ingest, "Storing in ChromaDB", |line, r| {
                if let Some((n, held)) = parse_upsert_summary(line) {
                    (r.upserted, r.collection) = (Some(n), Some(held));
                }
            });
            if !ok {
                return;
            }
            stage("Indexed", 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_helper_is_plain_ascii_so_it_survives_any_locale_on_the_command_line() {
        assert!(CHROMA_HELPER.is_ascii());
        assert!(CHROMA_HELPER.contains("def main(argv=None):"));
        assert!(
            !CHROMA_HELPER.contains("import campaign"),
            "it must not need the original scripts"
        );
        let args = helper_args(&["info".into(), "--db".into(), "/x".into()]);
        assert_eq!(
            (args[0].as_str(), args[2].as_str(), args[4].as_str()),
            ("-c", "info", "/x")
        );
    }

    #[test]
    fn the_helpers_summary_lines_are_read() {
        assert_eq!(
            parse_upsert_summary("Upserted 3 chunks into /home/a b/db. Collection now holds 7."),
            Some((3, 7))
        );
        assert_eq!(parse_upsert_summary("Upserted lots"), None);
    }

    fn report(kind: IndexKind) -> IndexReport {
        IndexReport {
            kind,
            session: 4,
            cancelled: false,
            failure: None,
            chunks: Some(3),
            utterances: Some(14),
            upserted: Some(3),
            collection: Some(7),
            output: None,
            db_path: "/db".into(),
            elapsed: Duration::ZERO,
        }
    }

    #[test]
    fn headlines_say_what_happened() {
        assert_eq!(
            report(IndexKind::Index).headline(),
            "Indexed 3 chunks of session 4; the store now holds 7."
        );
        assert_eq!(
            report(IndexKind::Report).headline(),
            "Alias report ready. Nothing was written to canon_aliases.json."
        );
        let mut r = report(IndexKind::Index);
        r.cancelled = true;
        assert!(r.headline().contains("cancelled") && !r.ok());
        let mut r = report(IndexKind::Index);
        r.failure = Some("Storing in ChromaDB failed (exit code 1).\nChromaDB is not installed.".into());
        assert_eq!(r.headline(), "Storing in ChromaDB failed (exit code 1).");
    }
}
