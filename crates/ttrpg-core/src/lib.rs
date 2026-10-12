//! Core of the session transcriber: configuration, campaign registry and path
//! rules. No UI. Behaviour is ported from `legacy/campaign.py` and checked
//! against it by the parity tests (see PORTING.md sections 3 and 9).

pub mod campaign;
pub mod error;
pub mod pyish;
pub mod report;
pub mod settings;

pub use campaign::{
    Campaign, CampaignPaths, DiscordEntry, SessionPaths, SpeakerRow, chunks_filename, list_campaigns,
    normalize_character, session_folder, split_slug,
};
pub use error::CampaignError;
pub use settings::{Settings, Sourced};
