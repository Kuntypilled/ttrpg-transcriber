//! The transcription run in Rust, replacing
//! `transcribe.sh`: for each audio file, ffmpeg to a 16 kHz mono WAV, whisper-cli,
//! the raw JSON archived, the segments filtered and collapsed; then the merged,
//! consolidated and Markdown transcripts. The files it writes (the log and state CSVs,
//! `archive/`, `temp/*.processed.tsv`, the three transcripts) are the ones the script
//! wrote, so a session started by one can be finished by the other.
//!
//! Progress is typed events on a channel. Unlike the script, it also reads whisper's
//! own segment lines to say how far through the current file it is.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Instant;

use crate::audio::{list_audio_files, resolve_base, resolve_speaker};
use crate::campaign::Campaign;
use crate::job::{PipelineEvent, RunReport};
use crate::pipeline::{self, PostOptions};
use crate::proc::{Stream, run_streaming};
use crate::progress::{ProgressEvent, whisper_segment_end_ms};
use crate::state::read_state_rows;
use crate::stock::StockRule;
use crate::timefmt;

/// Everything about how a run behaves, resolved before it starts.
#[derive(Debug, Clone)]
pub struct Options {
    /// Transcribe again files that already succeeded.
    pub force: bool,
    /// Files to transcribe again whatever the state file says: the ones just moved in over
    /// an older file of the same name, whose old result no longer describes them.
    pub redo: Vec<String>,
    /// Skip transcription: rebuild the transcripts from the saved whisper output.
    pub post_process_only: bool,
    /// Remove `temp/` afterwards.
    pub cleanup: bool,
    pub ignore_words: Vec<String>,
    pub stock: StockRule,
    pub whisper_bin: String,
    pub model: String,
    /// whisper-cli's `-dev`.
    pub device: Option<u32>,
    /// whisper-cli's `-mc`; -1 leaves whisper's own default.
    pub max_context: i64,
    pub ffmpeg: String,
}

/// The folders and settings of one run.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub campaign: Campaign,
    pub audio_dir: String,
    pub session_dir: String,
    pub temp_dir: String,
    pub options: Options,
    /// The environment the children run in.
    pub env: HashMap<String, String>,
}

impl RunConfig {
    fn archive_dir(&self) -> PathBuf {
        Path::new(&self.session_dir).join("archive")
    }
    fn log_path(&self) -> PathBuf {
        Path::new(&self.session_dir).join("transcription_log.csv")
    }
    pub fn state_path(&self) -> PathBuf {
        Path::new(&self.session_dir).join("transcription_state.csv")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Info,
    Warning,
    Error,
}

struct Run<'a> {
    cfg: &'a RunConfig,
    tx: &'a Sender<PipelineEvent>,
    cancel: &'a AtomicBool,
    offset: i64,
}

enum Track {
    Done,
    Failed(String),
    Cancelled,
}

/// `YYYY-MM-DD HH:MM:SS` now, in the local zone.
fn now(offset: i64) -> String {
    timefmt::format_timestamp(timefmt::unix_now(), offset)
}

fn append(path: &Path, text: &str) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// Seconds of audio in a WAV file, from its header; `None` if it is not one.
pub fn wav_seconds(path: &Path) -> Option<f64> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let (mut pos, mut byte_rate) = (12usize, 0u32);
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        if id == b"fmt " && pos + 16 <= bytes.len() {
            byte_rate = u32::from_le_bytes(bytes[pos + 16..pos + 20].try_into().ok()?);
        } else if id == b"data" {
            // A streamed file may carry an unfinished size; the file length is then the truth.
            let avail = bytes.len() - (pos + 8);
            let n = if size == 0 || size > avail { avail } else { size };
            return (byte_rate > 0).then(|| n as f64 / f64::from(byte_rate));
        }
        pos += 8 + size + (size & 1);
    }
    None
}

impl Run<'_> {
    fn emit(&self, ev: PipelineEvent) {
        let _ = self.tx.send(ev);
    }

    fn say(&self, line: impl Into<String>) {
        self.emit(PipelineEvent::Log(line.into()));
    }

    fn log(&self, level: Level, msg: &str) {
        let tag = match level {
            Level::Info => "INFO",
            Level::Warning => "WARNING",
            Level::Error => "ERROR",
        };
        append(&self.cfg.log_path(), &format!("{},{tag},{msg}\n", now(self.offset)));
        self.say(match level {
            Level::Info => format!("  {msg}"),
            Level::Warning => format!("  WARNING: {msg}"),
            Level::Error => format!("  ERROR: {msg}"),
        });
    }

    /// A line of ffmpeg's or whisper's stderr, into the log file as it came.
    fn raw_log(&self, line: &str) {
        append(&self.cfg.log_path(), &format!("{line}\n"));
    }

    fn record_state(&self, file: &str, size: u64, secs: u64, status: &str, player: &str, error: &str) {
        let q = |s: &str| s.replace('"', "'");
        append(
            &self.cfg.state_path(),
            &format!(
                "\"{}\",\"{size}\",\"{secs}\",\"{status}\",\"{}\",\"{}\",\"{}\"\n",
                q(file),
                now(self.offset),
                q(player),
                q(error)
            ),
        );
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn setup(&self) -> Result<(), String> {
        for dir in [&self.cfg.temp_dir, &self.cfg.session_dir] {
            fs::create_dir_all(dir).map_err(|e| format!("Could not create {dir}: {e}"))?;
        }
        let archive = self.cfg.archive_dir();
        fs::create_dir_all(&archive).map_err(|e| format!("Could not create {}: {e}", archive.display()))?;
        for (path, header) in [
            (self.cfg.log_path(), "Timestamp,Level,Message\n"),
            (
                self.cfg.state_path(),
                "FileName,FileSize,ProcessingTime,Status,Timestamp,PlayerName,ErrorMessage\n",
            ),
        ] {
            if !path.exists() {
                fs::write(&path, header).map_err(|e| format!("Could not create {}: {e}", path.display()))?;
            }
        }
        Ok(())
    }

    fn banner(&self) {
        let o = &self.cfg.options;
        let c = &self.cfg;
        self.say("");
        self.say("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        self.say(format!("  Session Transcription — {}", c.campaign.slug));
        self.say("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        self.log(Level::Info, &format!("Campaign : {}", c.campaign.slug));
        self.log(Level::Info, &format!("Aliases  : {}", c.campaign.aliases_path()));
        self.log(Level::Info, &format!("Input    : {}", c.audio_dir));
        self.log(Level::Info, &format!("Output   : {}", c.session_dir));
        self.log(Level::Info, &format!("Temp     : {}", c.temp_dir));
        self.log(Level::Info, &format!("Model    : {}", o.model));
        self.log(
            Level::Info,
            &format!(
                "Force    : {} | PostOnly: {} | Cleanup: {}",
                o.force, o.post_process_only, o.cleanup
            ),
        );
        self.log(Level::Info, &format!("Filter   : {}", o.ignore_words.join(" ")));
        if o.stock.enabled() {
            self.log(
                Level::Info,
                &format!(
                    "Stock    : {} when {} s or more",
                    o.stock.phrases.join(","),
                    o.stock.min_seconds
                ),
            );
        } else {
            self.log(Level::Info, "Stock    : off");
        }
        if !o.post_process_only {
            let device = o
                .device
                .map_or("whisper-cli's default".to_string(), |d| format!("device {d}"));
            self.log(Level::Info, &format!("Whisper  : {} ({device})", o.whisper_bin));
        }
        self.say("");
    }

    fn post_options(&self) -> PostOptions {
        PostOptions {
            ignore_words: self.cfg.options.ignore_words.clone(),
            stock: self.cfg.options.stock.clone(),
        }
    }

    /// Filter and collapse one track's segments, write its `.processed.tsv`.
    fn process_track(&self, base: &str, speaker: &str, json: &str) -> Result<(), String> {
        let segments = pipeline::segments_from_json(json)?;
        let processed = pipeline::process_segments(speaker, &segments, &self.post_options());
        let file = format!("{base}.processed.tsv");
        fs::write(
            Path::new(&self.cfg.temp_dir).join(&file),
            pipeline::rows_to_tsv(&processed.rows),
        )
        .map_err(|e| format!("Could not write {file}: {e}"))?;
        self.log(Level::Info, &format!("  {}", processed.log_line(&file)));
        Ok(())
    }

    fn transcribe_file(&self, audio: &Path, index: usize, total: usize) -> Track {
        let o = &self.cfg.options;
        let filename = audio
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let base = filename
            .rsplit_once('.')
            .map_or(filename.as_str(), |(b, _)| b)
            .to_string();
        let player = resolve_speaker(&self.cfg.campaign, &filename).label();
        let size = fs::metadata(audio).map_or(0, |m| m.len());
        let started = Instant::now();
        let secs = |t: &Instant| t.elapsed().as_secs();
        self.log(
            Level::Info,
            &format!("Transcribing: {filename} — speaker: {player} ({} MB)", size / 1_048_576),
        );

        let temp = Path::new(&self.cfg.temp_dir);
        let raw_json = temp.join(format!("{base}.json"));
        let wav = temp.join(format!("{base}_input.wav"));
        let _ = fs::remove_file(&raw_json);

        // whisper-cli's own decoder fails on some MP3/OGG; ffmpeg reads everything, and 16 kHz
        // mono WAV is what whisper wants anyway.
        self.log(Level::Info, "  Converting to WAV via ffmpeg...");
        let ffmpeg_args: Vec<String> = [
            "-loglevel",
            "warning",
            "-y",
            "-i",
            &audio.to_string_lossy(),
            "-ar",
            "16000",
            "-ac",
            "1",
            "-f",
            "wav",
            &wav.to_string_lossy(),
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let ff = run_streaming(
            &o.ffmpeg,
            &ffmpeg_args,
            &self.cfg.env,
            self.cancel,
            |_, line| self.raw_log(&line),
            || self.say("Cancelling: stopping ffmpeg..."),
        );
        match ff {
            Ok(out) if out.cancelled => {
                let _ = fs::remove_file(&wav);
                return Track::Cancelled;
            }
            Ok(out) if out.exit_code == Some(0) => {}
            other => {
                let _ = fs::remove_file(&wav);
                if let Err(e) = other {
                    self.log(Level::Error, &format!("Could not start {}: {e}", o.ffmpeg));
                }
                self.log(Level::Error, &format!("ffmpeg conversion failed for: {filename}"));
                self.record_state(
                    &filename,
                    size,
                    secs(&started),
                    "Error",
                    &player,
                    "ffmpeg conversion failed",
                );
                return Track::Failed("ffmpeg conversion failed".into());
            }
        }

        let duration_ms = wav_seconds(&wav).map(|s| (s * 1000.0) as u64).filter(|d| *d > 0);
        // -mc 0: no context from earlier segments, which keeps a hallucination from carrying
        // across a silence. -of: this whisper-cli build takes a file prefix, not a folder.
        let mut args: Vec<String> = vec![
            "-m".into(),
            o.model.clone(),
            "-l".into(),
            "en".into(),
            "-of".into(),
            temp.join(&base).to_string_lossy().into_owned(),
            "-oj".into(),
        ];
        if o.max_context >= 0 {
            args.extend(["-mc".into(), o.max_context.to_string()]);
        }
        let prompt = self.cfg.campaign.seed_prompt();
        if !prompt.is_empty() {
            args.extend(["--prompt".into(), prompt]);
        }
        if let Some(d) = o.device {
            args.extend(["-dev".into(), d.to_string()]);
        }
        args.push(wav.to_string_lossy().into_owned());

        let mut last_permille = u16::MAX;
        let run = run_streaming(
            &o.whisper_bin,
            &args,
            &self.cfg.env,
            self.cancel,
            |stream, line| match stream {
                Stream::Err => self.raw_log(&line),
                Stream::Out => {
                    if let (Some(end), Some(total_ms)) = (whisper_segment_end_ms(&line), duration_ms) {
                        let permille = (end.saturating_mul(1000) / total_ms).min(1000) as u16;
                        if permille != last_permille {
                            last_permille = permille;
                            self.emit(PipelineEvent::Progress(ProgressEvent::InFile {
                                index,
                                total,
                                permille,
                            }));
                        }
                    }
                    self.say(line);
                }
            },
            || self.say("Cancelling: stopping whisper-cli..."),
        );
        let _ = fs::remove_file(&wav);
        match run {
            Ok(out) if out.cancelled => return Track::Cancelled,
            Ok(out) if out.exit_code == Some(0) => {}
            other => {
                if let Err(e) = other {
                    self.log(Level::Error, &format!("Could not start {}: {e}", o.whisper_bin));
                }
                self.log(Level::Error, &format!("whisper-cli failed on: {filename}"));
                self.record_state(
                    &filename,
                    size,
                    secs(&started),
                    "Error",
                    &player,
                    "whisper-cli exited non-zero",
                );
                return Track::Failed("whisper-cli exited non-zero".into());
            }
        }
        if !raw_json.is_file() {
            self.log(
                Level::Error,
                &format!("Expected JSON not found after transcription: {}", raw_json.display()),
            );
            self.record_state(&filename, size, secs(&started), "Error", &player, "Output JSON missing");
            return Track::Failed("Output JSON missing".into());
        }
        let json = match fs::read(&raw_json) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) => {
                let msg = format!("Could not read whisper's output: {e}");
                self.log(Level::Error, &msg);
                self.record_state(&filename, size, secs(&started), "Error", &player, &msg);
                return Track::Failed(msg);
            }
        };
        // The raw output is the one thing that cannot be rebuilt once the audio is gone.
        let archived = self.cfg.archive_dir().join(format!("{base}.whisper_original.json"));
        if let Err(e) = fs::copy(&raw_json, &archived) {
            self.log(
                Level::Warning,
                &format!("Could not archive {}: {e}", archived.display()),
            );
        }
        if let Err(msg) = self.process_track(&base, &player, &json) {
            let msg = format!("Unreadable whisper output: {msg}");
            self.log(Level::Error, &msg);
            self.record_state(&filename, size, secs(&started), "Error", &player, &msg);
            return Track::Failed(msg);
        }
        self.record_state(&filename, size, secs(&started), "Success", &player, "");
        self.log(Level::Info, &format!("Done: {filename} in {}s", secs(&started)));
        Track::Done
    }

    /// `-p`: rebuild every track from the saved whisper JSON, no audio needed.
    fn reprocess(&self) -> usize {
        self.log(Level::Info, "Post-process only — re-processing archived whisper output");
        let temp = Path::new(&self.cfg.temp_dir);
        // Stale ones would mix old settings with new.
        for f in list_files(temp, ".processed.tsv") {
            let _ = fs::remove_file(temp.join(f));
        }
        let mut sources: Vec<(String, PathBuf)> = Vec::new();
        for f in list_files(&self.cfg.archive_dir(), ".whisper_original.json") {
            let base = f.trim_end_matches(".whisper_original.json").to_string();
            sources.push((base, self.cfg.archive_dir().join(&f)));
        }
        for f in list_files(temp, ".json") {
            let base = f.trim_end_matches(".json").to_string();
            if !sources.iter().any(|(b, _)| *b == base) {
                sources.push((base, temp.join(&f)));
            }
        }
        sources.sort();
        if sources.is_empty() {
            self.log(Level::Warning, "No whisper JSON files found in archive/ or temp/");
            return 0;
        }
        self.log(
            Level::Info,
            &format!("Found {} whisper JSON file(s) to reprocess", sources.len()),
        );
        let mut done = 0;
        for (base, path) in sources {
            let player = resolve_base(&self.cfg.campaign, &base).label();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.log(Level::Info, &format!("Reprocessing: {name} → speaker: {player}"));
            let json = fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned());
            match json
                .map_err(|e| e.to_string())
                .and_then(|j| self.process_track(&base, &player, &j))
            {
                Ok(()) => done += 1,
                Err(e) => self.log(Level::Error, &format!("Could not reprocess {name}: {e}")),
            }
        }
        done
    }

    /// Merge, consolidate, write Markdown. False when there was nothing to merge.
    fn assemble(&self) -> bool {
        let temp = Path::new(&self.cfg.temp_dir);
        let mut names = list_files(temp, ".processed.tsv");
        names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        if names.is_empty() {
            self.log(Level::Warning, "No processed TSV files found to aggregate");
            return false;
        }
        let texts: Vec<String> = names
            .iter()
            .map(|n| {
                fs::read(temp.join(n))
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default()
            })
            .collect();
        let out = pipeline::assemble(&texts);
        for line in out.log_lines(names.len()) {
            self.log(Level::Info, &line);
        }
        let dir = Path::new(&self.cfg.session_dir);
        for (file, text) in [
            ("merged_transcript.tsv", &out.merged),
            ("consolidated_transcript.tsv", &out.consolidated),
            ("transcript.md", &out.markdown),
        ] {
            if let Err(e) = fs::write(dir.join(file), text) {
                self.log(Level::Error, &format!("Could not write {file}: {e}"));
                return false;
            }
        }
        true
    }

    fn summary(&self) {
        let rows = read_state_rows(&self.cfg.state_path().to_string_lossy());
        let ok = rows.iter().filter(|r| r.succeeded()).count();
        let errors = rows.iter().filter(|r| r.status == "Error").count();
        self.say("");
        self.say("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        self.say("  Transcription Summary");
        self.say("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        self.say(format!("  Total processed : {}", rows.len()));
        self.say(format!("  Success         : {ok}"));
        self.say(format!("  Errors          : {errors}"));
        self.say(format!("  Output folder   : {}", self.cfg.session_dir));
        self.say("");
    }
}

/// File names in `dir` ending in `suffix`.
fn list_files(dir: &Path, suffix: &str) -> Vec<String> {
    fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(suffix))
                .collect()
        })
        .unwrap_or_default()
}

/// Run the whole thing, filling `report` as it goes.
pub fn run(cfg: &RunConfig, tx: &Sender<PipelineEvent>, cancel: &AtomicBool, report: &mut RunReport) {
    let run = Run {
        cfg,
        tx,
        cancel,
        offset: timefmt::local_offset_secs(),
    };
    if let Err(e) = run.setup() {
        report.failure = Some(e);
        return;
    }
    run.banner();

    if cfg.options.post_process_only {
        run.reprocess();
    } else {
        let files = list_audio_files(&cfg.audio_dir);
        if files.is_empty() {
            run.log(
                Level::Warning,
                &format!("No supported audio files found in {}", cfg.audio_dir),
            );
            return;
        }
        let already: Vec<String> = read_state_rows(&cfg.state_path().to_string_lossy())
            .into_iter()
            .filter(|r| r.succeeded())
            .map(|r| r.file)
            .collect();
        let total = files.len();
        run.log(Level::Info, &format!("Found {total} audio file(s)"));
        report.tracks_found = Some(total);
        run.emit(PipelineEvent::Progress(ProgressEvent::Start { total }));
        run.say("");
        for (i, name) in files.iter().enumerate() {
            let index = i + 1;
            if run.cancelled() {
                report.cancelled = true;
                return;
            }
            run.emit(PipelineEvent::Progress(ProgressEvent::FileStart {
                index,
                total,
                name: name.clone(),
            }));
            if !cfg.options.force && !cfg.options.redo.contains(name) && already.contains(name) {
                run.log(Level::Info, &format!("Skipping (already processed): {name}"));
                report.skipped.push(name.clone());
            } else {
                if !cfg.options.force && already.contains(name) {
                    run.log(
                        Level::Info,
                        &format!("Transcribing again (the file was replaced): {name}"),
                    );
                }
                match run.transcribe_file(&Path::new(&cfg.audio_dir).join(name), index, total) {
                    Track::Done => report.succeeded.push(name.clone()),
                    Track::Failed(why) => report.failed.push((name.clone(), why)),
                    Track::Cancelled => {
                        report.cancelled = true;
                        return;
                    }
                }
            }
            run.emit(PipelineEvent::Progress(ProgressEvent::FileDone {
                index,
                total,
                name: name.clone(),
            }));
            run.say("");
        }
        run.emit(PipelineEvent::Progress(ProgressEvent::TranscribeComplete));
    }

    run.say("");
    run.log(Level::Info, "Running post-processing pipeline...");
    report.postprocess_done = run.assemble();
    run.emit(PipelineEvent::Progress(ProgressEvent::PostDone));
    if cfg.options.cleanup {
        run.log(Level::Info, "Removing temp/ directory");
        let _ = fs::remove_dir_all(&cfg.temp_dir);
    }
    run.summary();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(data_len: u32, declared: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(b"RIFF");
        b.extend(36u32.wrapping_add(declared).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes()); // PCM
        b.extend(1u16.to_le_bytes()); // mono
        b.extend(16_000u32.to_le_bytes());
        b.extend(32_000u32.to_le_bytes()); // byte rate
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend(declared.to_le_bytes());
        b.extend(vec![0u8; data_len as usize]);
        b
    }

    #[test]
    fn the_length_of_a_wav_comes_from_its_header_or_its_size() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("a.wav");
        fs::write(&p, wav(64_000, 64_000)).unwrap();
        assert_eq!(wav_seconds(&p), Some(2.0));
        // A streamed file whose size was never filled in: use what is there.
        fs::write(&p, wav(96_000, 0)).unwrap();
        assert_eq!(wav_seconds(&p), Some(3.0));
        fs::write(&p, wav(32_000, u32::MAX)).unwrap();
        assert_eq!(wav_seconds(&p), Some(1.0));
        fs::write(&p, b"not a wav file at all, but long enough to have a header, honestly").unwrap();
        assert_eq!(wav_seconds(&p), None);
        assert_eq!(wav_seconds(&dir.path().join("missing.wav")), None);
    }
}
