//! Application state and input handling. No drawing and no terminal: everything
//! here is driven by feeding it key and mouse events and by polling the job, so
//! the whole program can be exercised in tests.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use ttrpg_core::audio::{Speaker, list_audio_files, natural_cmp, resolve_speaker};
use ttrpg_core::preflight::{PreflightReport, preflight};
use ttrpg_core::{
    Campaign, JobPlan, PipelineEvent, RunReport, SessionPaths, Settings, Tracker, execute, legacy, list_campaigns,
    plan_job,
};

use crate::picker::{FilePicker, Outcome};
use crate::text;

/// Each campaign takes two terminal rows in the list: the name, then `slug · characters`.
pub const ITEM_HEIGHT: u16 = 2;
/// Lines moved per wheel notch.
const WHEEL_STEP: isize = 3;
/// Oldest lines are dropped beyond this.
const LOG_CAP: usize = 5000;
const SESSION_MAX_DIGITS: usize = 4;
/// A transcript larger than this is shown truncated.
const TRANSCRIPT_CAP: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Campaigns,
    Session,
    Preview,
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Focus::Campaigns => Focus::Session,
            Focus::Session => Focus::Preview,
            Focus::Preview => Focus::Campaigns,
        }
    }
    fn prev(self) -> Self {
        match self {
            Focus::Campaigns => Focus::Preview,
            Focus::Session => Focus::Campaigns,
            Focus::Preview => Focus::Session,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Paths,
    Log,
    Transcript,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Paths, Tab::Log, Tab::Transcript];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Paths => "Paths",
            Tab::Log => "Log",
            Tab::Transcript => "Transcript",
        }
    }
    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
    fn step(self, by: isize) -> Tab {
        Self::ALL[(self.index() as isize + by).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ChooseFiles,
    Start,
    Cancel,
    OpenFolder,
    ClearFiles,
    Rescan,
    Help,
    Quit,
}

pub enum Modal {
    Help,
    /// What is installed and what is missing.
    Setup,
    Confirm(Box<JobPlan>),
    Picker(Box<FilePicker>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub text: String,
    pub kind: StatusKind,
}

/// One audio track of the session: chosen to be moved in, already in the folder, or both
/// (a chosen file will replace the one in the folder).
#[derive(Debug, Clone)]
pub struct Track {
    pub name: String,
    pub speaker: Speaker,
    pub incoming: bool,
    pub in_folder: bool,
}

pub struct Transcript {
    pub path: String,
    pub lines: Vec<String>,
}

/// A running job, owned by the UI thread.
pub struct JobState {
    rx: Receiver<PipelineEvent>,
    pub cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    pub started: Instant,
}

/// Screen rectangles of the last draw, for the mouse.
#[derive(Debug, Clone, Default)]
pub struct Hit {
    pub campaigns: Rect,
    pub session_area: Rect,
    pub session_field: Rect,
    pub tracks: Rect,
    /// Terminal rows per track in the list (1, or 2 when the speaker sits under the name).
    pub track_row_height: u16,
    pub preview_area: Rect,
    pub preview_inner: Rect,
    pub tabs: Vec<(Rect, Tab)>,
    pub buttons: Vec<(Rect, Action)>,
    pub confirm_ok: Rect,
    pub confirm_cancel: Rect,
}

pub type Opener = Box<dyn Fn(&str) -> std::io::Result<()>>;

/// `xdg-open`, detached, with its exit reaped on another thread.
fn xdg_open(path: &str) -> std::io::Result<()> {
    let mut child = Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub struct App {
    pub settings: Settings,
    pub campaigns: Vec<(String, Campaign)>,
    pub list_state: ListState,
    pub focus: Focus,
    pub tab: Tab,
    /// The preview pane fills the screen (for reading the transcript).
    pub zoom: bool,
    pub modal: Option<Modal>,
    pub preflight: PreflightReport,

    pub session: Option<i64>,
    /// Digits being typed into the session field.
    pub editing: Option<String>,
    pub chosen: Vec<PathBuf>,
    pub tracks: Vec<Track>,
    pub track_state: ListState,
    picker_dir: Option<PathBuf>,

    pub job: Option<JobState>,
    pub tracker: Tracker,
    pub log: VecDeque<String>,
    /// How many lines the log view is scrolled back from the newest (0 follows).
    pub log_back: usize,
    pub report: Option<RunReport>,
    pub transcript: Option<Transcript>,
    pub transcript_scroll: usize,
    pub paths_scroll: usize,

    pub status: Status,
    pub tick: u64,
    quit_armed: bool,
    quit_after_job: bool,
    pub should_quit: bool,
    /// Something changed since the last draw.
    pub dirty: bool,
    pub hit: Hit,
    opener: Opener,
}

impl App {
    pub fn new(settings: Settings) -> Self {
        let campaigns = list_campaigns(&settings);
        let preflight = preflight(&settings);
        let mut app = App {
            settings,
            campaigns,
            list_state: ListState::default(),
            focus: Focus::Campaigns,
            tab: Tab::Paths,
            zoom: false,
            modal: None,
            preflight,
            session: None,
            editing: None,
            chosen: Vec::new(),
            tracks: Vec::new(),
            track_state: ListState::default(),
            picker_dir: None,
            job: None,
            tracker: Tracker::default(),
            log: VecDeque::new(),
            log_back: 0,
            report: None,
            transcript: None,
            transcript_scroll: 0,
            paths_scroll: 0,
            status: Status {
                text: String::new(),
                kind: StatusKind::Info,
            },
            tick: 0,
            quit_armed: false,
            quit_after_job: false,
            should_quit: false,
            dirty: true,
            hit: Hit::default(),
            opener: Box::new(xdg_open),
        };
        if !app.campaigns.is_empty() {
            app.list_state.select(Some(0));
        }
        app.on_campaign_changed();
        app.hint_for_idle();
        app
    }

    /// Replace what `Open folder` does (tests).
    pub fn set_opener(&mut self, opener: Opener) {
        self.opener = opener;
    }

    // -- derived state -------------------------------------------------------------

    pub fn selected(&self) -> Option<&(String, Campaign)> {
        self.list_state.selected().and_then(|i| self.campaigns.get(i))
    }

    pub fn campaign(&self) -> Option<&Campaign> {
        self.selected().map(|(_, c)| c)
    }

    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    pub fn session_paths(&self) -> Option<SessionPaths> {
        self.campaign()?.session_paths(&self.settings, self.session?).ok()
    }

    pub fn unknown_tracks(&self) -> usize {
        self.tracks.iter().filter(|t| !t.speaker.is_resolved()).count()
    }

    pub fn set_status(&mut self, kind: StatusKind, text: impl Into<String>) {
        self.status = Status {
            text: text.into(),
            kind,
        };
        self.dirty = true;
    }

    fn hint_for_idle(&mut self) {
        if !self.preflight.ok() {
            let missing = self.preflight.missing_names();
            self.set_status(
                StatusKind::Warn,
                format!("Cannot transcribe yet: missing {missing}. Press ! for what to install."),
            );
        } else if self.campaigns.is_empty() {
            self.set_status(StatusKind::Info, "No campaigns found. See the details pane for setup.");
        } else {
            self.set_status(
                StatusKind::Info,
                "Press f to choose the Craig audio files, then s to start.",
            );
        }
    }

    /// Chosen files plus the audio already in the session folder, one row per name.
    pub fn refresh_tracks(&mut self) {
        let file_name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned());
        let mut rows: Vec<(String, bool, bool)> = Vec::new(); // name, incoming, in_folder
        let in_folder = match (self.campaign(), self.session) {
            (Some(camp), Some(n)) => camp
                .audio_dir(&self.settings, n)
                .map(|d| list_audio_files(&d))
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for name in self.chosen.iter().filter_map(file_name) {
            let there = in_folder.contains(&name);
            rows.push((name, true, there));
        }
        for name in in_folder {
            if !rows.iter().any(|r| r.0 == name) {
                rows.push((name, false, true));
            }
        }
        rows.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        let tracks: Vec<Track> = match self.campaign() {
            Some(camp) => rows
                .into_iter()
                .map(|(name, incoming, in_folder)| Track {
                    speaker: resolve_speaker(camp, &name),
                    name,
                    incoming,
                    in_folder,
                })
                .collect(),
            None => Vec::new(),
        };
        self.tracks = tracks;
        let keep = self
            .track_state
            .selected()
            .unwrap_or(0)
            .min(self.tracks.len().saturating_sub(1));
        self.track_state
            .select(if self.tracks.is_empty() { None } else { Some(keep) });
        self.dirty = true;
    }

    // -- campaigns ------------------------------------------------------------------

    /// Forget the last run's result (bar and per-track marks), but not its log.
    fn clear_last_run(&mut self) {
        if !self.running() {
            self.report = None;
            self.tracker = Tracker::default();
        }
    }

    fn on_campaign_changed(&mut self) {
        self.clear_last_run();
        self.session = self.campaign().map(|c| c.suggest_session(&self.settings));
        self.editing = None;
        self.paths_scroll = 0;
        self.transcript_scroll = 0;
        self.refresh_tracks();
        self.reload_transcript_if_visible();
    }

    fn select_campaign(&mut self, idx: usize) {
        if idx >= self.campaigns.len() || self.list_state.selected() == Some(idx) {
            return;
        }
        if self.running() {
            self.set_status(
                StatusKind::Warn,
                "A job is running. Wait for it, or press x to cancel it.",
            );
            return;
        }
        self.list_state.select(Some(idx));
        self.on_campaign_changed();
        self.dirty = true;
    }

    fn move_campaign(&mut self, delta: isize) {
        if self.campaigns.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        self.select_campaign(cur.saturating_add_signed(delta).min(self.campaigns.len() - 1));
    }

    /// Rescan the vaults and the tools; keep the selected campaign if it is still there.
    pub fn rescan(&mut self) {
        if self.running() {
            return;
        }
        let before = self.selected().map(|(slug, _)| slug.clone());
        self.campaigns = list_campaigns(&self.settings);
        self.preflight = preflight(&self.settings);
        let idx = before
            .as_ref()
            .and_then(|slug| self.campaigns.iter().position(|(s, _)| s == slug))
            .or(if self.campaigns.is_empty() { None } else { Some(0) });
        let after = idx.and_then(|i| self.campaigns.get(i)).map(|(s, _)| s.clone());
        self.list_state.select(idx);
        if after == before {
            self.refresh_tracks();
            self.reload_transcript_if_visible();
        } else {
            self.on_campaign_changed();
        }
        let tools = if self.preflight.ok() {
            "All tools found."
        } else {
            "Some tools are missing (press !)."
        };
        self.set_status(
            StatusKind::Info,
            format!("Rescanned: {} campaign(s). {tools}", self.campaigns.len()),
        );
    }

    // -- session number ---------------------------------------------------------------

    fn begin_edit(&mut self) {
        if self.running() || self.campaigns.is_empty() {
            return;
        }
        self.focus = Focus::Session;
        self.editing = Some(String::new());
        self.dirty = true;
    }

    fn set_session(&mut self, n: i64) {
        if self.session != Some(n.max(1)) {
            self.clear_last_run();
        }
        self.session = Some(n.max(1));
        self.refresh_tracks();
        self.reload_transcript_if_visible();
        self.dirty = true;
    }

    fn adjust_session(&mut self, delta: i64) {
        if self.running() {
            return;
        }
        if let Some(cur) = self.session {
            self.set_session(cur + delta);
        }
    }

    fn edit_key(&mut self, key: KeyEvent) {
        let Some(buf) = self.editing.as_mut() else { return };
        match key.code {
            KeyCode::Char(c) if c.is_ascii_digit() && buf.len() < SESSION_MAX_DIGITS => buf.push(c),
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Enter => {
                let typed = std::mem::take(buf);
                self.editing = None;
                match typed.parse::<i64>() {
                    Ok(n) if n > 0 => self.set_session(n),
                    Ok(_) => self.set_status(StatusKind::Warn, "The session number must be 1 or higher."),
                    Err(_) => {} // nothing typed: keep the current number
                }
            }
            KeyCode::Esc => self.editing = None,
            _ => {}
        }
        self.dirty = true;
    }

    // -- transcript -------------------------------------------------------------------

    pub fn reload_transcript(&mut self) {
        self.transcript = self.session_paths().and_then(|sp| {
            let file = std::fs::File::open(&sp.transcript_md).ok()?;
            let mut bytes = Vec::new();
            file.take(TRANSCRIPT_CAP).read_to_end(&mut bytes).ok()?;
            let body = String::from_utf8_lossy(&bytes);
            Some(Transcript {
                path: sp.transcript_md,
                lines: body.lines().map(text::sanitize).collect(),
            })
        });
        self.dirty = true;
    }

    fn reload_transcript_if_visible(&mut self) {
        if self.tab == Tab::Transcript {
            self.reload_transcript();
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::Transcript {
            self.reload_transcript();
        }
        self.dirty = true;
    }

    // -- log ----------------------------------------------------------------------------

    pub fn push_log(&mut self, line: &str) {
        self.log.push_back(text::sanitize(line));
        if self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
        if self.log_back > 0 {
            self.log_back += 1;
        }
        self.dirty = true;
    }

    /// Scroll the preview pane: positive moves toward later content.
    pub fn scroll_preview(&mut self, lines: isize) {
        match self.tab {
            Tab::Log => self.log_back = self.log_back.saturating_add_signed(-lines).min(self.log.len()),
            Tab::Transcript => self.transcript_scroll = self.transcript_scroll.saturating_add_signed(lines),
            Tab::Paths => self.paths_scroll = self.paths_scroll.saturating_add_signed(lines),
        }
        self.dirty = true;
    }

    fn preview_to_start(&mut self) {
        match self.tab {
            Tab::Log => self.log_back = usize::MAX / 2,
            Tab::Transcript => self.transcript_scroll = 0,
            Tab::Paths => self.paths_scroll = 0,
        }
        self.dirty = true;
    }

    fn preview_to_end(&mut self) {
        match self.tab {
            Tab::Log => self.log_back = 0,
            Tab::Transcript => self.transcript_scroll = usize::MAX / 2,
            Tab::Paths => self.paths_scroll = usize::MAX / 2,
        }
        self.dirty = true;
    }

    // -- actions ----------------------------------------------------------------------------

    /// PORTING.md section 7.2's enablement rules, except that Start is also allowed for
    /// audio already in the session folder (a retry needs no new files).
    pub fn enabled(&self, action: Action) -> bool {
        let idle = !self.running();
        match action {
            Action::ChooseFiles => idle && self.campaign().is_some(),
            Action::Start => {
                idle && self.campaign().is_some() && self.session.is_some_and(|n| n > 0) && !self.tracks.is_empty()
            }
            Action::Cancel => !idle,
            Action::OpenFolder => self
                .session_paths()
                .is_some_and(|sp| Path::new(&sp.session_dir).is_dir()),
            Action::ClearFiles => idle && !self.chosen.is_empty(),
            Action::Rescan => idle,
            Action::Help | Action::Quit => true,
        }
    }

    pub fn do_action(&mut self, action: Action) {
        self.dirty = true;
        if action != Action::Quit {
            self.quit_armed = false;
        }
        if !self.enabled(action) {
            self.explain_disabled(action);
            return;
        }
        match action {
            Action::ChooseFiles => self.open_picker(),
            Action::Start => self.begin_start(),
            Action::Cancel => self.cancel_job(),
            Action::OpenFolder => self.open_folder(),
            Action::ClearFiles => {
                self.chosen.clear();
                self.refresh_tracks();
                self.set_status(StatusKind::Info, "Cleared the chosen files.");
            }
            Action::Rescan => self.rescan(),
            Action::Help => self.modal = Some(Modal::Help),
            Action::Quit => self.request_quit(),
        }
    }

    /// A disabled button says why, instead of doing nothing.
    fn explain_disabled(&mut self, action: Action) {
        let why = match action {
            Action::ChooseFiles | Action::Rescan | Action::ClearFiles if self.running() => "A job is running.",
            Action::ChooseFiles => "Pick a campaign first.",
            Action::Start if self.running() => "A job is already running.",
            Action::Start if self.campaign().is_none() => "Pick a campaign first.",
            Action::Start if self.tracks.is_empty() => "No audio yet. Press f to choose the Craig files.",
            Action::Start => "Set a session number first (n).",
            Action::Cancel => "Nothing is running.",
            Action::OpenFolder => "The session folder does not exist yet.",
            Action::ClearFiles => "No files are chosen.",
            _ => return,
        };
        self.set_status(StatusKind::Warn, why);
    }

    fn default_picker_start(&self) -> PathBuf {
        let home = self.settings.env_raw("HOME").unwrap_or("/");
        let downloads = Path::new(home).join("Downloads");
        if downloads.is_dir() {
            downloads
        } else {
            PathBuf::from(home)
        }
    }

    fn open_picker(&mut self) {
        let start = self.picker_dir.clone().unwrap_or_else(|| self.default_picker_start());
        self.modal = Some(Modal::Picker(Box::new(FilePicker::new(&start, &self.chosen))));
    }

    fn open_folder(&mut self) {
        let Some(sp) = self.session_paths() else { return };
        match (self.opener)(&sp.session_dir) {
            Ok(()) => self.set_status(StatusKind::Info, format!("Opened {}", sp.session_dir)),
            Err(e) => self.set_status(StatusKind::Error, format!("Could not open {}: {e}", sp.session_dir)),
        }
    }

    fn begin_start(&mut self) {
        self.preflight = preflight(&self.settings);
        if !self.preflight.ok() {
            self.modal = Some(Modal::Setup);
            self.set_status(
                StatusKind::Warn,
                format!("Cannot transcribe yet: missing {}.", self.preflight.missing_names()),
            );
            return;
        }
        let (Some(camp), Some(session)) = (self.campaign(), self.session) else {
            return;
        };
        let plan =
            legacy::locate(&self.settings).and_then(|l| plan_job(&self.settings, camp, session, &self.chosen, &l));
        match plan {
            Ok(plan) if plan.track_count() == 0 => {
                self.set_status(StatusKind::Warn, "No audio yet. Press f to choose the Craig files.");
            }
            Ok(plan) => self.modal = Some(Modal::Confirm(Box::new(plan))),
            Err(e) => self.set_status(StatusKind::Error, e.0),
        }
    }

    /// Start the job on its own thread. The UI never waits for it.
    fn launch(&mut self, plan: JobPlan) {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let handle = thread::spawn(move || execute(plan, tx, flag));
        self.attach_job(rx, cancel, Some(handle));
    }

    /// Adopt a job's event channel. Also how tests feed in a scripted job.
    pub fn attach_job(&mut self, rx: Receiver<PipelineEvent>, cancel: Arc<AtomicBool>, handle: Option<JoinHandle<()>>) {
        self.job = Some(JobState {
            rx,
            cancel,
            handle,
            started: Instant::now(),
        });
        self.log.clear();
        self.log_back = 0;
        self.report = None;
        self.tracker = Tracker::default();
        self.tracker.starting();
        self.tab = Tab::Log;
        self.set_status(
            StatusKind::Info,
            "Running. x cancels; the log shows what the script says.",
        );
    }

    pub fn cancel_job(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
            self.set_status(StatusKind::Warn, "Cancelling: stopping the script and its children...");
        }
    }

    /// `q`: quit, but a running job is cancelled first, and only on a second press.
    fn request_quit(&mut self) {
        if !self.running() {
            self.should_quit = true;
        } else if self.quit_armed {
            self.quit_after_job = true;
            self.cancel_job();
            self.set_status(StatusKind::Warn, "Cancelling, then quitting...");
        } else {
            self.quit_armed = true;
            self.set_status(
                StatusKind::Warn,
                "A job is running. Press q again to cancel it and quit.",
            );
        }
    }

    /// Ctrl-C stops a running job first, then quits.
    fn on_interrupt(&mut self) {
        if self.running() {
            self.cancel_job();
        } else {
            self.should_quit = true;
        }
    }

    // -- the running job ---------------------------------------------------------------------

    /// Drain pending job events. Call every loop iteration.
    pub fn poll_job(&mut self) {
        let mut finished: Option<RunReport> = None;
        loop {
            let Some(job) = &self.job else { return };
            match job.rx.try_recv() {
                Ok(PipelineEvent::Moving { index, total, name }) => self.tracker.moving(index, total, &name),
                Ok(PipelineEvent::Progress(p)) => self.tracker.apply(&p),
                Ok(PipelineEvent::Log(line)) => self.push_log(&line),
                Ok(PipelineEvent::Finished(report)) => {
                    finished = Some(*report);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = Some(RunReport {
                        session: self.session.unwrap_or(0),
                        failure: Some("The transcription job stopped unexpectedly.".into()),
                        ..RunReport::default()
                    });
                    break;
                }
            }
            self.dirty = true;
        }
        if let Some(report) = finished {
            self.finish_job(report);
        }
    }

    fn finish_job(&mut self, report: RunReport) {
        if let Some(h) = self.job.take().and_then(|mut job| job.handle.take()) {
            let _ = h.join();
        }
        self.push_log("");
        self.push_log("== Result ==");
        self.push_log(&report.headline());
        for (file, msg) in &report.failed {
            self.push_log(&format!("  failed: {file}: {msg}"));
        }
        for file in &report.skipped {
            self.push_log(&format!("  already done, skipped: {file}"));
        }
        match &report.transcript_md {
            Some(p) => self.push_log(&format!("transcript.md: {p}")),
            None => self.push_log("No transcript.md was written."),
        }

        let kind = if report.ok() {
            StatusKind::Ok
        } else if report.cancelled {
            StatusKind::Warn
        } else {
            StatusKind::Error
        };
        let headline = report.headline();
        let has_transcript = report.transcript_md.is_some();
        let ok = report.ok();
        self.report = Some(report);

        // The moved files are no longer where they were chosen.
        self.chosen.retain(|p| p.exists());
        self.refresh_tracks();
        self.log_back = 0;
        if ok && has_transcript {
            self.set_tab(Tab::Transcript);
            self.transcript_scroll = 0;
            self.set_status(kind, format!("{headline}. transcript.md is in the Transcript tab."));
        } else {
            self.reload_transcript_if_visible();
            self.set_status(kind, headline);
        }
        if self.quit_after_job {
            self.should_quit = true;
        }
    }

    /// Advance animation. Called on idle loop iterations.
    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self.running() {
            self.dirty = true;
        }
    }

    // -- keyboard ------------------------------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('c') && self.modal.take().is_none() {
                self.on_interrupt();
            }
            return;
        }
        if let Some(modal) = self.modal.take() {
            self.modal_key(modal, key);
            return;
        }
        if self.editing.is_some() {
            self.edit_key(key);
            return;
        }
        if key.code != KeyCode::Char('q') {
            self.quit_armed = false;
        }
        match key.code {
            KeyCode::Char('q') => self.do_action(Action::Quit),
            KeyCode::Char('?') => self.do_action(Action::Help),
            KeyCode::Char('!') => self.modal = Some(Modal::Setup),
            KeyCode::Char('f') => self.do_action(Action::ChooseFiles),
            KeyCode::Char('s') => self.do_action(Action::Start),
            KeyCode::Char('x') => self.do_action(Action::Cancel),
            KeyCode::Char('o') => self.do_action(Action::OpenFolder),
            KeyCode::Char('c') => self.do_action(Action::ClearFiles),
            KeyCode::Char('r') => self.do_action(Action::Rescan),
            KeyCode::Char('z') => self.toggle_zoom(),
            KeyCode::Esc if self.zoom => self.zoom = false,
            KeyCode::Char('n') => self.begin_edit(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.adjust_session(1),
            KeyCode::Char('-') => self.adjust_session(-1),
            KeyCode::Char('1') => self.set_tab(Tab::Paths),
            KeyCode::Char('2') => self.set_tab(Tab::Log),
            KeyCode::Char('3') => self.set_tab(Tab::Transcript),
            KeyCode::Char('[') => self.set_tab(self.tab.step(-1)),
            KeyCode::Char(']') => self.set_tab(self.tab.step(1)),
            KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => self.focus = self.focus.next(),
            KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => self.focus = self.focus.prev(),
            KeyCode::Enter => match self.focus {
                Focus::Campaigns => self.focus = Focus::Session,
                Focus::Session if self.tracks.is_empty() => self.do_action(Action::ChooseFiles),
                _ => {}
            },
            KeyCode::Char('j') | KeyCode::Down => self.move_in_focus(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_in_focus(-1),
            KeyCode::PageDown => self.page_in_focus(1),
            KeyCode::PageUp => self.page_in_focus(-1),
            KeyCode::Char('g') | KeyCode::Home => self.jump_in_focus(true),
            KeyCode::Char('G') | KeyCode::End => self.jump_in_focus(false),
            _ => self.dirty = false,
        }
    }

    fn toggle_zoom(&mut self) {
        self.zoom = !self.zoom;
        if self.zoom {
            self.focus = Focus::Preview;
        }
    }

    fn move_in_focus(&mut self, delta: isize) {
        match self.focus {
            Focus::Campaigns => self.move_campaign(delta),
            Focus::Session => self.move_track(delta),
            Focus::Preview => self.scroll_preview(delta),
        }
    }

    fn page_in_focus(&mut self, dir: isize) {
        let page = self.hit.preview_inner.height.max(3) as isize - 1;
        match self.focus {
            Focus::Campaigns => self.move_campaign(dir * 5),
            Focus::Session => self.move_track(dir * 8),
            Focus::Preview => self.scroll_preview(dir * page),
        }
    }

    fn jump_in_focus(&mut self, start: bool) {
        match (self.focus, start) {
            (Focus::Campaigns, true) => self.move_campaign(-(self.campaigns.len() as isize)),
            (Focus::Campaigns, false) => self.move_campaign(self.campaigns.len() as isize),
            (Focus::Session, true) => self.move_track(-(self.tracks.len() as isize)),
            (Focus::Session, false) => self.move_track(self.tracks.len() as isize),
            (Focus::Preview, true) => self.preview_to_start(),
            (Focus::Preview, false) => self.preview_to_end(),
        }
    }

    fn move_track(&mut self, delta: isize) {
        if self.tracks.is_empty() {
            return;
        }
        let cur = self.track_state.selected().unwrap_or(0);
        self.track_state
            .select(Some(cur.saturating_add_signed(delta).min(self.tracks.len() - 1)));
    }

    fn modal_key(&mut self, modal: Modal, key: KeyEvent) {
        match modal {
            Modal::Help | Modal::Setup => {
                // Any key closes; q still quits.
                if key.code == KeyCode::Char('q') {
                    self.should_quit = true;
                }
            }
            Modal::Confirm(plan) => match key.code {
                KeyCode::Enter | KeyCode::Char('y') => self.launch(*plan),
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => {}
                _ => self.modal = Some(Modal::Confirm(plan)),
            },
            Modal::Picker(mut picker) => match picker.on_key(key) {
                Outcome::Open => self.modal = Some(Modal::Picker(picker)),
                Outcome::Cancel => self.picker_dir = Some(picker.dir.clone()),
                Outcome::Done(files) => self.picked(files, picker.dir.clone()),
            },
        }
    }

    fn picked(&mut self, files: Vec<PathBuf>, dir: PathBuf) {
        self.picker_dir = Some(dir);
        let n = files.len();
        self.chosen = files;
        self.focus = Focus::Session;
        self.refresh_tracks();
        let unknown = self.unknown_tracks();
        let (kind, msg) = if unknown > 0 {
            (
                StatusKind::Warn,
                format!("{n} file(s) chosen; {unknown} unknown speaker(s). s starts."),
            )
        } else {
            (
                StatusKind::Info,
                format!("{n} file(s) chosen; all speakers resolved. s starts."),
            )
        };
        self.set_status(kind, msg);
    }

    // -- mouse -----------------------------------------------------------------------------------

    pub fn on_mouse(&mut self, m: MouseEvent) {
        let pos = Position::new(m.column, m.row);
        if let Some(modal) = self.modal.take() {
            self.modal_mouse(modal, m, pos);
            return;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click(pos),
            MouseEventKind::ScrollDown => self.wheel(pos, 1),
            MouseEventKind::ScrollUp => self.wheel(pos, -1),
            _ => {}
        }
    }

    fn click(&mut self, pos: Position) {
        self.dirty = true;
        if let Some((_, action)) = self.hit.buttons.iter().find(|(r, _)| r.contains(pos)).copied() {
            self.editing = None;
            self.do_action(action);
            return;
        }
        if let Some((_, tab)) = self.hit.tabs.iter().find(|(r, _)| r.contains(pos)).copied() {
            self.editing = None;
            self.focus = Focus::Preview;
            self.set_tab(tab);
            return;
        }
        if self.hit.session_field.contains(pos) {
            self.begin_edit();
            return;
        }
        self.editing = None; // a click anywhere else abandons the edit
        if self.hit.campaigns.contains(pos) {
            self.focus = Focus::Campaigns;
            let idx = self.list_state.offset() + usize::from((pos.y - self.hit.campaigns.y) / ITEM_HEIGHT);
            self.select_campaign(idx);
        } else if self.hit.tracks.contains(pos) {
            self.focus = Focus::Session;
            let idx =
                self.track_state.offset() + usize::from((pos.y - self.hit.tracks.y) / self.hit.track_row_height.max(1));
            if idx < self.tracks.len() {
                self.track_state.select(Some(idx));
            }
        } else if self.hit.session_area.contains(pos) {
            self.focus = Focus::Session;
        } else if self.hit.preview_area.contains(pos) {
            self.focus = Focus::Preview;
        }
    }

    fn wheel(&mut self, pos: Position, dir: isize) {
        self.dirty = true;
        if self.hit.campaigns.contains(pos) {
            self.move_campaign(dir);
        } else if self.hit.session_area.contains(pos) {
            self.move_track(dir * WHEEL_STEP);
        } else if self.hit.preview_area.contains(pos) {
            self.scroll_preview(dir * WHEEL_STEP);
        } else {
            self.dirty = false;
        }
    }

    fn modal_mouse(&mut self, modal: Modal, m: MouseEvent, pos: Position) {
        self.dirty = true;
        let left_click = matches!(m.kind, MouseEventKind::Down(MouseButton::Left));
        match modal {
            Modal::Help | Modal::Setup => {
                if !left_click {
                    self.modal = Some(modal);
                }
            }
            Modal::Confirm(plan) => {
                if left_click && self.hit.confirm_ok.contains(pos) {
                    self.launch(*plan);
                } else if !(left_click && self.hit.confirm_cancel.contains(pos)) {
                    self.modal = Some(Modal::Confirm(plan));
                }
            }
            Modal::Picker(mut picker) => match picker.on_mouse(m) {
                Outcome::Open => self.modal = Some(Modal::Picker(picker)),
                Outcome::Cancel => self.picker_dir = Some(picker.dir.clone()),
                Outcome::Done(files) => self.picked(files, picker.dir.clone()),
            },
        }
    }
}
