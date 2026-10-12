//! Looking at a campaign's memory store from the command line: `ttrpg-tui info` and
//! `ttrpg-tui query` (PORTING.md section 7.4). Both run the Chroma helper
//! ([`crate::index::CHROMA_HELPER`]) with the paths this program works out, so nobody has to
//! know where the store is.

use std::process::{Command, Stdio};

use crate::campaign::Campaign;
use crate::error::CampaignError;
use crate::index::{helper_args, helper_env, store_path};
use crate::settings::Settings;

/// What to ask the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryCommand {
    Info,
    Query {
        text: String,
        session_from: Option<i64>,
        character: Option<String>,
        contains: Option<String>,
        k: u32,
    },
}

/// The arguments the helper gets, after the Python it runs under.
pub fn memory_args(
    settings: &Settings,
    campaign: &Campaign,
    cmd: &MemoryCommand,
) -> Result<Vec<String>, CampaignError> {
    let db = store_path(settings, campaign)?;
    let mut rest: Vec<String> = Vec::new();
    match cmd {
        MemoryCommand::Info => {
            rest.extend([
                "info".into(),
                "--db".into(),
                db,
                "--pcs".into(),
                campaign.pcs().join(","),
            ]);
        }
        MemoryCommand::Query {
            text,
            session_from,
            character,
            contains,
            k,
        } => {
            rest.extend(["query".into(), "--db".into(), db]);
            if let Some(n) = session_from {
                rest.extend(["--from".into(), n.to_string()]);
            }
            if let Some(c) = character {
                rest.extend(["--char".into(), c.clone()]);
            }
            if let Some(t) = contains {
                rest.extend(["--contains".into(), t.clone()]);
            }
            rest.extend(["-k".into(), k.to_string()]);
            // After the options, so text that starts with a dash is still the text.
            rest.extend(["--".into(), text.clone()]);
        }
    }
    Ok(helper_args(&rest))
}

/// The line `info` prints before the helper's own, naming the campaign.
pub fn campaign_line(campaign: &Campaign) -> String {
    let system = campaign.system();
    format!(
        "campaign   : {} ({}, {})",
        campaign.slug,
        campaign.label(),
        if system.is_empty() { "system unset" } else { &system }
    )
}

/// Run the command with the terminal attached and return the helper's exit code.
pub fn run_memory_command(settings: &Settings, slug: &str, cmd: &MemoryCommand) -> Result<i32, CampaignError> {
    let campaign = Campaign::load(settings, slug)?;
    let args = memory_args(settings, &campaign, cmd)?;
    let python = settings.effective_python()?;
    if *cmd == MemoryCommand::Info {
        println!("{}", campaign_line(&campaign));
    }
    let status = Command::new(&python)
        .args(&args)
        .env_clear()
        .envs(helper_env(settings))
        .stdin(Stdio::null())
        .status()
        .map_err(|e| {
            CampaignError::new(format!(
                "Could not start {python}: {e}. Set \"python\" in the config file to the Python that has ChromaDB."
            ))
        })?;
    Ok(status.code().unwrap_or(1))
}
