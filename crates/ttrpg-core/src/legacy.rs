//! Finding the folder that holds `transcribe.sh`, `campaign.py` and friends.
//!
//! Until the Rust pipeline replaces them (PORTING.md section 11, milestone 5), the
//! TUI runs the original scripts. Order: `$GM_LEGACY_DIR` or the `legacy_dir`
//! config key, then a `legacy/` folder at or above the program, then the folder
//! in the source checkout this binary was built from.

use std::path::Path;

use crate::error::CampaignError;
use crate::settings::{Settings, Sourced};

/// The scripts the TUI shells out to.
pub const REQUIRED_SCRIPTS: [&str; 2] = ["transcribe.sh", "campaign.py"];

fn has_scripts(dir: &str) -> bool {
    REQUIRED_SCRIPTS.iter().all(|s| Path::new(dir).join(s).is_file())
}

/// The legacy script folder and how it was found.
pub fn locate(settings: &Settings) -> Result<Sourced, CampaignError> {
    if let Some(configured) = settings.legacy_dir()? {
        return if has_scripts(&configured.value) {
            Ok(configured)
        } else {
            Err(CampaignError::new(format!(
                "legacy_dir from {} is {}, which does not hold {}.",
                configured.source,
                configured.value,
                REQUIRED_SCRIPTS.join(" and ")
            )))
        };
    }
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().skip(1).take(6) {
            let candidate = ancestor.join("legacy").to_string_lossy().into_owned();
            if has_scripts(&candidate) {
                return Ok(Sourced {
                    value: candidate,
                    source: "next to the program".into(),
                });
            }
        }
    }
    let baked = concat!(env!("CARGO_MANIFEST_DIR"), "/../../legacy");
    if let Ok(dir) = Path::new(baked).canonicalize() {
        let dir = dir.to_string_lossy().into_owned();
        if has_scripts(&dir) {
            return Ok(Sourced {
                value: dir,
                source: "the source checkout this was built from".into(),
            });
        }
    }
    Err(CampaignError::new(format!(
        "Cannot find {}. Set GM_LEGACY_DIR, or \"legacy_dir\" in the config file, to the folder that holds them.",
        REQUIRED_SCRIPTS.join(" and ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(pairs: &[(&str, &str)]) -> Settings {
        let env: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Settings::from_env_map(env, "ttrpg-tui")
    }

    #[test]
    fn finds_the_checkout_by_default() {
        let found = locate(&settings(&[("HOME", "/nonexistent")])).unwrap();
        assert!(found.value.ends_with("/legacy"), "{found:?}");
        assert!(has_scripts(&found.value));
    }

    #[test]
    fn an_override_must_hold_the_scripts() {
        let err = locate(&settings(&[("HOME", "/h"), ("GM_LEGACY_DIR", "/nonexistent/legacy")])).unwrap_err();
        assert!(
            err.0.contains("$GM_LEGACY_DIR") && err.0.contains("/nonexistent/legacy"),
            "{err}"
        );
    }

    #[test]
    fn an_override_wins() {
        let real = locate(&settings(&[("HOME", "/h")])).unwrap().value;
        let found = locate(&settings(&[("HOME", "/h"), ("GM_LEGACY_DIR", &real)])).unwrap();
        assert_eq!(found.source, "$GM_LEGACY_DIR");
    }
}
