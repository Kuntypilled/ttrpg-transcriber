//! Two more settings the original only had as command-line flags the GUI never passed
//! (PORTING.md sections 4.1, 4.4 and 7.4): the words dropped as filler, and how much
//! context whisper keeps between segments.

use serde_json::Value;

use crate::error::CampaignError;
use crate::pipeline::DEFAULT_IGNORE_WORDS;
use crate::pyish;
use crate::settings::Settings;

pub const IGNORE_WORDS_ENV: &str = "GM_IGNORE_WORDS";
pub const MAX_CONTEXT_ENV: &str = "GM_WHISPER_MC";
/// `-mc 0`: no context from earlier segments, which keeps whisper from carrying a
/// hallucination across a silence, at the price of a weaker seed prompt after the first
/// segment. The script's own comment suggests 64 if proper nouns do not improve.
pub const DEFAULT_MAX_CONTEXT: i64 = 0;

fn words_from_text(text: &str) -> Vec<String> {
    let t = text.trim();
    if t.eq_ignore_ascii_case("off") || t.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    t.split(',')
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

impl Settings {
    /// Whole lines dropped as filler (case ignored, one trailing mark ignored):
    /// `GM_IGNORE_WORDS` (comma-separated, or `off`), else the `ignore_words` config key
    /// (a list, or text), else the script's eight words. Replaces the defaults, as
    /// `--ignore-words` did.
    pub fn ignore_words(&self) -> Result<Vec<String>, CampaignError> {
        if let Some(raw) = self.env(IGNORE_WORDS_ENV) {
            return Ok(words_from_text(raw));
        }
        match self.config()?.get("ignore_words") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| match v {
                    Value::String(s) => Ok(s.trim().to_string()),
                    other => Err(CampaignError::new(format!(
                        "ignore_words in {} must be a list of strings, got {}",
                        self.config_path(),
                        pyish::repr(other)
                    ))),
                })
                .collect(),
            Some(Value::String(s)) if !s.trim().is_empty() => Ok(words_from_text(s)),
            Some(Value::Null | Value::String(_)) | None => {
                Ok(DEFAULT_IGNORE_WORDS.iter().map(|w| (*w).to_string()).collect())
            }
            Some(other) => Err(CampaignError::new(format!(
                "ignore_words in {} must be a list of strings or text, got {}",
                self.config_path(),
                pyish::repr(other)
            ))),
        }
    }

    /// whisper-cli's `-mc`: `GM_WHISPER_MC`, else the `whisper_max_context` config key,
    /// else 0. -1 means whisper's own default (all of it).
    pub fn whisper_max_context(&self) -> Result<i64, CampaignError> {
        let bad = |src: String, v: &Value| {
            CampaignError::new(format!(
                "whisper_max_context from {src} is {}; it must be a whole number (0 for none, -1 for whisper's default).",
                pyish::repr(v)
            ))
        };
        let parse = |v: &Value, src: String| -> Result<i64, CampaignError> {
            match v {
                Value::Number(n) => n.as_i64().ok_or_else(|| bad(src.clone(), v)),
                Value::String(s) => s.trim().parse::<i64>().map_err(|_| bad(src.clone(), v)),
                _ => Err(bad(src, v)),
            }
            .and_then(|n| {
                if (-1..=100_000).contains(&n) {
                    Ok(n)
                } else {
                    Err(bad("the settings".into(), v))
                }
            })
        };
        if let Some(raw) = self.env(MAX_CONTEXT_ENV) {
            return parse(&Value::String(raw.to_string()), format!("${MAX_CONTEXT_ENV}"));
        }
        match self.config()?.get("whisper_max_context") {
            Some(Value::Null) | None => Ok(DEFAULT_MAX_CONTEXT),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(DEFAULT_MAX_CONTEXT),
            Some(v) => parse(v, self.config_path()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use super::*;

    fn settings(env: &[(&str, &str)], config: Option<&str>) -> (Settings, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
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
    fn the_defaults_are_the_scripts() {
        let (s, _t) = settings(&[], None);
        assert_eq!(s.ignore_words().unwrap(), DEFAULT_IGNORE_WORDS);
        assert_eq!(s.whisper_max_context().unwrap(), 0);
    }

    #[test]
    fn ignore_words_come_from_the_environment_then_the_config_and_replace_the_defaults() {
        let (s, _t) = settings(
            &[(IGNORE_WORDS_ENV, " Hmm, uh huh ,")],
            Some(r#"{"ignore_words": ["x"]}"#),
        );
        assert_eq!(s.ignore_words().unwrap(), ["Hmm", "uh huh"]);
        let (s, _t) = settings(&[], Some(r#"{"ignore_words": ["x", " y "]}"#));
        assert_eq!(s.ignore_words().unwrap(), ["x", "y"]);
        let (s, _t) = settings(&[], Some(r#"{"ignore_words": "a,b"}"#));
        assert_eq!(s.ignore_words().unwrap(), ["a", "b"]);
        let (s, _t) = settings(&[(IGNORE_WORDS_ENV, "off")], None);
        assert!(s.ignore_words().unwrap().is_empty());
        let (s, _t) = settings(&[], Some(r#"{"ignore_words": []}"#));
        assert!(s.ignore_words().unwrap().is_empty(), "an empty list filters nothing");
        let (s, _t) = settings(&[], Some(r#"{"ignore_words": [1]}"#));
        assert!(s.ignore_words().unwrap_err().0.contains("list of strings"));
    }

    #[test]
    fn max_context_is_a_whole_number_from_the_environment_or_the_config() {
        let (s, _t) = settings(&[(MAX_CONTEXT_ENV, "64")], Some(r#"{"whisper_max_context": 8}"#));
        assert_eq!(s.whisper_max_context().unwrap(), 64);
        let (s, _t) = settings(&[], Some(r#"{"whisper_max_context": 8}"#));
        assert_eq!(s.whisper_max_context().unwrap(), 8);
        let (s, _t) = settings(&[], Some(r#"{"whisper_max_context": "-1"}"#));
        assert_eq!(s.whisper_max_context().unwrap(), -1);
        for bad in [
            r#"{"whisper_max_context": "lots"}"#,
            r#"{"whisper_max_context": 2.5}"#,
            r#"{"whisper_max_context": -5}"#,
        ] {
            let (s, _t) = settings(&[], Some(bad));
            assert!(s.whisper_max_context().unwrap_err().0.contains("whole number"), "{bad}");
        }
        let (s, _t) = settings(&[(MAX_CONTEXT_ENV, "soon")], None);
        assert!(s.whisper_max_context().unwrap_err().0.contains("$GM_WHISPER_MC"));
    }
}
