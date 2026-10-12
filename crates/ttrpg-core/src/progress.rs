//! Progress of a transcription run as typed events (PORTING.md section 4.9; the original
//! printed them as `PROGRESS:` lines), and the progress-bar arithmetic the GUI used.

/// One step of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEvent {
    /// N audio files found.
    Start { total: usize },
    /// Starting file i.
    FileStart { index: usize, total: usize, name: String },
    /// File i finished. Also sent for skipped files.
    FileDone { index: usize, total: usize, name: String },
    /// The transcription loop finished.
    TranscribeComplete,
    /// Merge, consolidate and markdown finished.
    PostDone,
    /// How far through the current file whisper has got, from its own segment lines.
    /// The original could not tell.
    InFile { index: usize, total: usize, permille: u16 },
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
            ProgressEvent::InFile { index, total, permille } => {
                self.phase = Phase::Transcribing;
                let f = f64::from(*permille).clamp(0.0, 1000.0) / 1000.0;
                self.fraction = 0.08 + 0.82 * ((*index as f64 - 1.0) + f) / (*total).max(1) as f64;
                self.label = format!("File {index} / {total}: {}  {:.0}%", self.name, f * 100.0);
            }
        }
        self.fraction = self.fraction.clamp(0.0, 1.0);
    }
}

/// The end time, in milliseconds, of a segment line (see [`is_whisper_segment_line`]).
pub fn whisper_segment_end_ms(line: &str) -> Option<u64> {
    if !is_whisper_segment_line(line) {
        return None;
    }
    let b = line.trim_start().as_bytes();
    // `[hh:mm:ss.mmm --> hh:mm:ss.mmm]`: the end time is bytes 18..30.
    let n = |r: std::ops::Range<usize>| -> Option<u64> { std::str::from_utf8(&b[r]).ok()?.parse().ok() };
    Some(n(18..20)? * 3_600_000 + n(21..23)? * 60_000 + n(24..26)? * 1000 + n(27..30)?)
}

/// True for a segment line whisper-cli prints to stdout while it works:
/// `[00:00:30.000 --> 00:01:00.000]   Thank you.` They reach the log as they come. They
/// are raw, unfiltered output, and the app shows them dimmed so the run's own lines
/// stand out.
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
    fn the_end_time_of_a_segment_line_is_read_in_milliseconds() {
        assert_eq!(
            whisper_segment_end_ms("[00:00:30.000 --> 00:01:41.250]   x"),
            Some(101_250)
        );
        assert_eq!(
            whisper_segment_end_ms("  [01:02:03.450 --> 01:02:09.999]"),
            Some(3_729_999)
        );
        assert_eq!(whisper_segment_end_ms("Session folder"), None);
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
            "Done: 1-gmperson.flac in 3s",
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
