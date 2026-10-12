//! Running a child process in its own process group, streaming what it prints, and
//! stopping the whole group on cancel. Shared by the transcription job and the
//! index job.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Seconds between asking a process to stop and forcing it.
pub const GRACE: Duration = Duration::from_secs(3);

/// How a child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// `None` when it was killed by a signal or never reported.
    pub exit_code: Option<i32>,
    /// Cancel was requested while it ran.
    pub cancelled: bool,
}

fn spawn_reader<R: Read + Send + 'static>(stream: R, lines: Sender<String>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
                    if lines.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

/// Signal a process group (the program and everything it started).
pub fn signal_group(pid: u32, signal: &str) {
    let _ = Command::new("bash")
        .args([
            "-c",
            "kill -s \"$1\" -- \"-$2\" 2>/dev/null",
            "kill",
            signal,
            &pid.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Run `program args` with exactly `env`, calling `on_line` for every line of stdout
/// and stderr as it arrives. When `cancel` becomes true the group is asked to stop
/// (`on_cancel` is called once), then killed after [`GRACE`]. Err is only "could not
/// start it".
pub fn run_streaming(
    program: &str,
    args: &[String],
    env: &HashMap<String, String>,
    cancel: &AtomicBool,
    mut on_line: impl FnMut(String),
    mut on_cancel: impl FnMut(),
) -> Result<Outcome, std::io::Error> {
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let pid = child.id();
    let (line_tx, line_rx) = mpsc::channel::<String>();
    let readers = [
        child.stdout.take().map(|s| spawn_reader(s, line_tx.clone())),
        child.stderr.take().map(|s| spawn_reader(s, line_tx.clone())),
    ];
    drop(line_tx);

    let mut cancelled = false;
    let mut stop_requested: Option<Instant> = None;
    let mut forced = false;
    loop {
        match line_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => on_line(line),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if cancel.load(Ordering::Relaxed) {
            match stop_requested {
                None => {
                    cancelled = true;
                    on_cancel();
                    signal_group(pid, "TERM");
                    stop_requested = Some(Instant::now());
                }
                Some(at) if !forced && at.elapsed() > GRACE => {
                    signal_group(pid, "KILL");
                    forced = true;
                }
                Some(at) if at.elapsed() > GRACE * 3 => break,
                Some(_) => {}
            }
        }
    }
    let status = child.wait();
    for r in readers.into_iter().flatten() {
        let _ = r.join();
    }
    Ok(Outcome {
        exit_code: status.ok().and_then(|s| s.code()),
        cancelled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn env() -> HashMap<String, String> {
        HashMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())])
    }

    #[test]
    fn lines_from_both_streams_arrive_and_the_exit_code_is_kept() {
        let mut got = Vec::new();
        let out = run_streaming(
            "bash",
            &["-c".into(), "echo one; echo two >&2; exit 3".into()],
            &env(),
            &AtomicBool::new(false),
            |l| got.push(l),
            || {},
        )
        .unwrap();
        got.sort();
        assert_eq!(got, ["one", "two"]);
        assert_eq!(
            out,
            Outcome {
                exit_code: Some(3),
                cancelled: false
            }
        );
    }

    #[test]
    fn a_program_that_cannot_start_is_an_error() {
        let r = run_streaming(
            "/nonexistent/program",
            &[],
            &env(),
            &AtomicBool::new(false),
            |_| {},
            || {},
        );
        assert!(r.is_err());
    }

    #[test]
    fn cancel_stops_the_whole_group_including_grandchildren() {
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = Arc::clone(&cancel);
        let mut cancelled_calls = 0;
        let start = Instant::now();
        let out = run_streaming(
            "bash",
            &["-c".into(), "sleep 30 & echo started; wait".into()],
            &env(),
            &cancel,
            |l| {
                if l == "started" {
                    c2.store(true, Ordering::Relaxed);
                }
            },
            || cancelled_calls += 1,
        )
        .unwrap();
        assert!(out.cancelled && cancelled_calls == 1);
        assert!(start.elapsed() < Duration::from_secs(10), "did not wait for the sleep");
    }
}
