//! Render tests: draw each state into an in-memory terminal and read the text back.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::style::{Color, Modifier};
use tempfile::TempDir;
use ttrpg_core::{PipelineEvent, ProgressEvent, RunReport, Settings};

use super::draw;
use crate::app::{Action, App, Modal, Tab};

fn key(c: char) -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char(c),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn text(buf: &Buffer) -> String {
    let w = usize::from(buf.area.width);
    if w == 0 {
        return String::new();
    }
    buf.content
        .chunks(w)
        .map(|row| {
            row.iter()
                .map(|c| c.symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| draw(f, app)).unwrap();
    text(term.backend().buffer())
}

struct World {
    tmp: TempDir,
    app: App,
}

impl World {
    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn choose(&mut self, names: &[&str]) {
        fs::create_dir_all(self.path("home/Downloads")).unwrap();
        self.app.chosen = names
            .iter()
            .map(|n| {
                let p = self.path("home/Downloads").join(n);
                fs::write(&p, "x").unwrap();
                p
            })
            .collect();
        self.app.refresh_tracks();
    }

    fn press(&mut self, c: char) {
        self.app.on_key(key(c));
    }

    fn scripted_job(&mut self) -> Sender<PipelineEvent> {
        let (tx, rx) = mpsc::channel();
        self.app.attach_job(rx, Arc::new(AtomicBool::new(false)), None);
        tx
    }
}

/// Two campaigns with invented data. With `tools`, fake executables on PATH make
/// the tool check pass; without, the check fails the same way on every machine.
fn world(tools: bool) -> World {
    let tmp = TempDir::new().unwrap();
    let camps = tmp.path().join("vault/0.2_campaigns");
    for (name, body) in [
        (
            "alpha",
            r#"{"campaign": "Alpha Run", "system": "daggerheart", "world": "emberfall",
                "players": {"Mara Quill": "Pat"},
                "review_only": {"X": ["y"]},
                "discord_names": {
                    "gmperson": {"player": "Sky", "character": null, "role": "GM"},
                    "riverone": {"player": "Pat", "character": "Mara_Quill", "role": "Player"}}}"#,
        ),
        ("beta", r#"{"campaign": "Beta Run", "system": "dnd-5e"}"#),
    ] {
        fs::create_dir_all(camps.join(name)).unwrap();
        fs::write(camps.join(name).join("canon_aliases.json"), body).unwrap();
    }
    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    if tools {
        for t in ["bash", "python3", "ffmpeg", "jq", "whisper-cli", "xdg-open"] {
            let p = bin.join(t);
            fs::write(&p, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(tmp.path().join("model.bin"), "x").unwrap();
    }
    let p = |rel: &str| tmp.path().join(rel).to_string_lossy().into_owned();
    let env = HashMap::from([
        ("HOME".to_string(), p("home")),
        ("PATH".to_string(), p("bin")),
        ("GM_VAULT".to_string(), p("vault")),
        ("GM_AUDIO_ROOT".to_string(), p("audio")),
        ("GM_CHROMA_ROOT".to_string(), p("chroma")),
        ("GM_WORK_ROOT".to_string(), p("work")),
        ("GM_TRANSCRIBER_CONFIG".to_string(), p("none.json")),
        ("GM_WHISPER_MODEL".to_string(), p("model.bin")),
    ]);
    let app = App::new(Settings::from_env_map(env, "ttrpg-tui"));
    World { tmp, app }
}

fn cell_has(buf: &Buffer, x: u16, y: u16, m: Modifier) -> bool {
    buf[(x, y)].modifier.contains(m)
}

// -- idle ---------------------------------------------------------------------------------

#[test]
fn idle_screen_shows_the_three_panes_and_the_buttons() {
    let mut w = world(true);
    let screen = render(&mut w.app, 140, 32);
    for want in [
        "SESSION TRANSCRIBER",
        "Alpha Run · daggerheart · emberfall",
        "Campaigns (2)",
        "Session",
        "Paths",
        "Log",
        "Transcript",
        "Session    [ 1 ]  → S01",
        "Files (0)",
        "none yet",
        "No audio yet.",
        "Ready",
        "[f] Files",
        "[s] Start",
        "[p] Rebuild",
        "[x] Cancel",
        "[o] Open folder",
        "[c] Clear files",
        "[r] Resolve",
        "[?] Help",
        "[q] Quit",
        "Press f to choose the Craig audio files",
    ] {
        assert!(screen.contains(want), "missing {want:?} in:\n{screen}");
    }
    assert!(!screen.contains("cannot transcribe"), "tools are present:\n{screen}");
}

#[test]
fn missing_tools_are_announced_in_the_header_and_status() {
    let mut w = world(false);
    let screen = render(&mut w.app, 150, 30);
    assert!(screen.contains("! cannot transcribe yet: missing"), "{screen}");
    assert!(screen.contains("whisper-cli") && screen.contains("press !"), "{screen}");
}

#[test]
fn buttons_are_dim_until_they_can_be_used() {
    let mut w = world(true);
    let mut term = Terminal::new(TestBackend::new(130, 32)).unwrap();
    term.draw(|f| draw(f, &mut w.app)).unwrap();
    let start = w.app.hit.buttons.iter().find(|(_, a)| *a == Action::Start).unwrap().0;
    assert!(
        cell_has(term.backend().buffer(), start.x, start.y, Modifier::DIM),
        "Start is disabled with no files"
    );
    w.choose(&["1-gmperson.flac"]);
    term.draw(|f| draw(f, &mut w.app)).unwrap();
    let buf = term.backend().buffer();
    assert!(
        !cell_has(buf, start.x, start.y, Modifier::DIM),
        "Start is enabled with a file chosen"
    );
    assert_eq!(buf[(start.x + 1, start.y)].fg, Color::Cyan);
}

// -- session pane --------------------------------------------------------------------------

#[test]
fn chosen_files_show_who_is_speaking_and_flag_the_unknown() {
    let mut w = world(true);
    w.choose(&["10-late.flac", "2-riverone.flac", "1-gmperson.flac"]);
    let screen = render(&mut w.app, 130, 32);
    assert!(
        screen.contains("Files (3)") && screen.contains("⚠ 1 unknown speaker(s)"),
        "{screen}"
    );
    assert!(
        screen.contains("→ GM (Sky)") && screen.contains("→ Mara_Quill (Pat)"),
        "{screen}"
    );
    assert!(screen.contains("→ ⚠ unknown (\"10-late\")"), "{screen}");
    let order: Vec<usize> = ["1-gmperson", "2-riverone", "10-late"]
        .iter()
        .map(|n| screen.find(n).unwrap())
        .collect();
    assert!(order.windows(2).all(|p| p[0] < p[1]), "natural order: 1, 2, 10");
    assert!(
        screen.contains("+ 1-gmperson.flac"),
        "a file that will be moved in is marked +"
    );
}

#[test]
fn all_resolved_says_so() {
    let mut w = world(true);
    w.choose(&["1-gmperson.flac", "2-riverone.flac"]);
    assert!(render(&mut w.app, 130, 32).contains("all speakers resolved ✓"));
}

#[test]
fn files_already_in_the_folder_are_listed_with_a_dot() {
    let mut w = world(true);
    let audio = w.app.session_paths().unwrap().audio_dir;
    fs::create_dir_all(&audio).unwrap();
    fs::write(PathBuf::from(&audio).join("1-gmperson.flac"), "x").unwrap();
    w.app.refresh_tracks();
    let screen = render(&mut w.app, 130, 32);
    assert!(screen.contains("· 1-gmperson.flac"), "{screen}");
    // a chosen file with the same name replaces it
    w.choose(&["1-gmperson.flac"]);
    assert!(render(&mut w.app, 130, 32).contains("↻ 1-gmperson.flac"));
}

#[test]
fn typing_a_session_number_shows_the_field_being_edited() {
    let mut w = world(true);
    w.press('n');
    w.press('1');
    w.press('2');
    let screen = render(&mut w.app, 130, 32);
    assert!(
        screen.contains("[12_ ]") && screen.contains("Enter to accept"),
        "{screen}"
    );
}

// -- running and finished ----------------------------------------------------------------------

#[test]
fn a_running_job_shows_progress_and_the_scripts_words() {
    let mut w = world(true);
    let tx = w.scripted_job();
    tx.send(PipelineEvent::Log("  Found 4 audio file(s)".into())).unwrap();
    tx.send(PipelineEvent::Progress(ProgressEvent::Start { total: 4 }))
        .unwrap();
    tx.send(PipelineEvent::Progress(ProgressEvent::FileStart {
        index: 2,
        total: 4,
        name: "2-riverone.flac".into(),
    }))
    .unwrap();
    tx.send(PipelineEvent::Log(
        "  Transcribing: 2-riverone.flac - speaker: Mara_Quill (22 MB)".into(),
    ))
    .unwrap();
    w.app.poll_job();
    let screen = render(&mut w.app, 130, 32);
    assert!(screen.contains("File 2 / 4: 2-riverone.flac"), "{screen}");
    assert!(
        screen.contains("Found 4 audio file(s)") && screen.contains("Transcribing: 2-riverone.flac"),
        "{screen}"
    );
    let bar = screen.lines().find(|l| l.contains("File 2 / 4")).unwrap();
    let filled = bar.matches('█').count();
    assert!((10..120).contains(&filled), "a partly filled bar, got {filled}: {bar}");
    assert!(screen.contains("Running. x cancels"));
    assert_eq!(w.app.tab, Tab::Log, "the log opens when a job starts");
}

#[test]
fn a_clean_finish_is_complete_and_green() {
    let mut w = world(true);
    let tx = w.scripted_job();
    tx.send(PipelineEvent::Progress(ProgressEvent::Start { total: 1 }))
        .unwrap();
    tx.send(PipelineEvent::Progress(ProgressEvent::PostDone)).unwrap();
    tx.send(PipelineEvent::Finished(Box::new(RunReport {
        session: 1,
        tracks_found: Some(1),
        succeeded: vec!["1-gmperson.flac".into()],
        postprocess_done: true,
        elapsed: Duration::from_secs(131),
        ..Default::default()
    })))
    .unwrap();
    w.app.poll_job();
    assert!(!w.app.running());
    let screen = render(&mut w.app, 130, 32);
    assert!(screen.contains("Complete   2:11"), "{screen}");
    assert!(screen.contains("1 of 1 transcribed"), "{screen}");
    assert!(w.app.report.as_ref().unwrap().ok());
}

#[test]
fn a_failed_track_turns_the_bar_red_and_is_named_in_the_log() {
    let mut w = world(true);
    let tx = w.scripted_job();
    tx.send(PipelineEvent::Progress(ProgressEvent::Start { total: 5 }))
        .unwrap();
    tx.send(PipelineEvent::Finished(Box::new(RunReport {
        session: 1,
        tracks_found: Some(5),
        succeeded: vec!["a".into(), "b".into(), "c".into(), "d".into()],
        failed: vec![("3-rivertwo.flac".into(), "whisper-cli exited non-zero".into())],
        postprocess_done: true,
        ..Default::default()
    })))
    .unwrap();
    w.app.poll_job();
    let mut term = Terminal::new(TestBackend::new(130, 32)).unwrap();
    term.draw(|f| draw(f, &mut w.app)).unwrap();
    let screen = text(term.backend().buffer());
    assert!(screen.contains("Finished with problems"), "{screen}");
    assert!(screen.contains("4 of 5 transcribed, 1 failed"), "{screen}");
    assert!(
        screen.contains("failed: 3-rivertwo.flac: whisper-cli exited non-zero"),
        "{screen}"
    );
    let gauge_row = screen
        .lines()
        .position(|l| l.contains("Finished with problems"))
        .unwrap() as u16;
    assert_eq!(term.backend().buffer()[(0, gauge_row)].fg, Color::Red);
    assert!(!screen.contains("Complete"), "never claims success");
}

#[test]
fn tracks_show_their_own_outcome_after_a_run_and_the_bar_resets_on_a_new_session() {
    let mut w = world(true);
    w.choose(&["1-gmperson.flac", "2-riverone.flac", "3-late.flac"]);
    let tx = w.scripted_job();
    tx.send(PipelineEvent::Finished(Box::new(RunReport {
        session: 1,
        tracks_found: Some(3),
        succeeded: vec!["1-gmperson.flac".into()],
        skipped: vec!["2-riverone.flac".into()],
        failed: vec![("3-late.flac".into(), "whisper-cli exited non-zero".into())],
        postprocess_done: true,
        ..Default::default()
    })))
    .unwrap();
    w.app.poll_job();
    let screen = render(&mut w.app, 150, 32);
    assert!(
        screen.contains("✓ 1-gmperson.flac") && screen.contains("✓ 2-riverone.flac"),
        "{screen}"
    );
    assert!(screen.contains("✗ 3-late.flac"), "{screen}");
    assert!(screen.contains("Finished with problems"));

    w.press('+'); // another session: last session's marks and bar do not apply
    let next = render(&mut w.app, 150, 32);
    assert!(
        !next.contains("✓ ") && !next.contains("✗ ") && !next.contains("Finished with problems"),
        "{next}"
    );
    assert!(next.contains("Ready"));
}

#[test]
fn a_narrow_session_pane_puts_the_speaker_under_the_name_and_clicks_still_land() {
    let mut w = world(true);
    w.choose(&["1-gmperson.flac", "2-riverone.flac", "3-longer-craig-name.flac"]);
    let narrow = render(&mut w.app, 96, 30);
    assert!(
        narrow.contains("+ 3-longer-craig-name.flac"),
        "the full name is shown:\n{narrow}"
    );
    assert_eq!(w.app.hit.track_row_height, 2);
    assert!(narrow.lines().any(|l| l.contains("→ Mara_Quill (Pat)")), "{narrow}");
    // Click the second line of the third track: it still selects the third track.
    let rows = w.app.hit.tracks;
    w.app.on_mouse(ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
        column: rows.x + 3,
        row: rows.y + 2 * 2 + 1,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(w.app.track_state.selected(), Some(2));

    let wide = render(&mut w.app, 200, 30);
    assert_eq!(w.app.hit.track_row_height, 1, "one line per track when there is room");
    assert!(wide.contains("+ 1-gmperson.flac") && wide.contains("→ GM (Sky)"));
}

#[test]
fn a_dropped_job_thread_is_reported_not_hung() {
    let mut w = world(true);
    drop(w.scripted_job()); // the sender goes away without a Finished event
    w.app.poll_job();
    assert!(!w.app.running());
    assert!(
        w.app
            .report
            .as_ref()
            .unwrap()
            .headline()
            .contains("stopped unexpectedly")
    );
}

#[test]
fn script_output_cannot_corrupt_the_screen() {
    let mut w = world(true);
    let tx = w.scripted_job();
    tx.send(PipelineEvent::Log("\x1b[31mred\x1b[0m\x07 and\ttabs \x1b[2J".into()))
        .unwrap();
    w.app.poll_job();
    let screen = render(&mut w.app, 130, 32);
    assert!(screen.contains("[31mred[0m and    tabs [2J"), "{screen}");
    assert!(!screen.contains('\x1b') && !screen.contains('\x07'));
}

// -- preview tabs ---------------------------------------------------------------------------------

#[test]
fn the_log_says_when_it_is_scrolled_back_and_how_to_follow() {
    let mut w = world(true);
    let tx = w.scripted_job();
    for i in 0..200 {
        tx.send(PipelineEvent::Log(format!("line {i}"))).unwrap();
    }
    w.app.poll_job();
    let tail = render(&mut w.app, 130, 32);
    assert!(
        tail.contains("line 199") && !tail.contains("line 0\n"),
        "follows the newest line"
    );
    w.app.scroll_preview(-20);
    let back = render(&mut w.app, 130, 32);
    assert!(back.contains("scrolled back 20 lines, G follows"), "{back}");
    assert!(!back.contains("line 199"));
    w.app.scroll_preview(1_000);
    assert!(
        render(&mut w.app, 130, 32).contains("line 199"),
        "scrolling forward past the end follows again"
    );
}

#[test]
fn whisper_segment_lines_are_dim_and_the_scripts_own_lines_are_not() {
    let mut w = world(true);
    let tx = w.scripted_job();
    tx.send(PipelineEvent::Log("Session folder: /somewhere".into()))
        .unwrap();
    tx.send(PipelineEvent::Log(
        "[00:00:00.000 --> 00:00:30.000]   Thank you.".into(),
    ))
    .unwrap();
    tx.send(PipelineEvent::Log(
        "[00:00:30.000 --> 00:00:41.200]   The lantern is lit.".into(),
    ))
    .unwrap();
    tx.send(PipelineEvent::Log(
        "ERROR: whisper-cli failed on: 3-rivertwo.flac".into(),
    ))
    .unwrap();
    w.app.poll_job();
    let mut term = Terminal::new(TestBackend::new(130, 32)).unwrap();
    term.draw(|f| draw(f, &mut w.app)).unwrap();
    let buf = term.backend().buffer();
    // Find the first cell of each line by its text.
    let at = |needle: &str| -> (u16, u16) {
        for y in 0..buf.area.height {
            let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            if let Some(col) = row.find(needle) {
                let x = row[..col].chars().count() as u16;
                return (x, y);
            }
        }
        panic!("{needle} is not on screen");
    };
    let (x, y) = at("[00:00:00.000");
    assert!(cell_has(buf, x, y, Modifier::DIM), "a whisper segment line is dim");
    let (x, y) = at("The lantern is lit.");
    assert!(
        cell_has(buf, x, y, Modifier::DIM),
        "the whole line is dim, text included"
    );
    let (x, y) = at("Session folder:");
    assert!(!cell_has(buf, x, y, Modifier::DIM), "the script's own line is normal");
    let (x, y) = at("ERROR: whisper-cli failed");
    assert!(!cell_has(buf, x, y, Modifier::DIM));
    assert_eq!(buf[(x, y)].fg, Color::Red, "errors keep their colour");
}

#[test]
fn a_long_log_line_keeps_its_style_on_every_row_it_wraps_to() {
    let mut w = world(true);
    let tx = w.scripted_job();
    tx.send(PipelineEvent::Log(format!("ERROR: {}", "boom ".repeat(60))))
        .unwrap();
    w.app.poll_job();
    let mut term = Terminal::new(TestBackend::new(130, 32)).unwrap();
    term.draw(|f| draw(f, &mut w.app)).unwrap();
    let buf = term.backend().buffer();
    let red_rows = (0..buf.area.height)
        .filter(|&y| (0..buf.area.width).any(|x| buf[(x, y)].symbol() == "b" && buf[(x, y)].fg == Color::Red))
        .count();
    assert!(
        red_rows >= 3,
        "a 300-character line wraps over several rows, all red: {red_rows}"
    );
}

#[test]
fn the_transcript_tab_styles_speaker_headers_and_hides_markdown() {
    let mut w = world(true);
    let sp = w.app.session_paths().unwrap();
    fs::create_dir_all(&sp.session_dir).unwrap();
    fs::write(
        &sp.transcript_md,
        "**[00:00:04] GM:**  \nThe road is long.\n\n**[00:01:10] Mara_Quill:**  \nWe go on.\n\n",
    )
    .unwrap();
    w.press('3');
    let mut term = Terminal::new(TestBackend::new(130, 32)).unwrap();
    term.draw(|f| draw(f, &mut w.app)).unwrap();
    let screen = text(term.backend().buffer());
    assert!(
        screen.contains("[00:00:04] GM") && screen.contains("[00:01:10] Mara_Quill"),
        "{screen}"
    );
    assert!(
        screen.contains("The road is long.") && !screen.contains("**["),
        "{screen}"
    );
    assert!(screen.contains("lines 1-"), "position shown:\n{screen}");
    let row = screen.lines().position(|l| l.contains("[00:00:04] GM")).unwrap();
    let line = screen.lines().nth(row).unwrap();
    let col = line[..line.find("[00:00:04] GM").unwrap()].chars().count();
    let cell = &term.backend().buffer()[(col as u16 + 1, row as u16)];
    assert_eq!(cell.fg, Color::Cyan);
    assert!(cell.modifier.contains(Modifier::BOLD));
}

#[test]
fn the_transcript_tab_explains_when_there_is_none() {
    let mut w = world(true);
    w.press('3');
    assert!(render(&mut w.app, 130, 32).contains("No transcript.md for session 1 yet."));
}

#[test]
fn the_paths_tab_lists_campaign_and_session_paths() {
    let mut w = world(true);
    let screen = render(&mut w.app, 150, 40);
    for want in [
        "slug",
        "alpha",
        "characters",
        "Mara_Quill",
        "review_only",
        "1 entries",
        "Session 1",
        "transcript.md",
        "work (temporary)",
    ] {
        assert!(screen.contains(want), "missing {want:?}:\n{screen}");
    }
}

#[test]
fn tabs_are_click_targets() {
    let mut w = world(true);
    render(&mut w.app, 130, 32);
    assert_eq!(w.app.hit.tabs.len(), 3);
    let (rect, tab) = w.app.hit.tabs[1];
    assert_eq!(tab, Tab::Log);
    w.app.on_mouse(ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(w.app.tab, Tab::Log);
}

// -- zoom ----------------------------------------------------------------------------------------------

#[test]
fn zoom_gives_the_preview_the_whole_screen_and_esc_restores() {
    let mut w = world(true);
    w.press('3');
    w.press('z');
    let zoomed = render(&mut w.app, 130, 32);
    assert!(
        !zoomed.contains("Campaigns (") && zoomed.contains("z or Esc restores the other panes"),
        "{zoomed}"
    );
    w.app.on_key(KeyEvent {
        code: KeyCode::Esc,
        ..key('x')
    });
    assert!(render(&mut w.app, 130, 32).contains("Campaigns (2)"));
}

// -- resolving unknown speakers ---------------------------------------------------------------

fn ctrl_key(c: char) -> KeyEvent {
    KeyEvent {
        modifiers: KeyModifiers::CONTROL,
        ..key(c)
    }
}

fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        app.on_key(key(c));
    }
}

#[test]
fn the_resolve_button_is_dim_until_a_track_is_unknown_and_says_why() {
    let mut w = world(true);
    w.choose(&["1-gmperson.flac", "2-riverone.flac"]);
    assert!(!w.app.enabled(Action::Resolve));
    w.press('r');
    assert!(w.app.modal.is_none());
    assert_eq!(w.app.status.text, "Every track already has a speaker.");
    w.choose(&["1-gmperson.flac", "3-newperson.flac"]);
    assert!(w.app.enabled(Action::Resolve));
    let screen = render(&mut w.app, 130, 32);
    assert!(
        screen.contains("[r] Resolve") && screen.contains("1 unknown speaker(s)"),
        "{screen}"
    );
}

#[test]
fn the_resolve_form_lists_the_unknown_names_the_characters_and_the_keys() {
    let mut w = world(true);
    w.choose(&[
        "1-gmperson.flac",
        "3-newperson.flac",
        "4-otherperson.flac",
        "5-newperson.flac",
    ]);
    w.press('r');
    assert!(matches!(w.app.modal, Some(Modal::Resolve(_))));
    let screen = render(&mut w.app, 130, 32);
    for want in [
        "Who is speaking?",
        "These tracks are not in alpha's discord_names.",
        "leave its row empty to skip it",
        "An empty character means the GM.",
        "TRACK",
        "PLAYER",
        "CHARACTER",
        "> newperson",
        "otherperson",
        "Saved as discord_names key \"newperson\", from 3-newperson.flac",
        "Characters: Mara_Quill",
        "Ctrl-N / Ctrl-P pick a character",
        "[Ctrl-S] Save to campaign",
        "[Esc] Cancel",
    ] {
        assert!(screen.contains(want), "missing {want:?}:\n{screen}");
    }
    let Some(Modal::Resolve(form)) = &w.app.modal else {
        panic!("the form is open")
    };
    assert_eq!(
        form.rows.iter().map(|r| r.stem.as_str()).collect::<Vec<_>>(),
        ["newperson", "otherperson"],
        "one row for a name that two tracks share"
    );
}

#[test]
fn typing_shows_in_the_focused_field_and_the_form_fits_a_small_terminal() {
    let mut w = world(true);
    w.choose(&["3-newperson.flac", "4-otherperson.flac"]);
    w.press('r');
    type_str(&mut w.app, "Kay");
    w.app.on_key(KeyEvent {
        code: KeyCode::Tab,
        ..key('x')
    });
    w.app.on_key(ctrl_key('n'));
    let Some(Modal::Resolve(form)) = &w.app.modal else {
        panic!("the form is open")
    };
    assert_eq!(
        form.field(0, crate::resolve::Col::Character),
        "Mara_Quill",
        "Ctrl-N filled it"
    );
    let screen = render(&mut w.app, 80, 24);
    assert!(screen.contains("Kay") && screen.contains("Mara_Quill"), "{screen}");
    assert!(
        screen.contains("[Ctrl-S] Save to campaign") && screen.contains("[Esc] Cancel"),
        "{screen}"
    );
    // The focused field is highlighted, the others underlined.
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| draw(f, &mut w.app)).unwrap();
    let (rect, _, _) = w.app.hit.resolve_fields[1];
    let buf = term.backend().buffer();
    assert!(
        buf[(rect.x, rect.y)].modifier.contains(Modifier::REVERSED),
        "the character field has focus"
    );
    let (rect, _, _) = w.app.hit.resolve_fields[0];
    assert!(buf[(rect.x, rect.y)].modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn many_unknown_names_scroll_with_the_selected_row_in_view() {
    let mut w = world(true);
    let names: Vec<String> = (1..=20).map(|i| format!("{i}-guest{i:02}.flac")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    w.choose(&refs);
    w.press('r');
    for _ in 0..15 {
        w.app.on_key(KeyEvent {
            code: KeyCode::Down,
            ..key('x')
        });
    }
    let screen = render(&mut w.app, 100, 30);
    assert!(screen.contains("> guest16"), "{screen}");
    assert!(screen.contains("row 16 of 20"), "{screen}");
    assert!(
        !screen.contains("guest01 "),
        "the first rows have scrolled out:\n{screen}"
    );
}

#[test]
fn clicking_a_field_focuses_it_and_clicking_save_writes_the_file() {
    let mut w = world(true);
    w.choose(&["3-newperson.flac"]);
    w.press('r');
    render(&mut w.app, 130, 32);
    let (rect, row, col) = w.app.hit.resolve_fields[1];
    assert_eq!((row, col), (0, crate::resolve::Col::Character));
    let click = |app: &mut App, x: u16, y: u16| {
        app.on_mouse(ratatui::crossterm::event::MouseEvent {
            kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
    };
    click(&mut w.app, rect.x + 1, rect.y);
    type_str(&mut w.app, "Zed Wick");
    render(&mut w.app, 130, 32);
    let save = w.app.hit.resolve_save;
    assert!(save.width > 0);
    click(&mut w.app, save.x + 1, save.y);
    assert!(w.app.modal.is_none());
    let file = fs::read_to_string(w.path("vault/0.2_campaigns/alpha/canon_aliases.json")).unwrap();
    assert!(
        file.contains("\"newperson\": {\"player\": null, \"character\": \"Zed_Wick\", \"role\": \"Player\"}"),
        "{file}"
    );
    assert!(
        w.app
            .status
            .text
            .starts_with("Added 1 speaker(s) to alpha's discord_names: newperson."),
        "{}",
        w.app.status.text
    );
    assert_eq!(w.app.unknown_tracks(), 0);
    assert!(render(&mut w.app, 130, 32).contains("Zed_Wick"));
}

// -- index and report ---------------------------------------------------------------------------

#[test]
fn a_campaign_without_review_only_carries_a_banner_that_changes_once_a_session_exists() {
    let mut w = world(true);
    // alpha has a review_only list: no banner.
    assert!(!render(&mut w.app, 130, 32).contains("no review_only"));
    // beta has none, and no session has been transcribed yet.
    w.press('j');
    w.app.on_key(KeyEvent {
        code: KeyCode::Down,
        ..key('x')
    });
    assert_eq!(w.app.campaign().unwrap().slug, "beta");
    let early = render(&mut w.app, 130, 32);
    assert!(early.contains("no review_only yet (a = alias report)"), "{early}");
    // Once a transcript exists the nudge turns into a warning.
    let sp = w.app.session_paths().unwrap();
    fs::create_dir_all(&sp.session_dir).unwrap();
    fs::write(&sp.transcript_md, "**[00:00:01] GM:**  \nHello.\n\n").unwrap();
    let later = render(&mut w.app, 130, 32);
    assert!(later.contains("⚠ no review_only list: a = alias report"), "{later}");
}

#[test]
fn the_button_bar_drops_the_least_used_buttons_first_and_never_help_or_quit() {
    let mut w = world(true);
    let wide = render(&mut w.app, 150, 30);
    for want in [
        "[f] Files",
        "[r] Resolve",
        "[s] Start",
        "[i] Index",
        "[a] Report",
        "[o] Open folder",
        "[c] Clear files",
    ] {
        assert!(wide.contains(want), "missing {want:?}:\n{wide}");
    }
    let narrow = render(&mut w.app, 70, 30);
    let bar = narrow.lines().last().unwrap().to_string();
    for want in ["[f] Files", "[s] Start", "[x] Cancel", "[?] Help", "[q] Quit"] {
        assert!(bar.contains(want), "missing {want:?} in {bar:?}");
    }
    assert!(!bar.contains("Clear files") && !bar.contains("Open folder"), "{bar}");
    // Every button that is drawn can be clicked.
    assert_eq!(w.app.hit.buttons.len(), bar.matches('[').count());
}

#[test]
fn the_index_question_and_the_help_describe_the_choices() {
    let mut w = world(true);
    w.app.modal = Some(Modal::NoReviewOnly);
    let ask = render(&mut w.app, 130, 32);
    for want in [
        "No review_only list",
        "never fixed automatically",
        "writes nothing to that file",
        "[Enter] Run the report first",
        "[i] Index anyway",
        "[Esc] Cancel",
    ] {
        assert!(ask.contains(want), "missing {want:?}:\n{ask}");
    }
    assert!(w.app.hit.ask_report.width > 0 && w.app.hit.ask_index.width > 0 && w.app.hit.ask_cancel.width > 0);
    w.app.modal = None;
    w.press('?');
    let help = render(&mut w.app, 130, 44);
    assert!(
        help.contains("index the session") && help.contains("alias report"),
        "{help}"
    );
}

// -- dialogs ---------------------------------------------------------------------------------------------

#[test]
fn help_lists_the_keys() {
    let mut w = world(true);
    w.press('?');
    let screen = render(&mut w.app, 130, 40);
    for want in [
        "Keys",
        "choose the Craig audio files",
        "cancel the running job",
        "zoom the right-hand pane",
        "any key closes this",
    ] {
        assert!(screen.contains(want), "missing {want:?}:\n{screen}");
    }
}

#[test]
fn the_tool_check_dialog_names_what_to_install() {
    let mut w = world(false);
    w.press('!');
    let screen = render(&mut w.app, 130, 50);
    assert!(
        screen.contains("Tools") && screen.contains("[MISSING]") && screen.contains("whisper model"),
        "{screen}"
    );
    assert!(
        screen.contains("whisper_bin") && screen.contains("GM_WHISPER_MODEL"),
        "{screen}"
    );
    assert!(
        screen.contains("huggingface.co/ggerganov/whisper.cpp"),
        "the model download is spelled out:\n{screen}"
    );
}

#[test]
fn the_confirm_dialog_says_move_and_warns_about_replacements_and_unknowns() {
    let mut w = world(true);
    w.choose(&["1-gmperson.flac", "9-stranger.flac"]);
    let audio = w.app.session_paths().unwrap().audio_dir;
    fs::create_dir_all(&audio).unwrap();
    fs::write(PathBuf::from(&audio).join("1-gmperson.flac"), "older").unwrap();
    w.press('s');
    assert!(matches!(w.app.modal, Some(Modal::Confirm(_))));
    let screen = render(&mut w.app, 130, 40);
    for want in [
        "Start transcription?",
        "alpha",
        "(S01)",
        "MOVE (not copy) 2 file(s) into:",
        "/audio/alpha/Session 1",
        "The originals disappear",
        "replace ones with the same name already there",
        "transcribed again",
        "1-gmperson.flac",
        "1 track(s) have no speaker in discord_names",
        "[Enter] Move and start",
        "[Esc] Cancel",
    ] {
        assert!(screen.contains(want), "missing {want:?}:\n{screen}");
    }
    assert!(w.app.hit.confirm_ok.width > 0 && w.app.hit.confirm_cancel.width > 0);
}

#[test]
fn the_confirm_dialog_for_a_retry_says_nothing_moves() {
    let mut w = world(true);
    let audio = w.app.session_paths().unwrap().audio_dir;
    fs::create_dir_all(&audio).unwrap();
    fs::write(PathBuf::from(&audio).join("1-gmperson.flac"), "x").unwrap();
    w.app.refresh_tracks();
    w.press('s');
    let screen = render(&mut w.app, 130, 40);
    assert!(
        screen.contains("Nothing to move. 1 file(s) are already in:"),
        "{screen}"
    );
    assert!(
        screen.contains("[Enter] Start") && !screen.contains("Move and start"),
        "{screen}"
    );
}

#[test]
fn the_picker_shows_folders_files_sizes_and_ticks() {
    let mut w = world(true);
    fs::create_dir_all(w.path("home/Downloads/craig-2026")).unwrap();
    fs::write(w.path("home/Downloads/1-gmperson.flac"), vec![0u8; 3 * 1024 * 1024]).unwrap();
    fs::write(w.path("home/Downloads/notes.txt"), "x").unwrap();
    w.press('f');
    w.press('a');
    let screen = render(&mut w.app, 130, 40);
    for want in [
        "Choose audio files",
        "/home/Downloads",
        ".. (parent folder)",
        "craig-2026/",
        "[x] 1-gmperson.flac",
        "3 MB",
        "[c] Use 1 file(s)",
        "Esc cancel",
    ] {
        assert!(screen.contains(want), "missing {want:?}:\n{screen}");
    }
    assert!(!screen.contains("notes.txt"), "only folders and audio files are listed");
}

// -- robustness -------------------------------------------------------------------------------------------

#[test]
fn nothing_panics_at_any_size_in_any_state() {
    let mut w = world(true);
    w.choose(&["1-gmperson.flac", "9-stranger.flac"]);
    let tx = w.scripted_job();
    tx.send(PipelineEvent::Progress(ProgressEvent::Start { total: 2 }))
        .unwrap();
    for i in 0..30 {
        tx.send(PipelineEvent::Log(format!(
            "a fairly long line of script output number {i} that must wrap"
        )))
        .unwrap();
    }
    w.app.poll_job();
    let sizes = [
        (0, 0),
        (1, 1),
        (2, 2),
        (10, 3),
        (20, 6),
        (30, 8),
        (60, 15),
        (80, 24),
        (130, 40),
        (300, 80),
    ];
    for tab in Tab::ALL {
        for zoom in [false, true] {
            for modal in 0..5 {
                w.app.tab = tab;
                w.app.zoom = zoom;
                w.app.modal = match modal {
                    1 => Some(Modal::Help),
                    2 => Some(Modal::Setup),
                    3 => Some(Modal::Picker(Box::new(crate::picker::FilePicker::new(
                        &w.path("home"),
                        &[],
                    )))),
                    4 => {
                        let s = &w.app.settings;
                        let c = w.app.campaign().unwrap();
                        let plan = ttrpg_core::plan_job(s, c, 1, &w.app.chosen).unwrap();
                        Some(Modal::Confirm(Box::new(plan)))
                    }
                    _ => None,
                };
                for (cols, rows) in sizes {
                    render(&mut w.app, cols, rows);
                }
            }
        }
    }
}
