//! `ttrpg-tui`: terminal front end for the session transcriber.
//!
//! With no subcommand it opens the TUI. `doctor`, `list`, `paths` and
//! `whisper-device` are the plain-text commands of the original `campaign.py`.

mod app;
mod log;
mod ui;

use std::io::{IsTerminal, stdout};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ttrpg_core::Settings;
use ttrpg_core::report::{doctor, list_output, paths_output, whisper_device_output};

use crate::app::App;

const CLI_NAME: &str = "ttrpg-tui";

#[derive(Parser)]
#[command(name = CLI_NAME, version, about = "Terminal front end for the TTRPG session transcriber")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show what every root resolved to, and from where
    Doctor,
    /// slug, label and system of every campaign, tab-separated
    List,
    /// Print the configured whisper-cli device index, or nothing
    WhisperDevice,
    /// Shell-eval-able KEY='value' path resolution for a campaign
    Paths {
        /// Campaign id: <world>/<campaign> with a vaults root, else the folder name
        #[arg(long)]
        campaign: String,
        #[arg(long, allow_hyphen_values = true)]
        session: Option<i64>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let settings = Settings::from_process_env(CLI_NAME);
    match cli.command {
        None => match run_tui(settings) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{CLI_NAME}: {e:#}");
                ExitCode::FAILURE
            }
        },
        Some(cmd) => ExitCode::from(run_command(&settings, cmd)),
    }
}

/// The plain-text commands. Exit codes follow `campaign.py`: 2 for a problem the
/// user can fix (printed as `ERROR: ...` on stderr), and for `doctor` 1 when no
/// campaign was found.
fn run_command(settings: &Settings, cmd: Command) -> u8 {
    let result = match cmd {
        Command::List => {
            print!("{}", list_output(settings));
            Ok(0)
        }
        Command::WhisperDevice => whisper_device_output(settings).map(|o| {
            print!("{o}");
            0
        }),
        Command::Paths { campaign, session } => paths_output(settings, &campaign, session).map(|o| {
            print!("{o}");
            0
        }),
        Command::Doctor => {
            let report = doctor(settings);
            print!("{}", report.out);
            report.result
        }
    };
    match result {
        Ok(code) => code as u8,
        Err(e) => {
            eprintln!("ERROR: {e}");
            2
        }
    }
}

fn run_tui(settings: Settings) -> Result<()> {
    if !stdout().is_terminal() || !std::io::stdin().is_terminal() {
        bail!("the TUI needs an interactive terminal. For plain output use: {CLI_NAME} list | doctor | paths");
    }
    let log_path = log::log_path(&settings);
    let mut app = App::new(settings);
    log::append(&log_path, &format!("startup: {} campaign(s)", app.campaigns.len()));

    // `ratatui::init` enters raw mode and the alternate screen and installs a panic
    // hook that restores them. Ours runs first: it also turns mouse capture off and
    // records the panic, then hands over to ratatui's hook.
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture).context("enable mouse capture")?;
    let previous = std::panic::take_hook();
    let hook_log = log_path.clone();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        log::append(&hook_log, &format!("PANIC: {info}"));
        previous(info);
    }));

    let result = event_loop(&mut terminal, &mut app);

    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    match &result {
        Ok(()) => log::append(&log_path, "exit: normal"),
        Err(e) => log::append(&log_path, &format!("exit: error: {e:#}")),
    }
    result
}

/// Redraws only when something changed. The poll timeout is the tick that later
/// milestones will use to drain background-job events.
fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.should_quit {
        if app.dirty {
            terminal.draw(|frame| ui::draw(frame, app))?;
            app.dirty = false;
        }
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                Event::Resize(..) => app.dirty = true,
                _ => {}
            }
        }
    }
    Ok(())
}
