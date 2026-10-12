//! Config and root resolution (PORTING.md section 3.1).
//!
//! Every root resolves in the same order: environment variable, then the config
//! file, then a built-in default where a correct one exists. Each resolved value
//! remembers where it came from, because `doctor` prints that.
//!
//! The environment is captured once into a map so tests can build a `Settings`
//! without touching the process environment. The config file is read lazily and
//! only on first use, as the original did: a broken config.json is not an error
//! until something actually needs it.

use std::cell::OnceCell;
use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::error::CampaignError;
use crate::pyish;

pub const CONFIG_ENV: &str = "GM_TRANSCRIBER_CONFIG";
pub const CONFIG_RELPATH: &str = "ttrpg-transcriber/config.json";
pub const WHISPER_DEVICE_ENV: &str = "GM_WHISPER_DEVICE";
pub const DEFAULT_CAMPAIGNS_SUBDIR: &str = "0.2_campaigns";
pub const DEFAULT_TRANSCRIPTS_SUBDIR: &str = "08_transcripts";
pub const ALIASES_FILENAME: &str = "canon_aliases.json";

/// A resolved value and where it came from (`$GM_AUDIO_ROOT`, a config path, or
/// "built-in default").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sourced {
    pub value: String,
    pub source: String,
}

pub struct Settings {
    env: HashMap<String, String>,
    cli_name: String,
    config: OnceCell<Map<String, Value>>,
}

impl Settings {
    /// Settings from the real process environment. Variables whose value is not
    /// valid UTF-8 are ignored.
    pub fn from_process_env(cli_name: &str) -> Self {
        let env = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        Self::from_env_map(env, cli_name)
    }

    /// `cli_name` is how messages refer to this program ("ttrpg-tui doctor").
    pub fn from_env_map(env: HashMap<String, String>, cli_name: &str) -> Self {
        Self {
            env,
            cli_name: cli_name.to_string(),
            config: OnceCell::new(),
        }
    }

    pub fn cli_name(&self) -> &str {
        &self.cli_name
    }

    /// An environment variable, treating empty as unset (Python's `if val:`).
    pub fn env(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(String::as_str).filter(|v| !v.is_empty())
    }

    /// An environment variable including the empty string.
    pub fn env_raw(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(String::as_str)
    }

    // -- XDG ----------------------------------------------------------------

    /// `$NAME` if it is absolute, else the fallback with `~` expanded. The value is
    /// used verbatim: it is not normalised.
    fn xdg(&self, name: &str, fallback: &str) -> String {
        match self.env(name) {
            Some(v) if pyish::is_abs(v) => v.to_string(),
            _ => pyish::expanduser(fallback, self.env_raw("HOME")),
        }
    }

    pub fn xdg_config_home(&self) -> String {
        self.xdg("XDG_CONFIG_HOME", "~/.config")
    }
    pub fn xdg_data_home(&self) -> String {
        self.xdg("XDG_DATA_HOME", "~/.local/share")
    }
    pub fn xdg_cache_home(&self) -> String {
        self.xdg("XDG_CACHE_HOME", "~/.cache")
    }
    pub fn xdg_state_home(&self) -> String {
        self.xdg("XDG_STATE_HOME", "~/.local/state")
    }

    /// `expanduser` then `abspath`, using this environment's `$HOME`.
    pub fn expand_abs(&self, value: &str) -> String {
        pyish::expand_abs(value, self.env_raw("HOME"))
    }

    // -- config file --------------------------------------------------------

    pub fn config_path(&self) -> String {
        match self.env(CONFIG_ENV) {
            Some(v) => self.expand_abs(v),
            None => pyish::join(&self.xdg_config_home(), CONFIG_RELPATH),
        }
    }

    /// Contents of config.json, or an empty map when there is no file. Unknown
    /// keys and `_comment` entries are simply ignored by every reader.
    pub fn config(&self) -> Result<&Map<String, Value>, CampaignError> {
        if let Some(cfg) = self.config.get() {
            return Ok(cfg);
        }
        let path = self.config_path();
        let data = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(map)) => map,
                Ok(_) => Map::new(),
                Err(e) => {
                    return Err(CampaignError::new(format!("{path} is not valid JSON: {e}")));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
            Err(e) => return Err(CampaignError::new(format!("cannot read {path}: {e}"))),
        };
        Ok(self.config.get_or_init(|| data))
    }

    /// A config value that must be a path or name. Scalars are stringified the way
    /// Python's `str()` would; arrays and objects are rejected with a message
    /// naming the key (the original would have produced a nonsense path).
    fn config_string(&self, key: &str, v: &Value) -> Result<String, CampaignError> {
        match v {
            Value::Array(_) | Value::Object(_) => Err(CampaignError::new(format!(
                "{key} in {} must be a string, got {}",
                self.config_path(),
                pyish::repr(v)
            ))),
            other => Ok(pyish::str_of(other)),
        }
    }

    // -- roots --------------------------------------------------------------

    /// Environment variable, else config key, else default; made absolute.
    /// `Ok(None)` when nothing supplies it.
    fn resolve(
        &self,
        key: &str,
        env_names: &[&str],
        default: Option<String>,
    ) -> Result<Option<Sourced>, CampaignError> {
        for name in env_names {
            if let Some(v) = self.env(name) {
                return Ok(Some(Sourced {
                    value: self.expand_abs(v),
                    source: format!("${name}"),
                }));
            }
        }
        if let Some(v) = self.config()?.get(key).filter(|v| pyish::truthy(v)) {
            let s = self.config_string(key, v)?;
            return Ok(Some(Sourced {
                value: self.expand_abs(&s),
                source: self.config_path(),
            }));
        }
        Ok(default.map(|d| Sourced {
            value: self.expand_abs(&d),
            source: "built-in default".into(),
        }))
    }

    /// Environment variable, else config key, else default; not made absolute
    /// (used for the campaigns and transcripts sub-folder names).
    fn setting(&self, key: &str, env_name: &str, default: &str) -> Result<String, CampaignError> {
        if let Some(v) = self.env(env_name) {
            return Ok(v.to_string());
        }
        if let Some(v) = self.config()?.get(key).filter(|v| pyish::truthy(v)) {
            return self.config_string(key, v);
        }
        Ok(default.to_string())
    }

    /// The directory holding one vault per world, if configured.
    pub fn vaults_root(&self) -> Result<Option<Sourced>, CampaignError> {
        self.resolve("vaults_root", &["GM_VAULTS_ROOT"], None)
    }

    /// Multi-vault mode: a vaults root is configured, and it wins over a single vault.
    pub fn multi_vault(&self) -> Result<bool, CampaignError> {
        Ok(self.vaults_root()?.is_some())
    }

    /// The single-vault root. Fails with the setup instructions when unconfigured.
    pub fn vault_root(&self) -> Result<Sourced, CampaignError> {
        self.resolve("vault", &["GM_VAULT", "VAULT"], None)?
            .ok_or_else(|| CampaignError::new(self.unconfigured_message()))
    }

    pub fn campaigns_subdir(&self) -> Result<String, CampaignError> {
        self.setting("campaigns_subdir", "GM_CAMPAIGNS_SUBDIR", DEFAULT_CAMPAIGNS_SUBDIR)
    }

    pub fn transcripts_subdir(&self) -> Result<String, CampaignError> {
        self.setting(
            "transcripts_subdir",
            "GM_TRANSCRIPTS_SUBDIR",
            DEFAULT_TRANSCRIPTS_SUBDIR,
        )
    }

    /// The vault directory for one world (multi-vault mode).
    pub fn world_vault(&self, world: &str) -> Result<String, CampaignError> {
        match self.vaults_root()? {
            Some(root) => Ok(pyish::join(&root.value, world)),
            None => Err(CampaignError::new(self.unconfigured_message())),
        }
    }

    /// The folder holding the campaigns. `world` is required in multi-vault mode.
    pub fn campaigns_dir(&self, world: Option<&str>) -> Result<String, CampaignError> {
        if self.multi_vault()? {
            let world = world.ok_or_else(|| CampaignError::new("campaigns_dir() needs a world in multi-vault mode"))?;
            Ok(pyish::join(&self.world_vault(world)?, &self.campaigns_subdir()?))
        } else {
            Ok(pyish::join(&self.vault_root()?.value, &self.campaigns_subdir()?))
        }
    }

    pub fn audio_root(&self) -> Result<Sourced, CampaignError> {
        self.required_root(
            "audio_root",
            "GM_AUDIO_ROOT",
            pyish::join(&self.xdg_data_home(), "ttrpg_audio"),
        )
    }

    pub fn chroma_root(&self) -> Result<Sourced, CampaignError> {
        self.required_root(
            "chroma_root",
            "GM_CHROMA_ROOT",
            pyish::join(&self.xdg_data_home(), "ttrpg_memory"),
        )
    }

    pub fn work_root(&self) -> Result<Sourced, CampaignError> {
        self.required_root(
            "work_root",
            "GM_WORK_ROOT",
            pyish::join(&self.xdg_cache_home(), "ttrpg_transcribe"),
        )
    }

    fn required_root(&self, key: &str, env: &str, default: String) -> Result<Sourced, CampaignError> {
        // A default is always supplied, so resolve() cannot return None here.
        Ok(self.resolve(key, &[env], Some(default))?.expect("default supplied"))
    }

    // -- pipeline tools (not in the original campaign.py; transcribe.sh took these as flags) --

    /// Every environment variable, as handed to child processes.
    pub fn env_vars(&self) -> &HashMap<String, String> {
        &self.env
    }

    /// A command name or path from an environment variable, else a config key. A bare
    /// name is looked up on PATH, so it is not made absolute.
    fn command_setting(&self, env_name: &str, key: &str) -> Result<Option<Sourced>, CampaignError> {
        let pick = |raw: &str, source: String| {
            let expanded = pyish::expanduser(raw, self.env_raw("HOME"));
            let value = if expanded.contains('/') {
                self.expand_abs(&expanded)
            } else {
                expanded
            };
            Sourced { value, source }
        };
        if let Some(v) = self.env(env_name) {
            return Ok(Some(pick(v, format!("${env_name}"))));
        }
        match self.config()?.get(key).filter(|v| pyish::truthy(v)) {
            Some(v) => Ok(Some(pick(&self.config_string(key, v)?, self.config_path()))),
            None => Ok(None),
        }
    }

    /// The command name or path of whisper-cli: `$GM_WHISPER_BIN`, else the
    /// `whisper_bin` config key.
    pub fn whisper_bin(&self) -> Result<Option<Sourced>, CampaignError> {
        self.command_setting("GM_WHISPER_BIN", "whisper_bin")
    }

    /// The Python for indexing: `$GM_PYTHON`, else the `python` config key. ChromaDB
    /// usually lives in a venv or a `--user` install, so this is a setting.
    pub fn python(&self) -> Result<Option<Sourced>, CampaignError> {
        self.command_setting("GM_PYTHON", "python")
    }

    /// The Python to run, defaulting to `python3` on PATH.
    pub fn effective_python(&self) -> Result<String, CampaignError> {
        Ok(self.python()?.map_or_else(|| "python3".to_string(), |s| s.value))
    }

    /// `$GM_WHISPER_MODEL` or the `whisper_model` config key: the ggml model file.
    pub fn whisper_model(&self) -> Result<Option<Sourced>, CampaignError> {
        self.resolve("whisper_model", &["GM_WHISPER_MODEL"], None)
    }

    /// `$GM_LEGACY_DIR` or the `legacy_dir` config key: where `transcribe.sh` lives.
    pub fn legacy_dir(&self) -> Result<Option<Sourced>, CampaignError> {
        self.resolve("legacy_dir", &["GM_LEGACY_DIR"], None)
    }

    /// The command to run for whisper, with the script's own default.
    pub fn effective_whisper_bin(&self) -> Result<String, CampaignError> {
        Ok(self
            .whisper_bin()?
            .map_or_else(|| "whisper-cli".to_string(), |s| s.value))
    }

    /// The model file, with `transcribe.sh`'s default (which is `$HOME`-based, not XDG).
    pub fn effective_model(&self) -> Result<String, CampaignError> {
        Ok(self.whisper_model()?.map_or_else(
            || {
                let home = self.env_raw("HOME").unwrap_or("");
                pyish::join(home, ".local/share/whisper.cpp/models/ggml-large-v3-turbo.bin")
            },
            |s| s.value,
        ))
    }

    /// whisper-cli's `-dev N`: the ggml device index. `Ok(None)` lets whisper-cli
    /// choose. The environment wins over the config file, and a bad value in
    /// either is an error naming where it came from.
    pub fn whisper_device(&self) -> Result<Option<(u32, String)>, CampaignError> {
        // The original builds both candidates up front, so an unreadable config
        // file is reported even when the environment variable would have won.
        let cfg_val = self.config()?.get("whisper_device").cloned();
        let candidates = [
            (
                format!("${WHISPER_DEVICE_ENV}"),
                self.env_raw(WHISPER_DEVICE_ENV).map(|s| Value::String(s.to_string())),
            ),
            (self.config_path(), cfg_val),
        ];
        for (src, val) in candidates {
            let Some(val) = val else { continue };
            if val.is_null() || val == Value::String(String::new()) {
                continue;
            }
            let idx = pyish::parse_int(&pyish::str_of(&val))
                .filter(|n| (0..=i128::from(u32::MAX)).contains(n))
                .ok_or_else(|| {
                    CampaignError::new(format!(
                        "whisper_device from {src} is {}; it must be a whole number, 0 or higher \
                         (the device index whisper-cli prints at startup).",
                        pyish::repr(&val)
                    ))
                })?;
            return Ok(Some((idx as u32, src)));
        }
        Ok(None)
    }

    // -- messages -----------------------------------------------------------

    /// The setup instructions shown instead of an empty campaign list. There is
    /// deliberately no vault default: a wrong guess fails silently.
    pub fn unconfigured_message(&self) -> String {
        format!(
            "No vault is configured, so there are no campaigns to find.\n\
             \n\
             One vault per world (a directory containing <world>/0.2_campaigns/...):\n\
             \x20   export GM_VAULTS_ROOT=/path/to/vaults\n\
             \x20   or put {{\"vaults_root\": \"/path/to/vaults\"}} in the config file\n\
             \n\
             One vault for everything:\n\
             \x20   export GM_VAULT=/path/to/your/vault\n\
             \x20   or put {{\"vault\": \"/path/to/your/vault\"}} in the config file\n\
             \n\
             Then run:  {} doctor",
            self.cli_name
        )
    }

    /// One line for the UI when no campaign loaded: what is wrong, or where it looked.
    pub fn setup_hint(&self) -> Result<String, CampaignError> {
        if self.multi_vault()? {
            let root = self.vaults_root()?.expect("multi_vault").value;
            return Ok(format!(
                "No campaigns found under {root}/<world>/{}/",
                self.campaigns_subdir()?
            ));
        }
        match self.campaigns_dir(None) {
            Ok(dir) => Ok(format!("No campaigns found under {dir}")),
            Err(_) => Ok(format!(
                "No vault configured. Set GM_VAULTS_ROOT (or GM_VAULT), then run: {} doctor",
                self.cli_name
            )),
        }
    }

    /// True when neither a vaults root nor a single vault is configured.
    pub fn vault_unconfigured(&self) -> bool {
        matches!(self.multi_vault(), Ok(false)) && self.vault_root().is_err()
    }
}
