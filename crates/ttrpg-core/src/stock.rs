//! The stock-phrase rule of `legacy/transcribe.sh` as a setting.
//!
//! Whisper invents "Thank you." on a silent stretch of a track, one per 30 s
//! window, and the script's collapse step joins the repeats into one long line.
//! The script drops a line that is exactly one of these phrases when it lasts
//! `min_seconds` or more (see `legacy/README.md` and `ERRATA.md` for the reasoning).
//! The defaults here are the script's own; a test keeps them equal.
//!
//! Settings, in the usual order (environment, then the config file):
//!
//! | Setting | Environment | Config key | Default |
//! |---|---|---|---|
//! | phrases | `GM_STOCK_PHRASES` (comma-separated, or `off`) | `stock_phrases` (list, comma-separated text, or `off`; `[]` is off) | `thank you`, `we'll be right back` |
//! | length | `GM_STOCK_MIN_SECONDS` | `stock_min_seconds` | 20 |

use serde_json::Value;

use crate::error::CampaignError;
use crate::pyish;
use crate::settings::Settings;

pub const PHRASES_ENV: &str = "GM_STOCK_PHRASES";
pub const SECONDS_ENV: &str = "GM_STOCK_MIN_SECONDS";
pub const DEFAULT_PHRASES: [&str; 2] = ["thank you", "we'll be right back"];
pub const DEFAULT_MIN_SECONDS: u32 = 20;
const TURNED_OFF: &str = "turned off for this run";
/// A day. Anything longer is a typo, not a setting.
const MAX_SECONDS: u32 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockRule {
    /// Lines that are exactly one of these (case and one trailing mark ignored)
    /// are dropped when long enough. Empty means the rule is off.
    pub phrases: Vec<String>,
    pub min_seconds: u32,
    /// Where a non-default value came from: `$GM_STOCK_PHRASES`, the config file,
    /// "turned off for this run". `None` when everything is the default.
    pub set_by: Option<String>,
}

impl Default for StockRule {
    fn default() -> Self {
        Self {
            phrases: DEFAULT_PHRASES.iter().map(|p| (*p).to_string()).collect(),
            min_seconds: DEFAULT_MIN_SECONDS,
            set_by: None,
        }
    }
}

impl StockRule {
    pub fn enabled(&self) -> bool {
        !self.phrases.is_empty()
    }

    fn same_as_default(&self) -> bool {
        let d = Self::default();
        self.phrases == d.phrases && self.min_seconds == d.min_seconds
    }

    /// The same length with no phrases: what turning it off for one run means.
    pub fn off(&self) -> Self {
        Self {
            phrases: Vec::new(),
            min_seconds: self.min_seconds,
            set_by: Some(TURNED_OFF.into()),
        }
    }

    /// "Set by $GM_STOCK_PHRASES." or "Turned off for this run.", when it is not the default.
    pub fn origin_note(&self) -> Option<String> {
        self.set_by.as_ref().map(|by| {
            if by == TURNED_OFF {
                "Turned off for this run.".to_string()
            } else {
                format!("Set by {by}.")
            }
        })
    }

    /// The flags that make `transcribe.sh` behave this way. None when the script's
    /// own defaults already do, so an older script without the flags still runs.
    pub fn script_args(&self) -> Vec<String> {
        let d = Self::default();
        let mut args = Vec::new();
        if self.phrases != d.phrases {
            args.extend(["--stock-phrases".to_string(), self.phrases.join(",")]);
        }
        if self.min_seconds != d.min_seconds {
            args.extend(["--stock-min-seconds".to_string(), self.min_seconds.to_string()]);
        }
        args
    }

    /// `"thank you" and "we'll be right back"`, or `"a", "b" and "c"`.
    pub fn phrase_list(&self) -> String {
        let q: Vec<String> = self.phrases.iter().map(|p| format!("\"{p}\"")).collect();
        match q.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        }
    }

    /// One line for the confirm step.
    pub fn describe(&self) -> String {
        if self.enabled() {
            format!(
                "{} lines that last {} s or more are dropped",
                self.phrase_list(),
                self.min_seconds
            )
        } else {
            "off, nothing is dropped for being a stock phrase".into()
        }
    }
}

/// Phrases from text: comma-separated, or `off` / `none`.
fn phrases_from_text(text: &str, src: &str) -> Result<Vec<String>, CampaignError> {
    let t = text.trim();
    if t.eq_ignore_ascii_case("off") || t.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    let out: Vec<String> = t
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    if out.is_empty() {
        return Err(CampaignError::new(format!(
            "stock phrases from {src} is {}; write the phrases separated by commas, or off to turn the rule off.",
            pyish::repr(&Value::String(text.to_string()))
        )));
    }
    check_phrases(&out, src)?;
    Ok(out)
}

fn check_phrases(phrases: &[String], src: &str) -> Result<(), CampaignError> {
    for p in phrases {
        if p.contains(',') || p.chars().any(char::is_control) {
            return Err(CampaignError::new(format!(
                "stock phrase {} from {src} contains a comma or a control character; \
                 the script splits the list on commas.",
                pyish::repr(&Value::String(p.clone()))
            )));
        }
    }
    Ok(())
}

fn seconds_from(v: &Value, src: &str) -> Result<u32, CampaignError> {
    let bad = || {
        CampaignError::new(format!(
            "stock length from {src} is {}; it must be a whole number of seconds, 0 to {MAX_SECONDS}.",
            pyish::repr(v)
        ))
    };
    let n = match v {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Value::String(s) => pyish::parse_int(s).and_then(|n| u32::try_from(n).ok()),
        _ => None,
    };
    n.filter(|n| *n <= MAX_SECONDS).ok_or_else(bad)
}

impl Settings {
    /// The stock-phrase rule: environment, then config file, then the script's
    /// defaults. A bad value in either is an error naming where it came from.
    pub fn stock_rule(&self) -> Result<StockRule, CampaignError> {
        let mut rule = StockRule::default();
        let mut set_by: Option<String> = None;

        if let Some(raw) = self.env(PHRASES_ENV) {
            let src = format!("${PHRASES_ENV}");
            rule.phrases = phrases_from_text(raw, &src)?;
            set_by = Some(src);
        } else {
            match self.config()?.get("stock_phrases") {
                Some(Value::Array(items)) => {
                    let src = self.config_path();
                    let mut out = Vec::new();
                    for item in items {
                        match item {
                            Value::String(s) if !s.trim().is_empty() => out.push(s.trim().to_string()),
                            other => {
                                return Err(CampaignError::new(format!(
                                    "stock_phrases in {src} must be a list of non-empty strings, got {}",
                                    pyish::repr(other)
                                )));
                            }
                        }
                    }
                    check_phrases(&out, &src)?;
                    rule.phrases = out;
                    set_by = Some(src);
                }
                Some(Value::String(s)) if !s.trim().is_empty() => {
                    let src = self.config_path();
                    rule.phrases = phrases_from_text(s, &src)?;
                    set_by = Some(src);
                }
                Some(Value::Null | Value::String(_)) | None => {}
                Some(other) => {
                    return Err(CampaignError::new(format!(
                        "stock_phrases in {} must be a list of strings or text, got {}",
                        self.config_path(),
                        pyish::repr(other)
                    )));
                }
            }
        }

        if let Some(raw) = self.env(SECONDS_ENV) {
            let src = format!("${SECONDS_ENV}");
            rule.min_seconds = seconds_from(&Value::String(raw.to_string()), &src)?;
            set_by.get_or_insert(src);
        } else {
            match self.config()?.get("stock_min_seconds") {
                Some(Value::Null) | None => {}
                Some(Value::String(s)) if s.trim().is_empty() => {}
                Some(v) => {
                    let src = self.config_path();
                    rule.min_seconds = seconds_from(v, &src)?;
                    set_by.get_or_insert(src);
                }
            }
        }

        // Saying the default out loud is not a customisation worth reporting.
        rule.set_by = if rule.same_as_default() { None } else { set_by };
        Ok(rule)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn settings(env: &[(&str, &str)], config: Option<&str>) -> (Settings, TempDir) {
        let tmp = TempDir::new().unwrap();
        let mut map: HashMap<String, String> = env.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
        map.insert("HOME".into(), tmp.path().to_string_lossy().into_owned());
        let cfg = tmp.path().join("config.json");
        if let Some(body) = config {
            fs::write(&cfg, body).unwrap();
        }
        map.insert("GM_TRANSCRIBER_CONFIG".into(), cfg.to_string_lossy().into_owned());
        (Settings::from_env_map(map, "t"), tmp)
    }

    #[test]
    fn with_nothing_set_it_is_the_scripts_default_and_passes_no_flags() {
        let (s, _t) = settings(&[], None);
        let r = s.stock_rule().unwrap();
        assert_eq!(r, StockRule::default());
        assert!(r.enabled() && r.script_args().is_empty() && r.set_by.is_none());
        assert_eq!(
            r.describe(),
            "\"thank you\" and \"we'll be right back\" lines that last 20 s or more are dropped"
        );
    }

    #[test]
    fn the_defaults_equal_the_ones_in_the_script() {
        let script =
            fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../legacy/transcribe.sh"))
                .unwrap();
        let line = |prefix: &str| {
            script
                .lines()
                .find(|l| l.starts_with(prefix))
                .unwrap_or_else(|| panic!("{prefix} not in transcribe.sh"))
                .to_string()
        };
        let phrases: Vec<String> = line("DEFAULT_STOCK_PHRASES=(")
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();
        assert_eq!(phrases, DEFAULT_PHRASES);
        assert_eq!(
            line("DEFAULT_STOCK_MIN_SECONDS="),
            format!("DEFAULT_STOCK_MIN_SECONDS={DEFAULT_MIN_SECONDS}")
        );
    }

    #[test]
    fn the_environment_wins_over_the_config_file() {
        let (s, _t) = settings(
            &[(PHRASES_ENV, " Thanks for watching , bye "), (SECONDS_ENV, "30")],
            Some(r#"{"stock_phrases": ["x"], "stock_min_seconds": 99}"#),
        );
        let r = s.stock_rule().unwrap();
        assert_eq!(r.phrases, ["Thanks for watching", "bye"]);
        assert_eq!(r.min_seconds, 30);
        assert_eq!(r.set_by.as_deref(), Some("$GM_STOCK_PHRASES"));
        assert_eq!(
            r.script_args(),
            [
                "--stock-phrases",
                "Thanks for watching,bye",
                "--stock-min-seconds",
                "30"
            ]
        );
        assert_eq!(r.phrase_list(), "\"Thanks for watching\" and \"bye\"");
    }

    #[test]
    fn the_config_file_can_set_a_list_text_or_off() {
        let (s, t) = settings(&[], Some(r#"{"stock_phrases": ["thank you", "see you next week"]}"#));
        let r = s.stock_rule().unwrap();
        assert_eq!(r.phrases, ["thank you", "see you next week"]);
        assert_eq!(r.set_by.as_deref(), Some(s.config_path().as_str()));
        assert_eq!(r.phrase_list(), "\"thank you\" and \"see you next week\"");
        drop(t);

        let (s, _t) = settings(&[], Some(r#"{"stock_phrases": "a, b, c", "stock_min_seconds": "45"}"#));
        let r = s.stock_rule().unwrap();
        assert_eq!(r.phrases, ["a", "b", "c"]);
        assert_eq!(r.min_seconds, 45);
        assert_eq!(r.phrase_list(), "\"a\", \"b\" and \"c\"");

        for off in [
            r#"{"stock_phrases": []}"#,
            r#"{"stock_phrases": "off"}"#,
            r#"{"stock_phrases": "None"}"#,
        ] {
            let (s, _t) = settings(&[], Some(off));
            let r = s.stock_rule().unwrap();
            assert!(!r.enabled(), "{off}");
            assert_eq!(r.script_args(), ["--stock-phrases", ""], "{off}");
            assert!(r.describe().starts_with("off"));
        }
        let (s, _t) = settings(&[(PHRASES_ENV, "off")], None);
        assert!(!s.stock_rule().unwrap().enabled());
    }

    #[test]
    fn empty_or_null_means_not_set_and_the_default_spelled_out_is_no_customisation() {
        let (s, _t) = settings(&[], Some(r#"{"stock_phrases": null, "stock_min_seconds": ""}"#));
        assert_eq!(s.stock_rule().unwrap(), StockRule::default());
        let (s, _t) = settings(
            &[],
            Some(r#"{"stock_phrases": ["thank you", "we'll be right back"], "stock_min_seconds": 20}"#),
        );
        let r = s.stock_rule().unwrap();
        assert!(r.set_by.is_none() && r.script_args().is_empty());
    }

    #[test]
    fn bad_values_are_errors_that_name_where_they_came_from() {
        for (env, cfg, want) in [
            (&[(SECONDS_ENV, "soon")][..], None, "$GM_STOCK_MIN_SECONDS"),
            (&[(SECONDS_ENV, "-5")][..], None, "whole number of seconds"),
            (&[(SECONDS_ENV, "86401")][..], None, "0 to 86400"),
            (&[(PHRASES_ENV, " , ,")][..], None, "$GM_STOCK_PHRASES"),
            (&[][..], Some(r#"{"stock_phrases": [1]}"#), "list of non-empty strings"),
            (&[][..], Some(r#"{"stock_phrases": [""]}"#), "list of non-empty strings"),
            (
                &[][..],
                Some(r#"{"stock_phrases": {"a": 1}}"#),
                "list of strings or text",
            ),
            (&[][..], Some(r#"{"stock_phrases": ["a,b"]}"#), "contains a comma"),
            (
                &[][..],
                Some(r#"{"stock_min_seconds": 2.5}"#),
                "whole number of seconds",
            ),
            (
                &[][..],
                Some(r#"{"stock_min_seconds": [20]}"#),
                "whole number of seconds",
            ),
        ] {
            let (s, _t) = settings(env, cfg);
            let err = s.stock_rule().expect_err(want).0;
            assert!(err.contains(want), "{err} should mention {want}");
        }
        let (s, _t) = settings(&[], Some("{not json"));
        assert!(s.stock_rule().unwrap_err().0.contains("is not valid JSON"));
    }

    #[test]
    fn off_for_one_run_keeps_the_length_and_says_so() {
        let r = StockRule {
            min_seconds: 45,
            ..StockRule::default()
        }
        .off();
        assert!(!r.enabled() && r.min_seconds == 45);
        assert_eq!(r.set_by.as_deref(), Some("turned off for this run"));
        assert_eq!(r.origin_note().as_deref(), Some("Turned off for this run."));
        assert_eq!(StockRule::default().origin_note(), None);
        assert_eq!(r.script_args(), ["--stock-phrases", "", "--stock-min-seconds", "45"]);
    }
}
