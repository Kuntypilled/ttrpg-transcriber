//! The whole app, driven the way a person drives it: keys and clicks in, screen state
//! out. The job is the real Rust runner with real `ffmpeg` and a stub `whisper-cli`; Index runs
//! the real Chroma helper against a stand-in for `chromadb`. Tracks are one-second tones with invented
//! names. Needs bash, ffmpeg and python3; without them each test prints SKIPPED and passes.

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
echo "[00:00:00.000 --> 00:00:00.900]   you"
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
 {"offsets":{"from":1000,"to":2000},"text":" The lantern is lit."},
 {"offsets":{"from":4000,"to":5000},"text":" It’s the Cinder Road."}
]}
J
"#;

/// Index runs the real Chroma helper under `python3`, against a stand-in for the `chromadb`
/// package, so the whole path from key press to stored chunk is the program's own.
const FAKE_CHROMA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ttrpg-core/tests/fixtures/fake_chroma");
const NO_CHROMA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../ttrpg-core/tests/fixtures/no_chroma");

macro_rules! require_tools {
    () => {
        let present = ["bash", "ffmpeg", "python3"].iter().all(|t| {
            Command::new("sh")
                .args(["-c", &format!("command -v {t}")])
                .output()
                .is_ok_and(|o| o.status.success())
        });
        if !present {
            eprintln!("SKIPPED: bash, ffmpeg and python3 are needed");
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
            ("GM_PYTHON".to_string(), "python3".to_string()),
            ("PYTHONPATH".to_string(), FAKE_CHROMA.to_string()),
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
    rig.press('R');
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
    rig.press('R');
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

#[test]
fn the_confirm_step_shows_the_invented_lines_rule_and_can_flip_it_for_one_run() {
    require_tools!();
    let mut rig = Rig::new(&[("STUB_THANKS", "1")]);
    rig.tone("1-gmperson.flac");
    rig.choose_all_in_downloads();
    rig.press('s');
    assert!(matches!(rig.app.modal, Some(Modal::Confirm(_))));
    let on = rig.draw();
    for want in [
        "Invented lines",
        "\"thank you\" and \"we'll be right back\"",
        "20 s or more",
        "[t] Keep invented lines",
    ] {
        assert!(on.contains(want), "missing {want:?}:\n{on}");
    }

    // t flips it off for this run, and says so.
    rig.press('t');
    let off = rig.draw();
    assert!(
        off.contains("Off: lines like \"Thank you.\"") && off.contains("[t] Drop invented lines"),
        "{off}"
    );
    assert!(off.contains("Turned off for this run."), "{off}");
    // Clicking the button flips it back on, and a second click off again.
    let btn = rig.app.hit.confirm_stock;
    assert!(btn.width > 0);
    rig.click(btn.x + 1, btn.y);
    assert!(rig.draw().contains("[t] Keep invented lines"));
    let btn = rig.app.hit.confirm_stock;
    rig.click(btn.x + 1, btn.y);
    assert!(rig.draw().contains("[t] Drop invented lines"));

    // Run it off: both "Thank you." lines stay in the transcript.
    rig.app.on_key(key(KeyCode::Enter));
    assert!(rig.app.running());
    rig.pump_until("the job to finish", |a| !a.running());
    let report = rig.app.report.clone().unwrap();
    assert!(report.ok(), "{report:?}");
    let md = fs::read_to_string(report.transcript_md.as_deref().unwrap()).unwrap();
    assert_eq!(md.matches("Thank you.").count(), 2, "{md}");

    // The flip lasted one run: the next confirm step is back to the setting (on).
    rig.press('s');
    assert!(rig.draw().contains("[t] Keep invented lines"));
    rig.app.on_key(key(KeyCode::Esc));
}

#[test]
fn by_default_a_run_drops_the_invented_line_and_keeps_the_real_one() {
    require_tools!();
    let mut rig = Rig::new(&[("STUB_THANKS", "1")]);
    rig.tone("1-gmperson.flac");
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    rig.pump_until("the job to finish", |a| !a.running());
    let report = rig.app.report.clone().unwrap();
    let md = fs::read_to_string(report.transcript_md.as_deref().unwrap()).unwrap();
    assert_eq!(
        md.matches("Thank you.").count(),
        1,
        "the 30 s one is gone, the 1.5 s one stays:\n{md}"
    );
    let log = rig.app.log.iter().cloned().collect::<Vec<_>>().join("\n");
    assert!(log.contains("stock phrases dropped: 1"), "{log}");
}

#[test]
fn a_setting_that_turns_the_rule_off_is_shown_and_t_can_turn_it_back_on() {
    require_tools!();
    let mut rig = Rig::new(&[("STUB_THANKS", "1"), ("GM_STOCK_PHRASES", "off")]);
    rig.tone("1-gmperson.flac");
    rig.choose_all_in_downloads();
    rig.press('s');
    let screen = rig.draw();
    assert!(
        screen.contains("Off: lines like") && screen.contains("Set by $GM_STOCK_PHRASES"),
        "{screen}"
    );
    rig.press('t');
    let screen = rig.draw();
    assert!(
        screen.contains("[t] Keep invented lines") && screen.contains("\"thank you\" and \"we'll be right back\""),
        "t falls back to the script's default when the setting is off:\n{screen}"
    );
}

const EXPANDED_ALIASES: &str = r#"{
  "_comment": "Hand-kept notes. The layout below is on purpose.",
  "campaign": "Demo",
  "system": "daggerheart",
  "discord_names": {
    "gmperson": {
      "player": "Sky",
      "character": null,
      "role": "GM"
    },
    "riverone": {
      "player": "Pat",
      "character": "Mara_Quill",
      "role": "Player"
    },
    "rivertwo": {
      "player": "Lee",
      "character": "Tomas_Ink",
      "role": "Player"
    }
  },
  "players": {"Mara Quill": "Pat", "Tomas Ink": "Lee"}
}
"#;

fn ctrl(c: char) -> KeyEvent {
    KeyEvent {
        modifiers: KeyModifiers::CONTROL,
        ..key(KeyCode::Char(c))
    }
}

impl Rig {
    fn aliases_path(&self) -> PathBuf {
        self.tmp.path().join("vault/0.2_campaigns/demo/canon_aliases.json")
    }

    /// Use a hand-formatted alias file and reopen the app on it.
    fn with_aliases(&mut self, body: &str) {
        fs::write(self.aliases_path(), body).unwrap();
        self.restart();
    }

    fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            self.press(c);
        }
    }
}

fn labels_in(path: &str) -> Vec<String> {
    let mut out: Vec<String> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|l| l.strip_prefix("**["))
        .filter_map(|l| l.split_once("] "))
        .filter_map(|(_, rest)| rest.split_once(":**").map(|(label, _)| label.to_string()))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn resolving_unknown_speakers_changes_only_the_appended_lines_and_the_next_run_uses_the_new_names() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    rig.with_aliases(EXPANDED_ALIASES);
    rig.five_tracks();
    rig.choose_all_in_downloads();
    assert_eq!(rig.app.unknown_tracks(), 2);
    assert!(rig.draw().contains("[r] Resolve"));

    rig.press('r');
    assert!(matches!(rig.app.modal, Some(Modal::Resolve(_))));
    let form = rig.draw();
    assert!(form.contains("> plain_0") && form.contains("newperson"), "{form}");
    assert!(form.contains("Characters: Mara_Quill, Tomas_Ink"), "{form}");

    // plain_0 is Kay, playing Wern Reed. newperson is Jo, with no character: the GM.
    rig.type_text("Kay");
    rig.app.on_key(key(KeyCode::Tab));
    rig.type_text("Wern Reed");
    rig.app.on_key(key(KeyCode::Tab));
    rig.type_text("Jo");
    rig.app.on_key(ctrl('s'));
    assert!(rig.app.modal.is_none(), "saved and closed");

    assert_eq!(rig.app.unknown_tracks(), 0);
    assert!(
        rig.app
            .status
            .text
            .starts_with("Added 2 speaker(s) to demo's discord_names: plain_0, newperson."),
        "{}",
        rig.app.status.text
    );
    assert_eq!(rig.app.status.kind, StatusKind::Ok);
    let screen = rig.draw();
    assert!(
        screen.contains("Wern_Reed") && screen.contains("GM (Jo)") && !screen.contains("⚠ unknown"),
        "{screen}"
    );
    assert!(
        screen.contains("all speakers resolved") || !screen.contains("unknown speaker"),
        "{screen}"
    );

    // The file: cut the new lines out and take back the comma the last old entry gained,
    // and the original is what is left, byte for byte.
    let after = fs::read_to_string(rig.aliases_path()).unwrap();
    let new_part = "    \"plain_0\": {\n      \"player\": \"Kay\",\n      \"character\": \"Wern_Reed\",\n      \"role\": \"Player\"\n    },\n    \"newperson\": {\n      \"player\": \"Jo\",\n      \"character\": null,\n      \"role\": \"GM\"\n    }\n";
    assert!(after.contains(new_part), "{after}");
    let restored = after
        .replace(new_part, "")
        .replacen("    },\n  },\n", "    }\n  },\n", 1);
    assert_eq!(
        restored, EXPANDED_ALIASES,
        "only the appended lines (and one comma) changed"
    );
    let value: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert_eq!(value["discord_names"]["plain_0"]["character"], "Wern_Reed");
    assert_eq!(value["discord_names"]["newperson"]["role"], "GM");
    assert_eq!(value["players"]["Tomas Ink"], "Lee");

    // The next run labels the tracks with the new names.
    rig.start_and_confirm();
    rig.pump_until("the job to finish", |a| !a.running());
    let report = rig.app.report.clone().unwrap();
    assert!(report.ok(), "{report:?}");
    let labels = labels_in(report.transcript_md.as_deref().unwrap());
    assert_eq!(labels, ["GM", "Mara_Quill", "Tomas_Ink", "Wern_Reed"], "{labels:?}");
}

#[test]
fn resolving_says_what_went_wrong_and_never_writes_a_half_file() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    rig.with_aliases(EXPANDED_ALIASES);
    rig.tone("4-plain_0.flac");
    rig.tone("5-newperson.flac");
    rig.choose_all_in_downloads();
    let before = fs::read_to_string(rig.aliases_path()).unwrap();

    // Saving an empty form does nothing but say so.
    rig.press('r');
    rig.app.on_key(ctrl('s'));
    assert!(matches!(rig.app.modal, Some(Modal::Resolve(_))));
    assert!(rig.draw().contains("Nothing is filled in."));
    // Esc with nothing typed closes without a word.
    rig.app.on_key(key(KeyCode::Esc));
    assert!(rig.app.modal.is_none());

    // Typing then Esc asks first; the second Esc discards.
    rig.press('r');
    rig.type_text("Kay");
    rig.app.on_key(key(KeyCode::Esc));
    assert!(matches!(rig.app.modal, Some(Modal::Resolve(_))));
    assert!(rig.draw().contains("Esc again discards"));
    rig.app.on_key(key(KeyCode::Esc));
    assert!(rig.app.modal.is_none());
    assert_eq!(
        fs::read_to_string(rig.aliases_path()).unwrap(),
        before,
        "nothing written so far"
    );

    // The file went bad since the app loaded it: the form stays open with the reason,
    // and the file is left exactly as it is.
    rig.press('r');
    rig.type_text("Kay");
    fs::write(rig.aliases_path(), "{ this is not json").unwrap();
    rig.app.on_key(ctrl('s'));
    assert!(matches!(rig.app.modal, Some(Modal::Resolve(_))), "still open");
    let screen = rig.draw();
    assert!(screen.contains("is not valid JSON"), "{screen}");
    assert_eq!(fs::read_to_string(rig.aliases_path()).unwrap(), "{ this is not json");
    rig.app.on_key(key(KeyCode::Esc));
    rig.app.on_key(key(KeyCode::Esc));
    assert!(rig.app.modal.is_none());
    assert_eq!(rig.app.unknown_tracks(), 2, "nothing changed in the app either");
}

#[test]
fn a_name_already_listed_without_a_character_is_left_alone_and_said_so() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    // newperson is in the file, but with no character and no GM role: the script still
    // calls the track unknown. The append never overwrites, so the person must edit it.
    let body = EXPANDED_ALIASES.replace(
        "  \"discord_names\": {\n",
        "  \"discord_names\": {\n    \"newperson\": {\"player\": \"Jo\"},\n",
    );
    rig.with_aliases(&body);
    rig.tone("5-newperson.flac");
    rig.tone("4-plain_0.flac");
    rig.choose_all_in_downloads();
    assert_eq!(rig.app.unknown_tracks(), 2);
    rig.press('r');
    // Row 0 is plain_0 (leave it), row 1 is newperson.
    rig.app.on_key(key(KeyCode::Down));
    rig.type_text("Someone Else");
    rig.app.on_key(ctrl('s'));
    assert!(rig.app.modal.is_none());
    let status = rig.app.status.text.clone();
    assert!(status.contains("Nothing was added"), "{status}");
    assert!(
        status.contains("Left alone, already listed without a character or GM role: newperson"),
        "{status}"
    );
    assert!(status.contains("2 track(s) still unknown"), "{status}");
    assert_eq!(rig.app.status.kind, StatusKind::Warn);
    assert_eq!(fs::read_to_string(rig.aliases_path()).unwrap(), body, "not touched");
}

#[test]
fn resolve_is_refused_while_a_job_runs() {
    require_tools!();
    let mut rig = Rig::new(&[("STUB_SLEEP", "25.7")]);
    rig.tone("1-gmperson.flac");
    rig.tone("5-newperson.flac");
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    rig.pump_until("the first track", |a| a.tracker.label().starts_with("File 1 / 2"));
    rig.press('r');
    assert!(rig.app.modal.is_none());
    assert_eq!(rig.app.status.text, "A job is running.");
    rig.press('x');
    rig.pump_until("cancel", |a| !a.running());
}

const INDEX_ALIASES: &str = r#"{
  "campaign": "Demo",
  "system": "daggerheart",
  "names": ["Marrow Hall"],
  "aliases": {"Marrow Hall": ["Marro Hall"]},
  "discord_names": {
    "gmperson": {"player": "Sky", "character": null, "role": "GM"},
    "riverone": {"player": "Pat", "character": "Mara_Quill", "role": "Player"},
    "rivertwo": {"player": "Lee", "character": "Tomas_Ink", "role": "Player"}
  },
  "players": {"Mara Quill": "Pat", "Tomas Ink": "Lee"},
  "speaker_map": [
    {"match": "gm", "player": "Sky", "character": null},
    {"match": "mara", "player": "Pat", "character": "Mara_Quill"},
    {"match": "tomas", "player": "Lee", "character": "Tomas_Ink"}
  ]
}
"#;

impl Rig {
    /// Three tracks transcribed, so the session has a transcript.md to index.
    fn transcribed(&mut self, aliases: &str) {
        self.with_aliases(aliases);
        for n in ["1-gmperson.flac", "2-riverone.flac", "3-rivertwo.flac"] {
            self.tone(n);
        }
        self.choose_all_in_downloads();
        self.start_and_confirm();
        self.pump_until("the transcription", |a| !a.running());
        assert!(self.app.report.as_ref().unwrap().ok());
    }

    fn chunks_path(&self) -> PathBuf {
        self.tmp
            .path()
            .join("vault/0.2_campaigns/demo/08_transcripts/chunks/s01_chunks.jsonl")
    }
}

#[test]
fn indexing_is_off_until_there_is_a_transcript_and_says_why() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    rig.with_aliases(INDEX_ALIASES);
    assert!(!rig.app.enabled(Action::Index) && !rig.app.enabled(Action::Report));
    rig.press('i');
    assert_eq!(rig.app.status.text, "There is no transcript.md for this session yet.");
    assert!(rig.app.modal.is_none() && !rig.app.running());
}

#[test]
fn without_a_review_only_list_indexing_asks_and_the_report_comes_first_by_default() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    rig.transcribed(INDEX_ALIASES);
    let banner = rig.draw();
    assert!(banner.contains("no review_only list: a = alias report"), "{banner}");

    rig.press('i');
    assert!(matches!(rig.app.modal, Some(Modal::NoReviewOnly)));
    let ask = rig.draw();
    for want in [
        "No review_only list",
        "This campaign has no review_only list yet.",
        "[Enter] Run the report first",
        "[i] Index anyway",
        "[Esc] Cancel",
    ] {
        assert!(ask.contains(want), "missing {want:?}:\n{ask}");
    }
    // Esc asks nothing further and starts nothing.
    rig.app.on_key(key(KeyCode::Esc));
    assert!(rig.app.modal.is_none() && !rig.app.running());
    assert!(!rig.chunks_path().exists());

    // Enter runs the report, then opens it.
    rig.press('i');
    rig.app.on_key(key(KeyCode::Enter));
    assert!(rig.app.running());
    rig.pump_until("the report", |a| !a.running());
    let r = rig.app.index_report.clone().unwrap();
    assert!(r.ok(), "{}", r.headline());
    let report = rig
        .tmp
        .path()
        .join("vault/0.2_campaigns/demo/08_transcripts/S01/alias_candidates.md");
    assert!(report.is_file());
    assert_eq!(
        rig.opened.borrow().last().map(String::as_str),
        Some(report.to_str().unwrap())
    );
    assert!(
        rig.app
            .status
            .text
            .starts_with("Alias report ready. Nothing was written to canon_aliases.json. Opened"),
        "{}",
        rig.app.status.text
    );
    assert_eq!(rig.app.status.kind, StatusKind::Ok);
    assert!(rig.draw().contains("Report ready"));
    assert!(!rig.chunks_path().exists(), "a report does not index");
}

#[test]
fn index_anyway_chunks_and_stores_and_the_result_is_in_the_status_and_the_log() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    rig.transcribed(INDEX_ALIASES);
    rig.press('i');
    rig.press('i'); // index anyway
    assert!(rig.app.running());
    assert_eq!(rig.app.tab, Tab::Log);
    rig.pump_until("indexing", |a| !a.running());
    let r = rig.app.index_report.clone().unwrap();
    assert!(r.ok(), "{}", r.headline());
    let status = rig.app.status.text.clone();
    assert!(
        status.starts_with("Indexed ") && status.contains(" chunks of session 1; the store now holds "),
        "{status}"
    );
    let stored = fs::read_to_string(rig.tmp.path().join("chroma/demo/store.json")).unwrap();
    assert!(stored.contains("Cinder Road"), "the chunk reached the store: {stored}");
    assert_eq!(rig.app.status.kind, StatusKind::Ok);
    assert!(rig.chunks_path().is_file());
    let log = rig.app.log.iter().cloned().collect::<Vec<_>>().join("\n");
    assert!(log.contains("Upserted ") && log.contains("== Result =="), "{log}");
    let screen = rig.draw();
    assert!(screen.contains("Indexed "), "{screen}");
}

#[test]
fn with_a_review_only_list_indexing_starts_straight_away() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    let with_review = INDEX_ALIASES.replace(
        "\"names\"",
        "\"review_only\": {\"Marrow Hall\": [\"borrow hall\"]},\n  \"names\"",
    );
    rig.transcribed(&with_review);
    assert!(!rig.draw().contains("no review_only list"));
    rig.press('i');
    assert!(rig.app.modal.is_none(), "no question");
    assert!(rig.app.running());
    rig.pump_until("indexing", |a| !a.running());
    assert!(rig.app.index_report.as_ref().unwrap().ok());
}

#[test]
fn a_missing_chromadb_is_explained_in_the_status() {
    require_tools!();
    let mut rig = Rig::new(&[("PYTHONPATH", NO_CHROMA)]);
    rig.transcribed(INDEX_ALIASES);
    rig.press('i');
    rig.press('i');
    rig.pump_until("indexing", |a| !a.running());
    let r = rig.app.index_report.clone().unwrap();
    assert!(!r.ok());
    let status = rig.app.status.text.clone();
    assert!(
        status.contains("Storing in ChromaDB failed") && status.contains("pip install chromadb"),
        "{status}"
    );
    assert_eq!(rig.app.status.kind, StatusKind::Error);
    assert!(rig.draw().contains("Failed"));
    assert!(rig.chunks_path().is_file(), "the chunk file is kept");
}

#[test]
fn x_cancels_indexing_and_the_app_says_nothing_was_added() {
    require_tools!();
    let mut rig = Rig::new(&[("FAKE_CHROMA_SLEEP", "30")]);
    rig.transcribed(INDEX_ALIASES);
    rig.press('i');
    rig.press('i');
    rig.pump_until("the storing step", |a| a.tracker.label().starts_with("Embedding"));
    rig.press('x');
    rig.pump_until("cancel", |a| !a.running());
    let r = rig.app.index_report.clone().unwrap();
    assert!(r.cancelled);
    assert!(rig.app.status.text.contains("cancelled") && rig.app.status.text.contains("Nothing was added"));
    assert_eq!(rig.app.status.kind, StatusKind::Warn);
}

#[test]
fn the_bar_says_how_far_through_the_current_track_whisper_is() {
    require_tools!();
    let mut rig = Rig::new(&[("STUB_SLEEP", "26.1")]);
    rig.tone("1-gmperson.flac");
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    // The stub prints a segment ending at 0.9 s of a 1 s track, then sleeps.
    rig.pump_until("the in-track percentage", |a| a.tracker.label().contains("90%"));
    let screen = rig.draw();
    assert!(screen.contains("File 1 / 1: 1-gmperson.flac  90%"), "{screen}");
    rig.press('x');
    rig.pump_until("the job to stop", |a| !a.running());
}

#[test]
fn transcribe_all_again_is_offered_at_the_confirm_step_and_lasts_one_run() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    rig.five_tracks();
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    rig.pump_until("the first run", |a| !a.running());
    assert!(rig.app.report.as_ref().unwrap().ok());

    rig.press('s');
    let screen = rig.draw();
    for want in [
        "Nothing to move. 5 file(s) are already in:",
        "Tracks that were already transcribed are skipped.",
        "[F] Transcribe all again",
    ] {
        assert!(screen.contains(want), "missing {want:?}:\n{screen}");
    }
    // F, or a click on the button, flips it.
    rig.press('F');
    let screen = rig.draw();
    assert!(
        screen.contains("Every track is transcribed again.") && screen.contains("[F] Skip finished tracks"),
        "{screen}"
    );
    rig.press('F');
    assert!(rig.draw().contains("[F] Transcribe all again"));
    let btn = rig.app.hit.confirm_force;
    assert!(btn.width > 0);
    rig.click(btn.x + 1, btn.y);
    assert!(rig.draw().contains("[F] Skip finished tracks"));

    rig.app.on_key(key(KeyCode::Enter));
    rig.pump_until("the second run", |a| !a.running());
    let again = rig.app.report.clone().unwrap();
    assert_eq!((again.succeeded.len(), again.skipped.len()), (5, 0), "{again:?}");

    // The flip lasted one run.
    rig.press('s');
    assert!(rig.draw().contains("[F] Transcribe all again"));
    rig.app.on_key(key(KeyCode::Esc));
}

#[test]
fn rebuild_is_off_until_there_is_saved_output_and_says_why() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    assert!(!rig.app.enabled(Action::Reprocess));
    rig.press('p');
    assert!(rig.app.modal.is_none());
    assert_eq!(rig.app.status.kind, StatusKind::Warn);
    assert!(
        rig.app.status.text.contains("No saved whisper output"),
        "{}",
        rig.app.status.text
    );
}

#[test]
fn rebuild_rewrites_the_transcript_from_saved_output_with_no_tools_and_no_audio() {
    require_tools!();
    let mut rig = Rig::new(&[("STUB_THANKS", "1")]);
    rig.tone("1-gmperson.flac");
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    rig.pump_until("the first run", |a| !a.running());
    let first = rig.app.report.clone().unwrap();
    let path = first.transcript_md.clone().unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap().matches("Thank you.").count(), 1);
    assert!(rig.app.enabled(Action::Reprocess));

    // The audio folder is emptied and whisper is gone: a rebuild needs neither.
    fs::remove_dir_all(rig.app.session_paths().unwrap().audio_dir).unwrap();
    rig.env.insert("GM_WHISPER_BIN".into(), "/no/such/whisper-cli".into());
    rig.restart();
    assert_eq!(rig.app.session, Some(2), "a fresh start offers the next session");
    rig.press('n');
    rig.press('1');
    rig.app.on_key(key(KeyCode::Enter));
    assert_eq!(rig.app.session, Some(1));
    assert!(!rig.app.preflight.ok(), "transcribing would be refused");
    assert!(rig.app.enabled(Action::Reprocess), "rebuilding is not");

    rig.press('p');
    assert!(matches!(rig.app.modal, Some(Modal::Confirm(_))));
    let screen = rig.draw();
    for want in [
        "Rebuild the transcript?",
        "Rebuilds merged_transcript.tsv",
        "saved in archive/",
        "whisper does not run again",
        "transcript.md is rewritten",
        "[Enter] Rebuild",
        "Invented lines",
        "[t] Keep invented lines",
    ] {
        assert!(screen.contains(want), "missing {want:?}:\n{screen}");
    }
    assert!(
        !screen.contains("[F]"),
        "transcribing again means nothing here:\n{screen}"
    );
    rig.press('F');
    assert!(!rig.draw().contains("Every track is transcribed again"));

    // Turn the rule off for this rebuild: the dropped line comes back.
    rig.press('t');
    rig.app.on_key(key(KeyCode::Enter));
    assert!(rig.app.running());
    rig.pump_until("the rebuild", |a| !a.running());
    let report = rig.app.report.clone().unwrap();
    assert!(report.ok() && report.rebuilt, "{report:?}");
    assert_eq!(
        report.headline(),
        "Rebuilt the transcript from the saved whisper output."
    );
    assert_eq!(fs::read_to_string(&path).unwrap().matches("Thank you.").count(), 2);
    assert_eq!(rig.app.tab, Tab::Transcript);
    assert_eq!(rig.app.status.kind, StatusKind::Ok);
    assert_eq!(
        rig.app.status.text,
        "Rebuilt the transcript from the saved whisper output. transcript.md is in the Transcript tab."
    );
}

#[test]
fn escape_at_the_rebuild_step_changes_nothing() {
    require_tools!();
    let mut rig = Rig::new(&[]);
    rig.tone("1-gmperson.flac");
    rig.choose_all_in_downloads();
    rig.start_and_confirm();
    rig.pump_until("the first run", |a| !a.running());
    let path = rig.app.report.clone().unwrap().transcript_md.unwrap();
    fs::write(&path, "my hand edits").unwrap();
    rig.press('p');
    rig.app.on_key(key(KeyCode::Esc));
    assert!(rig.app.modal.is_none() && !rig.app.running());
    assert_eq!(fs::read_to_string(&path).unwrap(), "my hand edits");
}
