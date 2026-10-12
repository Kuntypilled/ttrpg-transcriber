//! The `PROGRESS:` lines `transcribe.sh` prints (PORTING.md section 4.9) as typed
//! events, and the progress-bar arithmetic the GUI used.

/// One machine-readable line from the script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEvent {
    /// `PROGRESS:start:<N>`: N audio files found.
    Start { total: usize },
    /// `PROGRESS:file:<i>/<N>:<filename>`: starting file i.
    FileStart { index: usize, total: usize, name: String },
    /// `PROGRESS:done:<i>/<N>:<filename>`: file i finished. Also sent for skipped files.
    FileDone { index: usize, total: usize, name: String },
    /// `PROGRESS:complete`: the transcription loop finished.
    TranscribeComplete,
    /// `PROGRESS:postdone`: merge, consolidate and markdown finished.
    PostDone,
}

/// Parse one output line. Anything that is not a well-formed progress line is `None`
/// and belongs in the log. The file name may itself contain colons.
pub fn parse_progress_line(line: &str) -> Option<ProgressEvent> {
    let rest = line.strip_prefix("PROGRESS:")?;
    let mut parts = rest.splitn(3, ':');
    let tag = parts.next()?;
    let position = |s: &str| -> Option<(usize, usize)> {
        let (i, n) = s.split_once('/')?;
        Some((i.trim().parse().ok()?, n.trim().parse().ok()?))
    };
    match tag {
        "start" => Some(ProgressEvent::Start {
            total: parts.next()?.trim().parse().ok()?,
        }),
        "file" | "done" => {
            let (index, total) = position(parts.next()?)?;
            let name = parts.next().unwrap_or("").to_string();
            Some(if tag == "file" {
                ProgressEvent::FileStart { index, total, name }
            } else {
                ProgressEvent::FileDone { index, total, name }
            })
        }
        "complete" => Some(ProgressEvent::TranscribeComplete),
        "postdone" => Some(ProgressEvent::PostDone),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Moving,
    Transcribing,
    PostProcessing,
    /// Indexing or reporting: not a transcription phase.
    Working,
    Finished,
}

/// Turns events into the bar position and label. The mapping is the GUI's:
/// 0.04 moving files, 0.08 on start, `0.08 + 0.82 * (i - 1) / N` when file i
/// starts, `0.08 + 0.82 * i / N` when it ends, 0.92 post-processing, 1.0 done.
#[derive(Debug, Clone)]
pub struct Tracker {
    pub phase: Phase,
    pub total: usize,
    pub index: usize,
    pub name: String,
    fraction: f64,
    label: String,
}

impl Default for Tracker {
    fn default() -> Self {
        Tracker {
            phase: Phase::Idle,
            total: 0,
            index: 0,
            name: String::new(),
            fraction: 0.0,
            label: "Ready".into(),
        }
    }
}

impl Tracker {
    pub fn fraction(&self) -> f64 {
        self.fraction
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn moving(&mut self, index: usize, total: usize, name: &str) {
        self.phase = Phase::Moving;
        self.fraction = 0.04;
        self.label = format!("Moving file {index} / {total}: {name}");
    }

    pub fn starting(&mut self) {
        self.phase = Phase::Transcribing;
        self.fraction = 0.08;
        self.label = "Starting...".into();
    }

    /// A step of another kind of job (indexing): the bar goes to `fraction` with this label.
    pub fn stage(&mut self, label: &str, fraction: f64) {
        self.phase = if fraction >= 1.0 {
            Phase::Finished
        } else {
            Phase::Working
        };
        self.fraction = fraction.clamp(0.0, 1.0);
        self.label = label.to_string();
    }

    pub fn apply(&mut self, ev: &ProgressEvent) {
        match ev {
            ProgressEvent::Start { total } => {
                self.phase = Phase::Transcribing;
                self.total = *total;
                self.fraction = 0.08;
                self.label = format!("Transcribing 0 / {total}...");
            }
            ProgressEvent::FileStart { index, total, name } => {
                self.phase = Phase::Transcribing;
                (self.index, self.total, self.name) = (*index, *total, name.clone());
                self.fraction = 0.08 + 0.82 * (*index as f64 - 1.0) / (*total).max(1) as f64;
                self.label = format!("File {index} / {total}: {name}");
            }
            ProgressEvent::FileDone { index, total, .. } => {
                self.fraction = 0.08 + 0.82 * *index as f64 / (*total).max(1) as f64;
                self.label = format!("Done {index} / {total} files");
            }
            ProgressEvent::TranscribeComplete => {
                self.phase = Phase::PostProcessing;
                self.fraction = 0.92;
                self.label = "Merging and formatting...".into();
            }
            ProgressEvent::PostDone => {
                self.phase = Phase::Finished;
                self.fraction = 1.0;
                self.label = "Complete".into();
            }
        }
        self.fraction = self.fraction.clamp(0.0, 1.0);
    }
}

/// True for a segment line whisper-cli prints to stdout while it works:
/// `[00:00:30.000 --> 00:01:00.000]   Thank you.` The script does not redirect
/// whisper's stdout, so these reach the log. They are raw, unfiltered output, and
/// the app shows them dimmed so the script's own lines stand out.
pub fn is_whisper_segment_line(line: &str) -> bool {
    // `[hh:mm:ss.mmm --> hh:mm:ss.mmm]`: 31 bytes of fixed shape.
    const SHAPE: &[u8; 31] = b"[dd:dd:dd.ddd --> dd:dd:dd.ddd]";
    let b = line.trim_start().as_bytes();
    b.len() >= SHAPE.len()
        && b.iter().zip(SHAPE).all(|(got, want)| match want {
            b'd' => got.is_ascii_digit(),
            other => got == other,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_line_the_script_prints() {
        assert_eq!(
            parse_progress_line("PROGRESS:start:5"),
            Some(ProgressEvent::Start { total: 5 })
        );
        assert_eq!(
            parse_progress_line("PROGRESS:file:2/5:2-riverone.flac"),
            Some(ProgressEvent::FileStart {
                index: 2,
                total: 5,
                name: "2-riverone.flac".into()
            })
        );
        assert_eq!(
            parse_progress_line("PROGRESS:done:5/5:5-x.flac"),
            Some(ProgressEvent::FileDone {
                index: 5,
                total: 5,
                name: "5-x.flac".into()
            })
        );
        assert_eq!(
            parse_progress_line("PROGRESS:complete"),
            Some(ProgressEvent::TranscribeComplete)
        );
        assert_eq!(parse_progress_line("PROGRESS:postdone"), Some(ProgressEvent::PostDone));
    }

    #[test]
    fn file_names_may_contain_colons() {
        assert_eq!(
            parse_progress_line("PROGRESS:file:1/2:a:b: c.flac"),
            Some(ProgressEvent::FileStart {
                index: 1,
                total: 2,
                name: "a:b: c.flac".into()
            })
        );
    }

    #[test]
    fn everything_else_is_log_text() {
        for line in [
            "",
            "hello",
            "  PROGRESS:start:5",
            "PROGRESS:",
            "PROGRESS:bogus",
            "PROGRESS:start:x",
            "PROGRESS:file:x/5:a",
            "PROGRESS:file:2:a",
            "progress:start:5",
            "Skipping: PROGRESS:complete",
        ] {
            assert_eq!(parse_progress_line(line), None, "{line:?}");
        }
    }

    #[test]
    fn bar_position_follows_the_gui_mapping() {
        let mut t = Tracker::default();
        assert_eq!((t.phase, t.fraction(), t.label()), (Phase::Idle, 0.0, "Ready"));
        t.moving(1, 5, "a.flac");
        assert_eq!(t.fraction(), 0.04);
        t.apply(&ProgressEvent::Start { total: 4 });
        assert_eq!(t.fraction(), 0.08);
        t.apply(&ProgressEvent::FileStart {
            index: 1,
            total: 4,
            name: "a".into(),
        });
        assert!((t.fraction() - 0.08).abs() < 1e-9);
        t.apply(&ProgressEvent::FileDone {
            index: 1,
            total: 4,
            name: "a".into(),
        });
        assert!((t.fraction() - (0.08 + 0.82 / 4.0)).abs() < 1e-9);
        t.apply(&ProgressEvent::FileStart {
            index: 3,
            total: 4,
            name: "c".into(),
        });
        assert!((t.fraction() - (0.08 + 0.82 * 2.0 / 4.0)).abs() < 1e-9);
        assert_eq!(t.label(), "File 3 / 4: c");
        t.apply(&ProgressEvent::FileDone {
            index: 4,
            total: 4,
            name: "d".into(),
        });
        assert!((t.fraction() - 0.90).abs() < 1e-9);
        t.apply(&ProgressEvent::TranscribeComplete);
        assert_eq!((t.phase, t.fraction()), (Phase::PostProcessing, 0.92));
        t.apply(&ProgressEvent::PostDone);
        assert_eq!((t.phase, t.fraction()), (Phase::Finished, 1.0));
    }

    #[test]
    fn zero_total_does_not_divide_by_zero() {
        let mut t = Tracker::default();
        t.apply(&ProgressEvent::FileStart {
            index: 0,
            total: 0,
            name: "x".into(),
        });
        assert!(t.fraction().is_finite());
    }

    #[test]
    fn recognises_whisper_segment_lines_and_nothing_else() {
        for yes in [
            "[00:00:00.000 --> 00:00:30.000]   Thank you.",
            "  [01:02:03.450 --> 01:02:09.999]  The lantern is lit.",
            "[00:00:30.000 --> 00:01:00.000]",
        ] {
            assert!(is_whisper_segment_line(yes), "{yes}");
        }
        for no in [
            "",
            "Session folder: /x",
            "PROGRESS:done:1/5:a.flac",
            "[00:00:00.000 -> 00:00:30.000] one dash short",
            "[0:00:00.000 --> 00:00:30.000] one digit short",
            "[00:00:00,000 --> 00:00:30,000] comma, not a dot",
            "see [00:00:00.000 --> 00:00:30.000] mid-line",
            "[ok] [00:00:00.000 --> 00:00:30.000]",
        ] {
            assert!(!is_whisper_segment_line(no), "{no}");
        }
    }
}
