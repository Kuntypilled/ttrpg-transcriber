//! Text output of the `list`, `paths`, `doctor` and `whisper-device` commands.
//!
//! The formats match `campaign.py` so the Rust and Python outputs can be diffed
//! (PORTING.md section 9, items 5 and 6). Output is returned as a string; the
//! binary prints it.

use std::fmt::Write as _;
use std::path::Path;

use crate::campaign::{Campaign, list_campaigns};
use crate::error::CampaignError;
use crate::pyish;
use crate::settings::{Settings, Sourced};

/// `slug<TAB>label<TAB>system` per campaign.
pub fn list_output(settings: &Settings) -> String {
    let mut out = String::new();
    for (slug, c) in list_campaigns(settings) {
        let _ = writeln!(out, "{slug}\t{}\t{}", c.label(), c.system());
    }
    out
}

fn shquote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Shell-eval-able `KEY='value'` path resolution.
pub fn paths_output(settings: &Settings, slug: &str, session: Option<i64>) -> Result<String, CampaignError> {
    let c = Campaign::load(settings, slug)?;
    let p = c.paths(settings)?;
    let mut rows: Vec<(&str, String)> = vec![
        ("CAMPAIGN", p.slug),
        ("CAMPAIGN_LABEL", p.label),
        ("CAMPAIGN_SYSTEM", p.system),
        ("CAMPAIGN_WORLD", p.world),
        ("CAMPAIGN_ROOT", p.root),
        ("ALIASES_FILE", p.aliases_file),
        ("TRANSCRIPTS_DIR", p.transcripts_dir),
        ("CHUNKS_DIR", p.chunks_dir),
        ("AUDIO_BASE", p.audio_base),
        ("DB_PATH", p.db_path),
    ];
    if let Some(n) = session {
        let s = c.session_paths(settings, n)?;
        rows.extend([
            ("SESSION", s.session.to_string()),
            ("SESSION_DIR", s.session_dir),
            ("TRANSCRIPT_MD", s.transcript_md),
            ("CHUNKS_FILE", s.chunks_file),
            ("AUDIO_DIR", s.audio_dir),
            ("WORK_DIR", s.work_dir),
        ]);
    }
    let mut out = String::new();
    for (k, v) in rows {
        let _ = writeln!(out, "{k}={}", shquote(&v));
    }
    Ok(out)
}

/// The configured whisper-cli device index, or nothing.
pub fn whisper_device_output(settings: &Settings) -> Result<String, CampaignError> {
    Ok(settings
        .whisper_device()?
        .map(|(dev, _)| format!("{dev}\n"))
        .unwrap_or_default())
}

/// What `doctor` printed, and how it ended. Output produced before an error is
/// kept, as the original printed as it went.
pub struct DoctorReport {
    pub out: String,
    pub result: Result<i32, CampaignError>,
}

/// Show what every root resolved to, and from where.
pub fn doctor(settings: &Settings) -> DoctorReport {
    let mut out = String::new();
    let result = doctor_into(settings, &mut out);
    DoctorReport { out, result }
}

fn row(out: &mut String, label: &str, path: &str, src: &str) {
    let mark = if Path::new(path).is_dir() { "ok     " } else { "missing" };
    let _ = writeln!(out, "{label:<14} {path}");
    let _ = writeln!(out, "{:<14} {mark}  from {src}", "");
}

fn sourced_row(out: &mut String, label: &str, s: &Sourced) {
    row(out, label, &s.value, &s.source);
}

fn doctor_into(s: &Settings, out: &mut String) -> Result<i32, CampaignError> {
    let cfg = s.config_path();
    let present = if Path::new(&cfg).is_file() {
        ""
    } else {
        "   (not present)"
    };
    let _ = writeln!(out, "config file    {cfg}{present}");
    out.push('\n');

    if s.multi_vault()? {
        let root = s.vaults_root()?.expect("multi_vault");
        sourced_row(out, "vaults root", &root);
        let sub = s.campaigns_subdir()?;
        let mut worlds: Vec<String> = std::fs::read_dir(&root.value)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.'))
            .map(|n| pyish::join(&root.value, &n))
            .filter(|d| Path::new(&pyish::join(d, &sub)).is_dir())
            .collect();
        worlds.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        let names: Vec<&str> = worlds.iter().map(|w| w.rsplit('/').next().unwrap_or(w)).collect();
        let names = if names.is_empty() {
            "(none)".to_string()
        } else {
            names.join(", ")
        };
        let _ = writeln!(out, "{:<14} {names}", "worlds");
        let single_set = s.config()?.get("vault").is_some_and(pyish::truthy)
            || s.env("GM_VAULT").is_some()
            || s.env("VAULT").is_some();
        if single_set {
            let _ = writeln!(out, "{:<14} note: a single vault is also set; the vaults root wins", "");
        }
    } else {
        match s.vault_root() {
            Err(e) => {
                let _ = writeln!(out, "{:<14} NOT CONFIGURED", "vault");
                out.push('\n');
                let _ = writeln!(out, "{e}");
                return Ok(2);
            }
            Ok(vault) => {
                sourced_row(out, "vault", &vault);
                let src = format!("vault + {}", pyish::repr_str(&s.campaigns_subdir()?));
                row(out, "campaigns", &s.campaigns_dir(None)?, &src);
            }
        }
    }
    sourced_row(out, "audio root", &s.audio_root()?);
    sourced_row(out, "chroma root", &s.chroma_root()?);
    sourced_row(out, "work root", &s.work_root()?);
    match s.whisper_device() {
        Err(e) => {
            let _ = writeln!(out, "{:<14} INVALID\n{:<14} {e}", "whisper dev", "");
        }
        Ok(None) => {
            let _ = writeln!(
                out,
                "{:<14} not set (whisper-cli uses its own default, device 0)",
                "whisper dev"
            );
        }
        Ok(Some((dev, src))) => {
            let _ = writeln!(out, "{:<14} {dev}", "whisper dev");
            let _ = writeln!(out, "{:<14} set    from {src}", "");
        }
    }
    out.push('\n');

    let camps = list_campaigns(s);
    if camps.is_empty() {
        let _ = writeln!(out, "{}", s.setup_hint()?);
        return Ok(1);
    }
    let _ = writeln!(out, "{} campaign(s):", camps.len());
    for (slug, c) in &camps {
        let flag = if c.has_review_only() {
            ""
        } else {
            "   (no review_only yet)"
        };
        let system = c.system();
        let system = if system.is_empty() { "system unset" } else { &system };
        let _ = writeln!(out, "  {slug:<24} {}  [{system}]{flag}", c.label());
    }
    Ok(0)
}
