//! The whole app, driven the way a person drives it: keys and clicks in, screen state
//! out. The job is the real `legacy/transcribe.sh` with real `ffmpeg` and `jq`, and a
//! stub `whisper-cli`. Tracks are one-second tones with invented names. Needs bash,
//! python3, ffmpeg and jq; without them each test prints SKIPPED and passes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use tempfile::TempDir;
use ttrpg_core::Settings;
use ttrpg_tui::app::{Action, App, Modal, StatusKind, Tab};
use ttrpg_tui::ui;

const STUB: &str = r#"#!/usr/bin/env bash
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
cat > "${of}.json" <<'J'
{"transcription":[
 {"offsets":{"from":1000,"to":2000},"text":" The lantern is lit."},
 {"offsets":{"from":4000,"to":5000},"text":" It’s the Cinder Road."}
]}
J
"#;

macro_rules! require_tools {
    () => {
        let present = ["bash", "python3", "ffmpeg", "jq"].iter().all(|t| {
            Command::new("sh")
                .args(["-c", &format!("command -v {t}")])
                .output()
                .is_ok_and(|o| o.status.success())
        });
        if !present {
            eprintln!("SKIPPED: bash, python3, ffmpeg and jq are needed");
            return;
        }
    };
}

struct Rig {
    tmp: TempDir,
    env: HashMap<String, String>,
    app: App,
    term: Terminal<TestBackend>,
    opened: Rc<RefCell<Vec<String>>>,
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

impl Rig {
    fn new(extra_env: &[(&str, &str)]) -> Rig {
        let tmp = TempDir::new().unwrap();
        let p = |rel: &str| tmp.path().join(rel).to_string_lossy().into_owned();
        let camp = tmp.path().join("vault/0.2_campaigns/demo");
        fs::create_dir_all(&camp).unwrap();
        fs::write(
            camp.join("canon_aliases.json"),
            r#"{"campaign": "Demo", "system": "daggerheart",
                "discord_names": {
                    "gmperson": {"player": "Sky", "character": null, "role": "GM"},
                    "riverone": {"player": "Pat", "character": "Mara_Quill", "role": "Player"},
                    "rivertwo": {"player": "Lee", "character": "Tomas_Ink", "role": "Player"}}}"#,
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("bin")).unwrap();
        let stub = tmp.path().join("bin/stub-whisper");
        fs::write(&stub, STUB).unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(tmp.path().join("model.bin"), "x").unwrap();
        fs::create_dir_all(tmp.path().join("home/Downloads")).unwrap();

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
        let opened = Rc::new(RefCell::new(Vec::new()));
        let app = Self::open(&env, &opened);
        Rig {
            tmp,
            env,
            app,
            term: Terminal::new(TestBackend::new(140, 40)).unwrap(),
            opened,
        }
    }

    fn open(env: &HashMap<String, String>, opened: &Rc<RefCell<Vec<String>>>) -> App {
        let mut app = App::new(Settings::from_env_map(env.clone(), "ttrpg-tui"));
        let sink = Rc::clone(opened);
        app.set_opener(Box::new(move |path| {
            sink.borrow_mut().push(path.to_string());
            Ok(())
        }));
        app
    }

    /// Quit and reopen the program on the same files.
    fn restart(&mut self) {
        self.app = Self::open(&self.env, &self.opened);
    }

    fn downloads(&self) -> PathBuf {
        self.tmp.path().join("home/Downloads")
    }

    fn tone(&self, name: &str) -> PathBuf {
        let path = self.downloads().join(name);
        let ok = Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-ar",
                "16000",
                "-ac",
                "1",
            ])
            .arg(&path)
            .status()
            .unwrap()
            .success();
        assert!(ok);
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

    fn press(&mut self, c: char) {
        self.app.on_key(key(KeyCode::Char(c)));
    }

    fn draw(&mut self) -> String {
        self.term.draw(|f| ui::draw(f, &mut self.app)).unwrap();
        let buf = self.term.backend().buffer();
        let w = usize::from(buf.area.width);
        buf.content
            .chunks(w)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn click(&mut self, x: u16, y: u16) {
        self.app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    }

    /// Poll the job like the event loop does until `done` holds.
    fn pump_until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            self.app.poll_job();
            if done(&self.app) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// f, a (all), c (use them): the picker, as typed.
    fn choose_all_in_downloads(&mut self) {
        self.press('f');
        assert!(matches!(self.app.modal, Some(Modal::Picker(_))), "f opens the picker");
        self.press('a');
        self.press('c');
        assert!(self.app.modal.is_none());
    }

    fn start_and_confirm(&mut self) {
        self.press('s');
        assert!(matches!(self.app.modal, Some(Modal::Confirm(_))), "s asks first");
        self.app.on_key(key(KeyCode::Enter));
        assert!(self.app.running());
    }
}

#[test]
fn from_picking_files_to_reading_the_transcript() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    let files = rig.five_tracks();

    // The picker starts in ~/Downloads, lists the tracks, and `a` then `c` chooses them all.
    rig.choose_all_in_downloads();
    assert_eq!(rig.app.chosen.len(), 5);
    assert_eq!(
        rig.app.unknown_tracks(),
        2,
        "plain_0 and newperson are not in discord_names"
    );
    assert!(rig.draw().contains("⚠ 2 unknown speaker(s)"));

    rig.start_and_confirm();
    assert_eq!(rig.app.tab, Tab::Log, "the log opens to show the script");
    rig.pump_until("the job to finish", |a| !a.running());

    let report = rig.app.report.clone().unwrap();
    assert!(report.ok(), "{report:?}");
    assert_eq!(report.headline(), "5 of 5 transcribed");
    for f in &files {
        assert!(!f.exists(), "{} was moved out of Downloads", f.display());
    }
    assert!(rig.app.chosen.is_empty(), "nothing left to move");
    assert_eq!(
        rig.app.tracks.len(),
        5,
        "the five tracks are now the folder's, so a retry stays possible"
    );
    assert!(rig.app.tracks.iter().all(|t| t.in_folder && !t.incoming));

    // The transcript opens by itself; the bar is full; the status says what happened.
    assert_eq!(rig.app.tab, Tab::Transcript);
    let screen = rig.draw();
    assert!(
        screen.contains("[00:00:01] GM") && screen.contains("The lantern is lit."),
        "{screen}"
    );
    assert!(screen.contains("Complete"), "{screen}");
    assert!(
        screen.contains("5 of 5 transcribed. transcript.md is in the Transcript tab."),
        "{screen}"
    );
    assert_eq!(rig.app.status.kind, StatusKind::Ok);
    assert!(rig.draw().contains("Result     transcript.md exists ✓"));

    // The log keeps the script's words and ends with the verdict.
    rig.app.set_tab(Tab::Log);
    let log = rig.app.log.iter().cloned().collect::<Vec<_>>().join("\n");
    assert!(
        log.contains("Found 5 audio file(s)") && log.contains("== Result ==") && log.contains("5 of 5 transcribed"),
        "{log}"
    );
}

#[test]
fn a_failing_track_is_shown_as_a_failure_and_can_be_retried() {
    require_tools!();
    let mut rig = Rig::new(&[("STUB_FAIL_MATCH", "rivertwo")]);
    rig.five_tracks();
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    rig.pump_until("the first run", |a| !a.running());
    let report = rig.app.report.clone().unwrap();
    assert!(!report.ok());
    assert_eq!(report.headline(), "4 of 5 transcribed, 1 failed");
    assert_eq!(rig.app.status.kind, StatusKind::Error);
    assert_eq!(rig.app.tab, Tab::Log, "problems keep you on the log");
    let screen = rig.draw();
    assert!(
        screen.contains("failed: 3-rivertwo.flac: whisper-cli exited non-zero"),
        "{screen}"
    );
    assert!(screen.contains("Finished with problems"), "{screen}");

    // Retry with no new files: the folder's tracks are the job, the finished ones are skipped.
    // (Same environment, so the stub still fails rivertwo; the point is the skip.)
    rig.press('s');
    let Some(Modal::Confirm(plan)) = &rig.app.modal else {
        panic!("expected the confirm dialog")
    };
    assert_eq!((plan.moves.len(), plan.existing.len()), (0, 5));
    rig.app.on_key(key(KeyCode::Enter));
    rig.pump_until("the retry", |a| !a.running());
    let retry = rig.app.report.clone().unwrap();
    assert_eq!(retry.skipped.len(), 4, "{retry:?}");
    assert_eq!(retry.failed.len(), 1);
}

#[test]
fn x_cancels_a_running_job_and_the_app_says_so() {
    require_tools!();
    let mut rig = Rig::new(&[("STUB_SLEEP", "29.3")]);
    rig.five_tracks();
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    rig.pump_until("the first track to start", |a| {
        a.tracker.label().starts_with("File 1 / 5")
    });
    assert!(rig.app.enabled(Action::Cancel) && !rig.app.enabled(Action::Start));

    rig.press('x');
    assert!(rig.app.status.text.starts_with("Cancelling"));
    let started = Instant::now();
    rig.pump_until("the job to stop", |a| !a.running());
    assert!(started.elapsed() < Duration::from_secs(20));
    let report = rig.app.report.clone().unwrap();
    assert!(report.cancelled && !report.ok());
    assert_eq!(rig.app.status.kind, StatusKind::Warn);
    let screen = rig.draw();
    assert!(screen.contains("Cancelled"), "{screen}");
    assert!(!screen.contains("Complete"));

    let running = Command::new("pgrep")
        .args(["-f", "sleep 29.3"])
        .output()
        .unwrap()
        .status
        .success();
    assert!(!running, "no child process was left running");
}

#[test]
fn q_twice_in_a_row_cancels_a_running_job_then_quits() {
    require_tools!();
    let mut rig = Rig::new(&[("STUB_SLEEP", "27.9")]);
    rig.five_tracks();
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    rig.pump_until("the first track", |a| a.tracker.label().starts_with("File 1 / 5"));

    rig.press('q');
    assert!(!rig.app.should_quit, "one q only warns");
    assert!(rig.app.status.text.contains("Press q again"));

    rig.press('j'); // any other key disarms it
    rig.press('q');
    assert!(
        !rig.app.should_quit && rig.app.running(),
        "the warning starts over after another key"
    );
    assert!(rig.app.status.text.contains("Press q again"));

    rig.press('q'); // second q in a row
    assert!(!rig.app.should_quit, "it waits for the job to stop first");
    rig.pump_until("quit", |a| a.should_quit);
    assert!(!rig.app.running());
    assert!(rig.app.report.as_ref().unwrap().cancelled);
}

#[test]
fn ctrl_c_stops_the_job_first_and_quits_only_when_idle() {
    require_tools!();
    let mut rig = Rig::new(&[("STUB_SLEEP", "26.1")]);
    rig.five_tracks();
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    rig.pump_until("the first track", |a| a.tracker.label().starts_with("File 1 / 5"));
    let ctrl_c = KeyEvent {
        modifiers: KeyModifiers::CONTROL,
        ..key(KeyCode::Char('c'))
    };
    rig.app.on_key(ctrl_c);
    rig.pump_until("cancel", |a| !a.running());
    assert!(!rig.app.should_quit);
    rig.app.on_key(ctrl_c);
    assert!(rig.app.should_quit);
}

#[test]
fn the_session_number_is_offered_edited_and_stepped() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    // Session 1 has a transcript, so 2 is offered.
    let s1 = rig.tmp.path().join("vault/0.2_campaigns/demo/08_transcripts/S01");
    fs::create_dir_all(&s1).unwrap();
    fs::write(s1.join("transcript.md"), "x").unwrap();
    assert_eq!(
        rig.app.session,
        Some(1),
        "the number chosen when the program opened stands"
    );
    rig.press('r');
    assert_eq!(rig.app.session, Some(1), "a rescan does not overwrite the number");
    rig.restart();
    assert_eq!(
        rig.app.session,
        Some(2),
        "reopening offers the session after the last finished one"
    );
    rig.press('+');
    assert_eq!(rig.app.session, Some(3));
    rig.press('-');
    rig.press('-');
    rig.press('-');
    assert_eq!(rig.app.session, Some(1), "never below 1");

    rig.press('n');
    for c in ['1', '4', 'x', '2'] {
        rig.press(c); // x is ignored
    }
    rig.app.on_key(key(KeyCode::Enter));
    assert_eq!(rig.app.session, Some(142));
    rig.press('n');
    rig.press('9');
    rig.app.on_key(key(KeyCode::Esc));
    assert_eq!(rig.app.session, Some(142), "Esc abandons the edit");
    rig.press('n');
    rig.press('0');
    rig.app.on_key(key(KeyCode::Enter));
    assert_eq!(rig.app.session, Some(142), "0 is refused");
    assert!(rig.app.status.text.contains("1 or higher"));
}

#[test]
fn start_says_why_it_cannot_run() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    rig.press('s');
    assert!(rig.app.status.text.contains("No audio yet"), "{}", rig.app.status.text);
    assert!(rig.app.modal.is_none());
    rig.press('x');
    assert_eq!(rig.app.status.text, "Nothing is running.");
    rig.press('o');
    assert_eq!(rig.app.status.text, "The session folder does not exist yet.");
    rig.press('c');
    assert_eq!(rig.app.status.text, "No files are chosen.");
}

#[test]
fn missing_tools_open_the_install_dialog_instead_of_a_confirm() {
    require_tools!();
    // A PATH with nothing on it: every required tool is missing.
    let mut rig = Rig::new(&[("PATH", "/nonexistent")]);
    rig.five_tracks();
    rig.choose_all_in_downloads();
    rig.press('s');
    assert!(matches!(rig.app.modal, Some(Modal::Setup)));
    assert_eq!(rig.app.status.kind, StatusKind::Warn);
    assert!(rig.draw().contains("[MISSING]"));
    assert_eq!(rig.app.chosen.len(), 5, "and nothing was moved");
    assert!(rig.downloads().join("1-gmperson.flac").exists());
}

#[test]
fn escape_at_the_confirm_step_moves_nothing() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    rig.five_tracks();
    rig.choose_all_in_downloads();
    rig.press('s');
    rig.app.on_key(key(KeyCode::Esc));
    assert!(rig.app.modal.is_none() && !rig.app.running());
    assert!(rig.downloads().join("1-gmperson.flac").exists());
    assert!(
        !rig.tmp.path().join("audio").exists(),
        "not even the folders were created"
    );
}

#[test]
fn the_buttons_and_panes_answer_the_mouse() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    rig.five_tracks();
    rig.draw();

    // [f] Files opens the picker.
    let files = rig
        .app
        .hit
        .buttons
        .iter()
        .find(|(_, a)| *a == Action::ChooseFiles)
        .unwrap()
        .0;
    rig.click(files.x + 2, files.y);
    assert!(matches!(rig.app.modal, Some(Modal::Picker(_))));
    rig.draw();
    // Click the first audio file in the picker's list (row 0 is "..", rows 1.. the files), then Use.
    let Some(Modal::Picker(p)) = &rig.app.modal else {
        unreachable!()
    };
    let (list, done) = (p.list_inner, p.done_button);
    rig.click(list.x + 6, list.y + 1);
    rig.click(done.x + 1, done.y);
    assert!(rig.app.modal.is_none());
    assert_eq!(rig.app.chosen.len(), 1);

    // Clicking the session number starts editing it; a click elsewhere abandons the edit.
    rig.draw();
    let field = rig.app.hit.session_field;
    rig.click(field.x + 1, field.y);
    assert!(rig.app.editing.is_some());
    rig.click(rig.app.hit.campaigns.x + 1, rig.app.hit.campaigns.y);
    assert!(rig.app.editing.is_none());

    // The Start button works like s, and the confirm dialog's own buttons are live.
    rig.draw();
    let start = rig.app.hit.buttons.iter().find(|(_, a)| *a == Action::Start).unwrap().0;
    rig.click(start.x + 1, start.y);
    assert!(matches!(rig.app.modal, Some(Modal::Confirm(_))));
    rig.draw();
    let cancel = rig.app.hit.confirm_cancel;
    rig.click(cancel.x + 1, cancel.y);
    assert!(rig.app.modal.is_none() && !rig.app.running());

    // A disabled button explains itself.
    let cancel_btn = rig
        .app
        .hit
        .buttons
        .iter()
        .find(|(_, a)| *a == Action::Cancel)
        .unwrap()
        .0;
    rig.click(cancel_btn.x + 1, cancel_btn.y);
    assert_eq!(rig.app.status.text, "Nothing is running.");
}

#[test]
fn campaigns_cannot_be_switched_mid_run() {
    require_tools!();
    let mut rig = Rig::new(&[("STUB_SLEEP", "25.7")]);
    // a second campaign
    let other = rig.tmp.path().join("vault/0.2_campaigns/other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("canon_aliases.json"), "{}").unwrap();
    rig.press('r');
    assert_eq!(rig.app.campaigns.len(), 2);
    rig.five_tracks();
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    rig.pump_until("the first track", |a| a.tracker.label().starts_with("File 1 / 5"));
    let before = rig.app.selected().unwrap().0.clone();
    rig.press('h'); // focus the campaign list
    rig.press('j');
    assert_eq!(rig.app.selected().unwrap().0, before);
    assert!(rig.app.status.text.contains("A job is running"));
    rig.press('x');
    rig.pump_until("cancel", |a| !a.running());
}

#[test]
fn open_folder_opens_the_session_folder_once_it_exists() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    rig.five_tracks();
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    rig.pump_until("done", |a| !a.running());
    rig.press('o');
    let opened = rig.opened.borrow();
    assert_eq!(opened.len(), 1);
    assert!(opened[0].ends_with("08_transcripts/S01"), "{opened:?}");
}
