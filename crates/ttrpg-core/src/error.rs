/// A problem the user can fix: bad config, unknown campaign, invalid id.
/// The message is shown as-is (the CLI prefixes it with `ERROR: `).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CampaignError(pub String);

impl CampaignError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}
