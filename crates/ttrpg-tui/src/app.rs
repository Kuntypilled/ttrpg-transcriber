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
    Campaign, IndexKind, IndexReport, JobPlan, PipelineEvent, RunReport, SessionPaths, Settings, StockRule, Tracker,
    execute, execute_index, has_saved_output, legacy, list_campaigns, plan_index, plan_job, plan_reprocess,
    save_speakers,
};

use crate::picker::{FilePicker, Outcome};
use crate::resolve::{self, Col, ResolveForm};
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
    Resolve,
    Start,
    Reprocess,
    Cancel,
    Index,
    Report,
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
    /// Who is speaking on the tracks `discord_names` does not know.
    Resolve(Box<ResolveForm>),
    /// Indexing a campaign that has no `review_only` list yet: report first, index anyway, or cancel.
    NoReviewOnly,
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

/// What a running job is doing, for the messages around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Transcribe,
    Index,
    Report,
}

/// A running job, owned by the UI thread.
pub struct JobState {
    pub kind: JobKind,
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
    pub confirm_stock: Rect,
    pub confirm_force: Rect,
    /// Where each field of the resolve form is drawn, and its Save and Cancel buttons.
    pub resolve_fields: Vec<(Rect, usize, Col)>,
    pub resolve_save: Rect,
    pub resolve_cancel: Rect,
    /// The three buttons of the "no review_only list" question.
    pub ask_report: Rect,
    pub ask_index: Rect,
    pub ask_cancel: Rect,
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
    /// How the last index or alias-report run ended.
    pub index_report: Option<IndexReport>,
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
            index_report: None,
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
            Action::Reprocess => {
                idle && self.campaign().is_some() && self.session.is_some_and(|n| n > 0) && self.has_saved_output()
            }
            Action::Cancel => !idle,
            Action::OpenFolder => self
                .session_paths()
                .is_some_and(|sp| Path::new(&sp.session_dir).is_dir()),
            Action::ClearFiles => idle && !self.chosen.is_empty(),
            Action::Resolve => idle && self.campaign().is_some() && self.unknown_tracks() > 0,
            Action::Index | Action::Report => idle && self.has_transcript(),
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
            Action::Reprocess => self.begin_reprocess(),
            Action::Cancel => self.cancel_job(),
            Action::OpenFolder => self.open_folder(),
            Action::ClearFiles => {
                self.chosen.clear();
                self.refresh_tracks();
                self.set_status(StatusKind::Info, "Cleared the chosen files.");
            }
            Action::Resolve => self.open_resolve(),
            Action::Index => self.begin_index(),
            Action::Report => self.start_index_job(IndexKind::Report),
            Action::Rescan => self.rescan(),
            Action::Help => self.modal = Some(Modal::Help),
            Action::Quit => self.request_quit(),
        }
    }

    /// A disabled button says why, instead of doing nothing.
    fn explain_disabled(&mut self, action: Action) {
        let why = match action {
            Action::ChooseFiles | Action::Rescan | Action::ClearFiles | Action::Resolve if self.running() => {
                "A job is running."
            }
            Action::Resolve if self.campaign().is_none() => "Pick a campaign first.",
            Action::Resolve => "Every track already has a speaker.",
            Action::Index | Action::Report if self.running() => "A job is running.",
            Action::Index | Action::Report if self.campaign().is_none() => "Pick a campaign first.",
            Action::Index | Action::Report => "There is no transcript.md for this session yet.",
            Action::ChooseFiles => "Pick a campaign first.",
            Action::Reprocess if self.running() => "A job is already running.",
            Action::Reprocess if self.campaign().is_none() => "Pick a campaign first.",
            Action::Reprocess if self.session.is_none() => "Set a session number first (n).",
            Action::Reprocess => "No saved whisper output for this session to rebuild from.",
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
        match plan_job(&self.settings, camp, session, &self.chosen) {
            Ok(plan) if plan.track_count() == 0 => {
                self.set_status(StatusKind::Warn, "No audio yet. Press f to choose the Craig files.");
            }
            Ok(plan) => self.modal = Some(Modal::Confirm(Box::new(plan))),
            Err(e) => self.set_status(StatusKind::Error, e.0),
        }
    }

    /// `p`: rebuild the transcripts from the saved whisper output. Needs no tools and no audio.
    fn begin_reprocess(&mut self) {
        let (Some(camp), Some(session)) = (self.campaign(), self.session) else {
            return;
        };
        match plan_reprocess(&self.settings, camp, session) {
            Ok(plan) => self.modal = Some(Modal::Confirm(Box::new(plan))),
            Err(e) => self.set_status(StatusKind::Error, e.0),
        }
    }

    /// Whether the selected session has saved whisper output.
    pub fn has_saved_output(&self) -> bool {
        match (self.campaign(), self.session) {
            (Some(camp), Some(n)) => has_saved_output(&self.settings, camp, n),
            _ => false,
        }
    }

    /// Flip the stock-phrase rule for this run only. Off turns it off; on goes back to
    /// what the settings say, or to the script's default if the settings turned it off.
    fn with_stock_toggled(&self, plan: JobPlan) -> JobPlan {
        let next = if plan.stock.enabled() {
            plan.stock.off()
        } else {
            self.settings
                .stock_rule()
                .ok()
                .filter(StockRule::enabled)
                .unwrap_or_default()
        };
        plan.with_stock_rule(next)
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
        self.attach_kind(JobKind::Transcribe, rx, cancel, handle);
    }

    /// Adopt the event channel of an index or alias-report job.
    pub fn attach_index_job(
        &mut self,
        kind: JobKind,
        rx: Receiver<PipelineEvent>,
        cancel: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    ) {
        self.attach_kind(kind, rx, cancel, handle);
    }

    fn attach_kind(
        &mut self,
        kind: JobKind,
        rx: Receiver<PipelineEvent>,
        cancel: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    ) {
        self.job = Some(JobState {
            kind,
            rx,
            cancel,
            handle,
            started: Instant::now(),
        });
        self.log.clear();
        self.log_back = 0;
        self.index_report = None;
        if kind == JobKind::Transcribe {
            self.report = None;
        }
        self.tracker = Tracker::default();
        self.tracker.starting();
        self.tab = Tab::Log;
        self.set_status(
            StatusKind::Info,
            match kind {
                JobKind::Transcribe => "Running. x cancels; the log shows what whisper says.",
                JobKind::Index => "Indexing. x cancels; the log shows what the helpers say.",
                JobKind::Report => "Scanning the transcript. x cancels.",
            },
        );
    }

    pub fn cancel_job(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
            self.set_status(StatusKind::Warn, "Cancelling: stopping the running job...");
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
        let mut index_finished: Option<IndexReport> = None;
        loop {
            let Some(job) = &self.job else { return };
            let kind = job.kind;
            match job.rx.try_recv() {
                Ok(PipelineEvent::Moving { index, total, name }) => self.tracker.moving(index, total, &name),
                Ok(PipelineEvent::Progress(p)) => self.tracker.apply(&p),
                Ok(PipelineEvent::Stage { label, fraction }) => self.tracker.stage(&label, fraction),
                Ok(PipelineEvent::Log(line)) => self.push_log(&line),
                Ok(PipelineEvent::Finished(report)) => {
                    finished = Some(*report);
                    break;
                }
                Ok(PipelineEvent::IndexFinished(report)) => {
                    index_finished = Some(*report);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let session = self.session.unwrap_or(0);
                    match kind {
                        JobKind::Transcribe => {
                            finished = Some(RunReport {
                                session,
                                failure: Some("The transcription job stopped unexpectedly.".into()),
                                ..RunReport::default()
                            });
                        }
                        JobKind::Index | JobKind::Report => {
                            let k = if kind == JobKind::Index {
                                IndexKind::Index
                            } else {
                                IndexKind::Report
                            };
                            index_finished = Some(IndexReport::failed(k, session, "The job stopped unexpectedly."));
                        }
                    }
                    break;
                }
            }
            self.dirty = true;
        }
        if let Some(report) = finished {
            self.finish_job(report);
        }
        if let Some(report) = index_finished {
            self.finish_index(report);
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
        self.index_report = None;

        // The moved files are no longer where they were chosen.
        self.chosen.retain(|p| p.exists());
        self.refresh_tracks();
        self.log_back = 0;
        if ok && has_transcript {
            self.set_tab(Tab::Transcript);
            self.transcript_scroll = 0;
            let stop = if headline.ends_with('.') { "" } else { "." };
            self.set_status(
                kind,
                format!("{headline}{stop} transcript.md is in the Transcript tab."),
            );
        } else {
            self.reload_transcript_if_visible();
            self.set_status(kind, headline);
        }
        if self.quit_after_job {
            self.should_quit = true;
        }
    }

    // -- indexing and the alias report ------------------------------------------------------

    fn has_transcript(&self) -> bool {
        self.session_paths()
            .is_some_and(|sp| Path::new(&sp.transcript_md).is_file())
    }

    /// Read the selected campaign again from disk: the GM may have edited its aliases
    /// since it was chosen.
    fn reload_selected(&mut self) -> Result<(), String> {
        let (Some(i), Some(slug)) = (self.list_state.selected(), self.selected().map(|(s, _)| s.clone())) else {
            return Err("Pick a campaign first.".into());
        };
        let fresh = Campaign::load(&self.settings, &slug).map_err(|e| e.0)?;
        if i < self.campaigns.len() {
            self.campaigns[i].1 = fresh;
        }
        Ok(())
    }

    /// `i`: index the transcript. A campaign with no `review_only` list is asked about first.
    fn begin_index(&mut self) {
        if let Err(e) = self.reload_selected() {
            self.set_status(StatusKind::Error, e);
            return;
        }
        if self.campaign().is_some_and(|c| !c.has_review_only()) {
            self.modal = Some(Modal::NoReviewOnly);
        } else {
            self.start_index_job(IndexKind::Index);
        }
    }

    fn start_index_job(&mut self, kind: IndexKind) {
        if let Err(e) = self.reload_selected() {
            self.set_status(StatusKind::Error, e);
            return;
        }
        let (Some(camp), Some(session)) = (self.campaign(), self.session) else {
            return;
        };
        let plan = legacy::locate(&self.settings).and_then(|l| plan_index(&self.settings, camp, session, kind, &l));
        let plan = match plan {
            Ok(p) => p,
            Err(e) => {
                self.set_status(StatusKind::Error, e.0);
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let handle = thread::spawn(move || execute_index(plan, tx, flag));
        let job_kind = if kind == IndexKind::Index {
            JobKind::Index
        } else {
            JobKind::Report
        };
        self.attach_index_job(job_kind, rx, cancel, Some(handle));
    }

    fn finish_index(&mut self, report: IndexReport) {
        if let Some(h) = self.job.take().and_then(|mut job| job.handle.take()) {
            let _ = h.join();
        }
        self.push_log("");
        self.push_log("== Result ==");
        self.push_log(&report.headline());
        if let Some(why) = &report.failure {
            for line in why.lines().skip(1) {
                self.push_log(line);
            }
        }
        if let Some(out) = &report.output {
            self.push_log(out);
        }
        let mut headline = report.headline();
        let kind = if report.ok() {
            StatusKind::Ok
        } else if report.cancelled {
            StatusKind::Warn
        } else {
            StatusKind::Error
        };
        if report.ok()
            && report.kind == IndexKind::Report
            && let Some(path) = &report.output
        {
            match (self.opener)(path) {
                Ok(()) => headline.push_str(" Opened alias_candidates.md."),
                Err(e) => headline.push_str(&format!(" It is at {path} (could not open it: {e}).")),
            }
        }
        if let Some(why) = &report.failure {
            // The first line says what failed; the rest says what to do.
            let more: Vec<&str> = why.lines().skip(1).collect();
            if !more.is_empty() {
                headline = format!("{headline} {}", more.join(" "));
            }
        }
        self.log_back = 0;
        self.index_report = Some(report);
        self.set_status(kind, headline);
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
            if key.code == KeyCode::Char('c') {
                if self.modal.take().is_none() {
                    self.on_interrupt();
                }
            } else if matches!(self.modal, Some(Modal::Resolve(_))) {
                // The resolve form is the one place Ctrl keys mean something else.
                if let Some(modal) = self.modal.take() {
                    self.modal_key(modal, key);
                }
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
            KeyCode::Char('p') => self.do_action(Action::Reprocess),
            KeyCode::Char('x') => self.do_action(Action::Cancel),
            KeyCode::Char('o') => self.do_action(Action::OpenFolder),
            KeyCode::Char('c') => self.do_action(Action::ClearFiles),
            KeyCode::Char('r') => self.do_action(Action::Resolve),
            KeyCode::Char('i') => self.do_action(Action::Index),
            KeyCode::Char('a') => self.do_action(Action::Report),
            KeyCode::Char('R') => self.do_action(Action::Rescan),
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
                KeyCode::Char('t') => self.modal = Some(Modal::Confirm(Box::new(self.with_stock_toggled(*plan)))),
                KeyCode::Char('F') if !plan.reprocess => {
                    let force = !plan.force;
                    self.modal = Some(Modal::Confirm(Box::new(plan.with_force(force))));
                }
                _ => self.modal = Some(Modal::Confirm(plan)),
            },
            Modal::Picker(mut picker) => match picker.on_key(key) {
                Outcome::Open => self.modal = Some(Modal::Picker(picker)),
                Outcome::Cancel => self.picker_dir = Some(picker.dir.clone()),
                Outcome::Done(files) => self.picked(files, picker.dir.clone()),
            },
            Modal::NoReviewOnly => match key.code {
                KeyCode::Enter | KeyCode::Char('a') | KeyCode::Char('r') => self.start_index_job(IndexKind::Report),
                KeyCode::Char('i') => self.start_index_job(IndexKind::Index),
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => {}
                _ => self.modal = Some(Modal::NoReviewOnly),
            },
            Modal::Resolve(mut form) => match form.on_key(key) {
                resolve::Outcome::Open => self.modal = Some(Modal::Resolve(form)),
                resolve::Outcome::Cancel => {}
                resolve::Outcome::Save => self.save_resolve(form),
            },
        }
    }

    // -- unknown speakers --------------------------------------------------------------

    /// One row per distinct unknown Discord name among the tracks, with a file that has it.
    fn unknown_speakers(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for t in &self.tracks {
            if let Speaker::Unknown { stem, .. } = &t.speaker
                && !out.iter().any(|(s, _)| s == stem)
            {
                out.push((stem.clone(), t.name.clone()));
            }
        }
        out
    }

    fn open_resolve(&mut self) {
        let Some(camp) = self.campaign() else { return };
        let form = ResolveForm::new(&camp.slug, self.unknown_speakers(), camp.pcs());
        self.modal = Some(Modal::Resolve(Box::new(form)));
    }

    /// Write the filled-in rows into the campaign's `discord_names`, reload the campaign
    /// and re-label the tracks. A failure keeps the form open with the reason.
    fn save_resolve(&mut self, mut form: Box<ResolveForm>) {
        if form.filled() == 0 {
            form.message = Some("Nothing is filled in. Type a player or a character, or press Esc.".into());
            self.modal = Some(Modal::Resolve(form));
            return;
        }
        let Some(camp) = self.campaign() else { return };
        let slug = camp.slug.clone();
        let report = match save_speakers(camp, &form.inputs()) {
            Ok(r) => r,
            Err(e) => {
                form.message = Some(e.0);
                self.modal = Some(Modal::Resolve(form));
                return;
            }
        };
        let reloaded = Campaign::load(&self.settings, &slug);
        match (reloaded, self.list_state.selected()) {
            (Ok(c), Some(i)) if i < self.campaigns.len() => self.campaigns[i].1 = c,
            (Err(e), _) => {
                self.set_status(
                    StatusKind::Error,
                    format!("Saved, but {slug} could not be reloaded: {}", e.0),
                );
                return;
            }
            _ => {}
        }
        self.refresh_tracks();
        let mut msg = if report.added.is_empty() {
            "Nothing was added: those names are already in discord_names.".to_string()
        } else {
            self.push_log(&format!("Added to {slug} discord_names: {}", report.added.join(", ")));
            format!(
                "Added {} speaker(s) to {slug}'s discord_names: {}.",
                report.added.len(),
                report.added.join(", ")
            )
        };
        if !report.left_alone.is_empty() {
            msg.push_str(&format!(
                " Left alone, already listed without a character or GM role: {}. Edit canon_aliases.json for those.",
                report.left_alone.join(", ")
            ));
        }
        let still = self.unknown_tracks();
        if still > 0 {
            msg.push_str(&format!(" {still} track(s) still unknown."));
        }
        let kind = if still == 0 && !report.added.is_empty() {
            StatusKind::Ok
        } else {
            StatusKind::Warn
        };
        self.set_status(kind, msg);
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
                format!("{n} file(s) chosen; {unknown} unknown speaker(s). r resolves them, s starts."),
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
                } else if left_click && self.hit.confirm_stock.contains(pos) {
                    self.modal = Some(Modal::Confirm(Box::new(self.with_stock_toggled(*plan))));
                } else if left_click && !plan.reprocess && self.hit.confirm_force.contains(pos) {
                    let force = !plan.force;
                    self.modal = Some(Modal::Confirm(Box::new(plan.with_force(force))));
                } else if !(left_click && self.hit.confirm_cancel.contains(pos)) {
                    self.modal = Some(Modal::Confirm(plan));
                }
            }
            Modal::Picker(mut picker) => match picker.on_mouse(m) {
                Outcome::Open => self.modal = Some(Modal::Picker(picker)),
                Outcome::Cancel => self.picker_dir = Some(picker.dir.clone()),
                Outcome::Done(files) => self.picked(files, picker.dir.clone()),
            },
            Modal::NoReviewOnly => {
                if left_click && self.hit.ask_report.contains(pos) {
                    self.start_index_job(IndexKind::Report);
                } else if left_click && self.hit.ask_index.contains(pos) {
                    self.start_index_job(IndexKind::Index);
                } else if !(left_click && self.hit.ask_cancel.contains(pos)) {
                    self.modal = Some(Modal::NoReviewOnly);
                }
            }
            Modal::Resolve(mut form) => {
                if !left_click {
                    self.modal = Some(Modal::Resolve(form));
                } else if self.hit.resolve_save.contains(pos) {
                    self.save_resolve(form);
                } else if self.hit.resolve_cancel.contains(pos) {
                    // The same as Esc: it asks first when something is typed.
                    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
                    if form.on_key(esc) == resolve::Outcome::Open {
                        self.modal = Some(Modal::Resolve(form));
                    }
                } else {
                    if let Some((_, row, col)) = self.hit.resolve_fields.iter().find(|(r, _, _)| r.contains(pos)) {
                        form.focus(*row, *col);
                    }
                    self.modal = Some(Modal::Resolve(form));
                }
            }
        }
    }
}
