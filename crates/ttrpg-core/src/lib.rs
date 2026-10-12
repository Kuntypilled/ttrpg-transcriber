//! Core of the session transcriber: configuration, campaign registry, path
//! rules and the transcription job. No UI. Behaviour is ported from
//! `legacy/campaign.py` and checked against it by the parity tests (see
//! PORTING.md sections 3 and 9); the job runs `legacy/transcribe.sh`.

pub mod audio;
pub mod campaign;
pub mod error;
pub mod job;
pub mod legacy;
pub mod preflight;
pub mod progress;
pub mod pyish;
pub mod report;
pub mod settings;
pub mod state;

pub use audio::{AUDIO_EXTENSIONS, Speaker, resolve_speaker};
pub use campaign::{
    Campaign, CampaignPaths, DiscordEntry, SessionPaths, SpeakerRow, chunks_filename, list_campaigns,
    normalize_character, session_folder, split_slug,
};
pub use error::CampaignError;
pub use job::{JobPlan, PipelineEvent, RunReport, execute, plan_job};
pub use progress::{Phase, ProgressEvent, Tracker, parse_progress_line};
pub use settings::{Settings, Sourced};
