//! Index to Memory and the alias report (PORTING.md sections 5, 6 and 7.3).
//!
//! Both are Python scripts run as subprocesses: the chunker (`transcript_to_chunks.py`)
//! writes the session's chunk JSONL, and the Chroma helper (`chroma_memory.py`)
//! embeds and stores it. ChromaDB and sentence-transformers have no practical Rust
//! equivalent that reads the existing store (PORTING.md section 6), so that half
//! stays Python for good; which Python is the `python` setting.
//!
//! [`plan_index`] checks everything up front. [`execute_index`] runs on its own thread
//! and reports through the same event channel as the transcription job.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::campaign::Campaign;
use crate::error::CampaignError;
use crate::job::PipelineEvent;
use crate::proc::run_streaming;
use crate::settings::{Settings, Sourced};

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
    pub scripts_dir: String,
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
    scripts: &Sourced,
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
    let needed: &[&str] = match kind {
        IndexKind::Index => &["transcript_to_chunks.py", "chroma_memory.py"],
        IndexKind::Report => &["transcript_to_chunks.py"],
    };
    for script in needed {
        if !Path::new(&scripts.value).join(script).is_file() {
            return Err(CampaignError::new(format!(
                "{script} is not in {} (from {}).",
                scripts.value, scripts.source
            )));
        }
    }
    let mut env = settings.env_vars().clone();
    env.insert("PYTHONUNBUFFERED".into(), "1".into());
    env.insert("PYTHONIOENCODING".into(), "utf-8".into());
    Ok(IndexPlan {
        kind,
        slug: campaign.slug.clone(),
        session,
        program: settings.effective_python()?,
        scripts_dir: scripts.value.clone(),
        env,
        transcript_md,
        chunks_path: campaign.chunks_path(settings, session)?,
        report_path: Path::new(&campaign.session_dir(settings, session)?)
            .join("alias_candidates.md")
            .to_string_lossy()
            .into_owned(),
        db_path: campaign.db_path(settings)?,
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

/// `3 chunks from 14 utterances (session 1); 2 alias rules applied`
pub fn parse_chunk_summary(line: &str) -> Option<(usize, usize)> {
    let (chunks, rest) = line.split_once(" chunks from ")?;
    let (utt, _) = rest.split_once(" utterances")?;
    Some((chunks.trim().parse().ok()?, utt.trim().parse().ok()?))
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
            "ChromaDB is not installed for {}. Install it with: {} -m pip install -r {}/requirements.txt \
             (or set \"python\" in the config file to a Python that has it).",
            plan.program, plan.program, plan.scripts_dir
        ));
    }
    if text.contains("No module named") {
        return Some(format!(
            "A Python module is missing for {}. See {}/requirements.txt.",
            plan.program, plan.scripts_dir
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
        |line| {
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

fn run(plan: &IndexPlan, tx: &Sender<PipelineEvent>, cancel: &AtomicBool, report: &mut IndexReport) {
    let script = |name: &str| Path::new(&plan.scripts_dir).join(name).to_string_lossy().into_owned();
    let session = plan.session.to_string();
    let mut chunker = vec![
        script("transcript_to_chunks.py"),
        "--campaign".into(),
        plan.slug.clone(),
        "--session".into(),
        session.clone(),
    ];
    match plan.kind {
        IndexKind::Report => {
            send(
                tx,
                PipelineEvent::Stage {
                    label: "Scanning the transcript for alias candidates...".into(),
                    fraction: 0.3,
                },
            );
            chunker.push("--report".into());
            if !step(plan, tx, cancel, report, chunker, "The alias report", |_, _| {}) {
                return;
            }
            if !Path::new(&plan.report_path).is_file() {
                report.failure = Some(format!(
                    "The report was not written: {} does not exist.",
                    plan.report_path
                ));
                return;
            }
            report.output = Some(plan.report_path.clone());
            send(
                tx,
                PipelineEvent::Stage {
                    label: "Report ready".into(),
                    fraction: 1.0,
                },
            );
        }
        IndexKind::Index => {
            send(
                tx,
                PipelineEvent::Stage {
                    label: "Chunking the transcript...".into(),
                    fraction: 0.1,
                },
            );
            let ok = step(plan, tx, cancel, report, chunker, "Chunking", |line, r| {
                if let Some((c, u)) = parse_chunk_summary(line) {
                    (r.chunks, r.utterances) = (Some(c), Some(u));
                }
            });
            if !ok {
                return;
            }
            match std::fs::metadata(&plan.chunks_path) {
                Ok(m) if m.len() > 0 && report.chunks != Some(0) => report.output = Some(plan.chunks_path.clone()),
                Ok(_) => {
                    report.failure = Some(format!(
                        "The chunk file is empty: {}. Nothing to index; check that the transcript has speaker lines.",
                        plan.chunks_path
                    ));
                    return;
                }
                Err(_) => {
                    report.failure = Some(format!("The chunk file was not written: {}.", plan.chunks_path));
                    return;
                }
            }
            send(
                tx,
                PipelineEvent::Stage {
                    label: "Embedding and storing (the first run loads the model, which takes a while)...".into(),
                    fraction: 0.5,
                },
            );
            let ingest = vec![
                script("chroma_memory.py"),
                "ingest".into(),
                "--campaign".into(),
                plan.slug.clone(),
                "--session".into(),
                session,
            ];
            let ok = step(plan, tx, cancel, report, ingest, "Storing in ChromaDB", |line, r| {
                if let Some((n, held)) = parse_upsert_summary(line) {
                    (r.upserted, r.collection) = (Some(n), Some(held));
                }
            });
            if !ok {
                return;
            }
            send(
                tx,
                PipelineEvent::Stage {
                    label: "Indexed".into(),
                    fraction: 1.0,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_summary_lines_are_read() {
        assert_eq!(
            parse_chunk_summary("3 chunks from 14 utterances (session 1); 2 alias rules applied"),
            Some((3, 14))
        );
        assert_eq!(parse_chunk_summary("Wrote /x/s01_chunks.jsonl"), None);
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
