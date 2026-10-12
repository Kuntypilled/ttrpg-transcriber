//! Core of the session transcriber: configuration, campaign registry, path
//! rules and the transcription job. No UI. Behaviour is ported from
//! `tests/oracle/campaign.py` and checked against it by the parity tests (see
//! PORTING.md sections 3 and 9); the job runs in Rust ([`transcribe`]).

pub mod alias_report;
pub mod aliases;
pub mod audio;
pub mod campaign;
pub mod chunker;
pub mod difflib;
pub mod error;
pub mod index;
pub mod job;
pub mod memory;
pub mod options;
pub mod pipeline;
pub mod preflight;
pub mod proc;
pub mod progress;
pub mod pyish;
pub mod report;
pub mod settings;
pub mod state;
pub mod stock;
pub mod timefmt;
pub mod transcribe;

pub use aliases::{SaveReport, SpeakerInput, append_object_entries, discord_entries, save_speakers};
pub use audio::{AUDIO_EXTENSIONS, Speaker, resolve_speaker};
pub use campaign::{
    Campaign, CampaignPaths, DiscordEntry, SessionPaths, SpeakerRow, chunks_filename, list_campaigns,
    normalize_character, session_folder, split_slug,
};
pub use error::CampaignError;
pub use index::{IndexKind, IndexPlan, IndexReport, execute_index, plan_index};
pub use job::{JobPlan, PipelineEvent, RunReport, execute, has_saved_output, plan_job, plan_reprocess};
pub use memory::{MemoryCommand, run_memory_command};
pub use progress::{Phase, ProgressEvent, Tracker, is_whisper_segment_line};
pub use settings::{Settings, Sourced};
pub use stock::StockRule;
