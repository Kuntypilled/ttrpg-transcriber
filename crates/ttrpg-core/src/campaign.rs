//! Campaign registry, schema accessors and path rules (PORTING.md sections 3.2 to 3.4).
//!
//! A campaign is any folder holding a `canon_aliases.json`. That file is the whole
//! registry: there is no second config to keep in sync.

use std::collections::HashSet;
use std::path::Path;

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::error::CampaignError;
use crate::pyish;
use crate::settings::{ALIASES_FILENAME, Settings};

// -- Naming -------------------------------------------------------------------
// Session folders are S<nn>, chunk files s<nn>_chunks.jsonl, zero-padded below 10.
// This matches transcript folders filed by hand before the tool existed.

pub fn session_folder(session: i64) -> String {
    format!("S{session:02}")
}

pub fn chunks_filename(session: i64) -> String {
    format!("s{session:02}_chunks.jsonl")
}

pub use crate::pyish::normalize_character;

/// One row of the ordered speaker map. Matching is a case-insensitive substring
/// test against the transcript's speaker label and the first match wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeakerRow {
    /// Already lower-cased.
    pub key: String,
    pub player: Option<String>,
    /// Underscore-normalised; `None` means the GM.
    pub character: Option<String>,
}

/// A `discord_names` entry. Keys are the audio filename stem after the leading
/// `N-` track number and the extension are removed (`1-someuser.flac` -> `someuser`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscordEntry {
    pub player: Option<String>,
    pub character: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignPaths {
    pub slug: String,
    pub label: String,
    pub system: String,
    pub world: String,
    pub root: String,
    pub aliases_file: String,
    pub transcripts_dir: String,
    pub chunks_dir: String,
    pub audio_base: String,
    pub db_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPaths {
    pub session: i64,
    pub session_dir: String,
    pub transcript_md: String,
    pub chunks_file: String,
    pub audio_dir: String,
    pub work_dir: String,
}

/// One campaign folder, backed by its `canon_aliases.json`.
#[derive(Debug, Clone)]
pub struct Campaign {
    pub slug: String,
    pub root: String,
    data: Map<String, Value>,
}

/// `(world, campaign)` for a campaign id. The world is `None` in single-vault mode.
///
/// In multi-vault mode the id must be exactly `<world>/<campaign>`. Anything else,
/// including `..` segments, is rejected rather than resolved.
pub fn split_slug(settings: &Settings, slug: &str) -> Result<(Option<String>, String), CampaignError> {
    let parts: Vec<&str> = slug.split('/').collect();
    let (world, camp) = if settings.multi_vault()? {
        if parts.len() != 2 {
            return Err(CampaignError::new(format!(
                "'{slug}' is not a campaign id. Multi-vault mode expects <world>/<campaign>, \
                 e.g. emberfall/main.\nRun: {} list",
                settings.cli_name()
            )));
        }
        (Some(parts[0]), parts[1])
    } else {
        if parts.len() != 1 {
            return Err(CampaignError::new(format!(
                "'{slug}' is not a campaign id. Single-vault mode expects a bare campaign \
                 folder name. To use <world>/<campaign> ids, set GM_VAULTS_ROOT."
            )));
        }
        (None, parts[0])
    };
    for part in [world, Some(camp)].into_iter().flatten() {
        if matches!(part, "" | "." | "..") {
            return Err(CampaignError::new(format!("'{slug}' is not a valid campaign id.")));
        }
    }
    Ok((world.map(str::to_string), camp.to_string()))
}

fn opt_string(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_string)
}

/// `normalize_character(row.get("character"))`: null stays null.
fn character_of(v: Option<&Value>) -> Option<String> {
    match v {
        None | Some(Value::Null) => None,
        Some(other) => Some(normalize_character(&pyish::str_of(other))),
    }
}

impl Campaign {
    pub fn load(settings: &Settings, slug: &str) -> Result<Campaign, CampaignError> {
        let (world, name) = split_slug(settings, slug)?;
        let root = pyish::join(&settings.campaigns_dir(world.as_deref())?, &name);
        let path = pyish::join(&root, ALIASES_FILENAME);
        if !Path::new(&path).is_file() {
            let known: Vec<String> = list_campaigns(settings).into_iter().map(|(s, _)| s).collect();
            let known = if known.is_empty() {
                "(none)".to_string()
            } else {
                known.join(", ")
            };
            return Err(CampaignError::new(format!(
                "No campaign '{slug}': {path} does not exist.\nKnown campaigns: {known}"
            )));
        }
        let text =
            std::fs::read_to_string(&path).map_err(|e| CampaignError::new(format!("cannot read {path}: {e}")))?;
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(data)) => Ok(Campaign {
                slug: slug.to_string(),
                root,
                data,
            }),
            Ok(_) => Err(CampaignError::new(format!("{path} must contain a JSON object"))),
            Err(e) => Err(CampaignError::new(format!("{path} is not valid JSON: {e}"))),
        }
    }

    /// The parsed `canon_aliases.json`, key order preserved.
    pub fn data(&self) -> &Map<String, Value> {
        &self.data
    }

    // -- identity --

    pub fn aliases_path(&self) -> String {
        pyish::join(&self.root, ALIASES_FILENAME)
    }

    /// `display_name`, else `campaign`, else the folder name.
    pub fn label(&self) -> String {
        ["display_name", "campaign"]
            .iter()
            .filter_map(|k| self.data.get(*k))
            .find(|v| pyish::truthy(v))
            .map_or_else(|| self.slug.clone(), pyish::str_of)
    }

    pub fn system(&self) -> String {
        self.data
            .get("system")
            .filter(|v| pyish::truthy(v))
            .map(pyish::str_of)
            .unwrap_or_default()
    }

    /// The vault folder in multi-vault mode, else the JSON `world` field.
    pub fn world(&self) -> String {
        if let Some((world, _)) = self.slug.split_once('/') {
            return world.to_string();
        }
        self.data
            .get("world")
            .filter(|v| pyish::truthy(v))
            .map(pyish::str_of)
            .unwrap_or_default()
    }

    // -- paths --

    pub fn transcripts_dir(&self, settings: &Settings) -> Result<String, CampaignError> {
        Ok(pyish::join(&self.root, &settings.transcripts_subdir()?))
    }

    pub fn chunks_dir(&self, settings: &Settings) -> Result<String, CampaignError> {
        Ok(pyish::join(&self.transcripts_dir(settings)?, "chunks"))
    }

    pub fn session_dir(&self, settings: &Settings, session: i64) -> Result<String, CampaignError> {
        Ok(pyish::join(&self.transcripts_dir(settings)?, &session_folder(session)))
    }

    pub fn chunks_path(&self, settings: &Settings, session: i64) -> Result<String, CampaignError> {
        Ok(pyish::join(&self.chunks_dir(settings)?, &chunks_filename(session)))
    }

    pub fn transcript_path(&self, settings: &Settings, session: i64) -> Result<String, CampaignError> {
        Ok(pyish::join(&self.session_dir(settings, session)?, "transcript.md"))
    }

    /// The campaign's own `audio_root` if it sets one, else `<audio_root>/<id>`.
    pub fn audio_dir_base(&self, settings: &Settings) -> Result<String, CampaignError> {
        if let Some(v) = self.data.get("audio_root").filter(|v| pyish::truthy(v)) {
            if matches!(v, Value::Array(_) | Value::Object(_)) {
                return Err(CampaignError::new(format!(
                    "audio_root in {} must be a string, got {}",
                    self.aliases_path(),
                    pyish::repr(v)
                )));
            }
            return Ok(settings.expand_abs(&pyish::str_of(v)));
        }
        Ok(pyish::join(&settings.audio_root()?.value, &self.slug))
    }

    /// `Session <n>`: not zero-padded, with a space.
    pub fn audio_dir(&self, settings: &Settings, session: i64) -> Result<String, CampaignError> {
        Ok(pyish::join(
            &self.audio_dir_base(settings)?,
            &format!("Session {session}"),
        ))
    }

    pub fn db_path(&self, settings: &Settings) -> Result<String, CampaignError> {
        Ok(pyish::join(&settings.chroma_root()?.value, &self.slug))
    }

    pub fn work_dir(&self, settings: &Settings, session: i64) -> Result<String, CampaignError> {
        Ok(pyish::join_all(
            &settings.work_root()?.value,
            &[&self.slug, &session_folder(session)],
        ))
    }

    pub fn paths(&self, settings: &Settings) -> Result<CampaignPaths, CampaignError> {
        Ok(CampaignPaths {
            slug: self.slug.clone(),
            label: self.label(),
            system: self.system(),
            world: self.world(),
            root: self.root.clone(),
            aliases_file: self.aliases_path(),
            transcripts_dir: self.transcripts_dir(settings)?,
            chunks_dir: self.chunks_dir(settings)?,
            audio_base: self.audio_dir_base(settings)?,
            db_path: self.db_path(settings)?,
        })
    }

    pub fn session_paths(&self, settings: &Settings, session: i64) -> Result<SessionPaths, CampaignError> {
        Ok(SessionPaths {
            session,
            session_dir: self.session_dir(settings, session)?,
            transcript_md: self.transcript_path(settings, session)?,
            chunks_file: self.chunks_path(settings, session)?,
            audio_dir: self.audio_dir(settings, session)?,
            work_dir: self.work_dir(settings, session)?,
        })
    }

    // -- sessions --

    /// Session numbers that already have a transcripts folder (`S<digits>`) or an
    /// audio folder (`Session <digits>`), ascending and without duplicates.
    pub fn session_numbers(&self, settings: &Settings) -> Result<Vec<i64>, CampaignError> {
        let mut found = std::collections::BTreeSet::new();
        let mut scan = |dir: &str, prefix: &str| {
            for entry in std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some(digits) = name.strip_prefix(prefix)
                    && !digits.is_empty()
                    && digits.bytes().all(|b| b.is_ascii_digit())
                    && entry.path().is_dir()
                {
                    found.extend(digits.parse::<i64>().ok());
                }
            }
        };
        scan(&self.transcripts_dir(settings)?, "S");
        scan(&self.audio_dir_base(settings)?, "Session ");
        Ok(found.into_iter().collect())
    }

    /// The session most likely to be transcribed next: the one after the latest
    /// session that has a transcript, or the latest session itself when its
    /// transcript never got written (so reopening the program offers the retry).
    /// A campaign with no sessions yet starts at 1.
    pub fn suggest_session(&self, settings: &Settings) -> i64 {
        let Some(&last) = self.session_numbers(settings).unwrap_or_default().last() else {
            return 1;
        };
        let done = self
            .transcript_path(settings, last)
            .is_ok_and(|p| Path::new(&p).is_file());
        if done { last + 1 } else { last }
    }

    // -- campaign data --

    pub fn seed_prompt(&self) -> String {
        opt_string(self.data.get("seed_prompt")).unwrap_or_default()
    }

    /// Ordered `[(match, player, character)]`. Falls back to a map derived from
    /// `players` and the GM entries of `discord_names`, so a campaign whose first
    /// recording has not happened yet still resolves its own characters.
    pub fn speaker_map(&self) -> Vec<SpeakerRow> {
        if let Some(Value::Array(rows)) = self.data.get("speaker_map") {
            let explicit: Vec<SpeakerRow> = rows
                .iter()
                .filter_map(|row| {
                    let obj = row.as_object()?;
                    let key = pyish::str_of(obj.get("match")?).to_lowercase();
                    Some(SpeakerRow {
                        key,
                        player: opt_string(obj.get("player")),
                        character: character_of(obj.get("character")),
                    })
                })
                .collect();
            if !explicit.is_empty() {
                return explicit;
            }
        }
        self.derived_speaker_map()
    }

    fn derived_speaker_map(&self) -> Vec<SpeakerRow> {
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (character, player) in self.players() {
            for key in [
                character.to_lowercase(),
                player.clone().unwrap_or_default().to_lowercase(),
            ] {
                if !key.is_empty() && seen.insert(key.clone()) {
                    out.push(SpeakerRow {
                        key,
                        player: player.clone(),
                        character: Some(character.clone()),
                    });
                }
            }
        }
        if let Some(Value::Object(names)) = self.data.get("discord_names") {
            for info in names.values() {
                let Some(info) = info.as_object() else { continue };
                if info.get("role").and_then(Value::as_str) != Some("GM") {
                    continue;
                }
                let player = opt_string(info.get("player"));
                for key in ["gm".to_string(), player.clone().unwrap_or_default().to_lowercase()] {
                    if !key.is_empty() && seen.insert(key.clone()) {
                        out.push(SpeakerRow {
                            key,
                            player: player.clone(),
                            character: None,
                        });
                    }
                }
            }
        }
        if !seen.contains("gm") {
            out.push(SpeakerRow {
                key: "gm".into(),
                player: None,
                character: None,
            });
        }
        out
    }

    /// `{normalised character: player}`. Key ORDER defines the order of the
    /// `has_<PC>` fields in chunk metadata.
    pub fn players(&self) -> IndexMap<String, Option<String>> {
        let mut out: IndexMap<String, Option<String>> = IndexMap::new();
        if let Some(Value::Object(raw)) = self.data.get("players") {
            for (character, player) in raw.iter().filter(|(k, _)| !k.starts_with('_')) {
                out.insert(normalize_character(character), opt_string(Some(player)));
            }
        }
        if !out.is_empty() {
            return out;
        }
        // Fall back to the characters named in speaker_map, in first-seen order.
        if let Some(Value::Array(rows)) = self.data.get("speaker_map") {
            for row in rows.iter().filter_map(Value::as_object) {
                if let Some(character) = character_of(row.get("character")).filter(|c| !c.is_empty()) {
                    out.entry(character).or_insert_with(|| opt_string(row.get("player")));
                }
            }
        }
        out
    }

    pub fn pcs(&self) -> Vec<String> {
        self.players().into_keys().collect()
    }

    /// `discord_names` object entries, comment keys dropped.
    pub fn discord_names(&self) -> IndexMap<String, DiscordEntry> {
        let Some(Value::Object(names)) = self.data.get("discord_names") else {
            return IndexMap::new();
        };
        names
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .filter_map(|(k, v)| {
                let o = v.as_object()?;
                Some((
                    k.clone(),
                    DiscordEntry {
                        player: opt_string(o.get("player")),
                        character: opt_string(o.get("character")),
                        role: opt_string(o.get("role")),
                    },
                ))
            })
            .collect()
    }

    /// Canon proper nouns, for the alias report.
    pub fn names(&self) -> Vec<String> {
        match self.data.get("names") {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            _ => Vec::new(),
        }
    }

    fn variant_map(&self, key: &str) -> IndexMap<String, Vec<String>> {
        let Some(Value::Object(table)) = self.data.get(key) else {
            return IndexMap::new();
        };
        table
            .iter()
            .filter(|(canon, _)| !canon.starts_with('_'))
            .filter_map(|(canon, variants)| {
                let list = variants.as_array()?;
                Some((
                    canon.clone(),
                    list.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                ))
            })
            .collect()
    }

    /// SAFE substitutions, auto-applied to the transcript by the chunker. File order.
    pub fn aliases(&self) -> IndexMap<String, Vec<String>> {
        self.variant_map("aliases")
    }

    /// Manglings that collide with real English words. NEVER auto-applied.
    pub fn review_only(&self) -> IndexMap<String, Vec<String>> {
        self.variant_map("review_only")
    }

    pub fn has_review_only(&self) -> bool {
        !self.review_only().is_empty()
    }
}

// -- Discovery ------------------------------------------------------------------

/// Names of the visible sub-directories of `dir`. Symlinks to directories count;
/// dot-folders are skipped, as shell-style `*` does.
fn visible_subdirs(dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.') && Path::new(dir).join(name).is_dir())
        .collect()
}

fn discover(settings: &Settings) -> Result<Vec<(String, Campaign)>, CampaignError> {
    // (aliases path, campaign id)
    let mut candidates: Vec<(String, String)> = Vec::new();
    if settings.multi_vault()? {
        let root = settings.vaults_root()?.expect("multi_vault").value;
        let sub = settings.campaigns_subdir()?;
        for world in visible_subdirs(&root) {
            let camps = pyish::join_all(&root, &[&world, &sub]);
            for camp in visible_subdirs(&camps) {
                let aliases = pyish::join_all(&camps, &[&camp, ALIASES_FILENAME]);
                if std::fs::symlink_metadata(&aliases).is_ok() {
                    candidates.push((aliases, format!("{world}/{camp}")));
                }
            }
        }
    } else {
        let camps = settings.campaigns_dir(None)?;
        for camp in visible_subdirs(&camps) {
            let aliases = pyish::join_all(&camps, &[&camp, ALIASES_FILENAME]);
            if std::fs::symlink_metadata(&aliases).is_ok() {
                candidates.push((aliases, camp));
            }
        }
    }
    // Sorted by the full path string, as the original sorted its glob results.
    // That is not the same as sorting folder names: "main" and "main-old" order
    // by the character after "main", which is "/" in one path and "-" in the other.
    candidates.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(candidates
        .into_iter()
        // An unreadable or invalid file is skipped, not fatal.
        .filter_map(|(_, slug)| Campaign::load(settings, &slug).ok().map(|c| (slug, c)))
        .collect())
}

/// Every campaign folder holding a `canon_aliases.json`, as `(id, Campaign)`.
///
/// Multi-vault ids are `<world>/<campaign>`; single-vault ids are the folder
/// name. An unconfigured or unreadable setup yields an empty list: the caller
/// shows [`Settings::setup_hint`] instead of dying.
pub fn list_campaigns(settings: &Settings) -> Vec<(String, Campaign)> {
    discover(settings).unwrap_or_default()
}
