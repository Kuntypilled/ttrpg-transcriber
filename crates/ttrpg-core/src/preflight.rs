//! Are the tools the pipeline shells out to present? (PORTING.md section 4.3.)
//!
//! This replaces `transcribe.sh --check` for the TUI. That flag creates the output
//! folders and CSV files as a side effect, so it cannot be run at startup.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::legacy;
use crate::settings::Settings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepStatus {
    /// Found; the text says where.
    Found(String),
    /// Missing; the lines say what to do.
    Missing(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepItem {
    pub name: String,
    /// A missing required item stops transcription. A missing optional one only
    /// disables a convenience.
    pub required: bool,
    pub status: DepStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreflightReport {
    pub items: Vec<DepItem>,
}

impl PreflightReport {
    /// Every required item is present.
    pub fn ok(&self) -> bool {
        self.missing_required().next().is_none()
    }

    pub fn missing_required(&self) -> impl Iterator<Item = &DepItem> {
        self.items
            .iter()
            .filter(|i| i.required && matches!(i.status, DepStatus::Missing(_)))
    }

    /// "whisper-cli, jq" for a banner.
    pub fn missing_names(&self) -> String {
        self.missing_required()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `command -v`: a path is checked directly, a bare name is searched on `$PATH`.
pub fn which(settings: &Settings, name: &str) -> Option<String> {
    let executable = |p: &Path| p.is_file() && p.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
    if name.contains('/') {
        return executable(Path::new(name)).then(|| name.to_string());
    }
    settings
        .env_raw("PATH")?
        .split(':')
        .map(|dir| {
            if dir.is_empty() {
                Path::new(".").join(name)
            } else {
                Path::new(dir).join(name)
            }
        })
        .find(|p| executable(p))
        .map(|p| p.to_string_lossy().into_owned())
}

/// The install command for whichever package manager this machine has.
fn install_hint(settings: &Settings, generic: &str, arch: &str, deb: &str, rpm: &str) -> String {
    let has = |m: &str| which(settings, m).is_some();
    if has("pacman") {
        format!("sudo pacman -S --needed {arch}")
    } else if has("apt") {
        format!("sudo apt install {deb}")
    } else if has("dnf") {
        format!("sudo dnf install {rpm}")
    } else if has("zypper") {
        format!("sudo zypper install {rpm}")
    } else {
        format!("install '{generic}' with your package manager")
    }
}

fn tool(settings: &Settings, name: &str, command: &str, hint: String) -> DepItem {
    let status = match which(settings, command) {
        Some(path) => DepStatus::Found(path),
        None => DepStatus::Missing(vec![hint]),
    };
    DepItem {
        name: name.to_string(),
        required: true,
        status,
    }
}

pub fn preflight(settings: &Settings) -> PreflightReport {
    let mut items = vec![
        tool(
            settings,
            "bash",
            "bash",
            install_hint(settings, "bash", "bash", "bash", "bash"),
        ),
        tool(
            settings,
            "python3",
            "python3",
            install_hint(settings, "python3", "python", "python3", "python3"),
        ),
        tool(
            settings,
            "ffmpeg",
            "ffmpeg",
            install_hint(settings, "ffmpeg", "ffmpeg", "ffmpeg", "ffmpeg"),
        ),
        tool(settings, "jq", "jq", install_hint(settings, "jq", "jq", "jq", "jq")),
    ];

    match settings.effective_whisper_bin() {
        Ok(bin) => {
            let hint = vec![
                "Install whisper.cpp so that whisper-cli is on PATH, or set \"whisper_bin\" in the config file."
                    .to_string(),
                "Arch: sudo pacman -S ggml-vulkan whisper-cpp (swap ggml-vulkan for ggml-cuda, ggml-hip or ggml-sycl;"
                    .to_string(),
                "with no GPU install whisper-cpp alone). Elsewhere build it: https://github.com/ggml-org/whisper.cpp"
                    .to_string(),
            ];
            let status = match which(settings, &bin) {
                Some(path) => DepStatus::Found(path),
                None => DepStatus::Missing(hint),
            };
            items.push(DepItem {
                name: bin,
                required: true,
                status,
            });
        }
        Err(e) => items.push(DepItem {
            name: "whisper-cli".into(),
            required: true,
            status: DepStatus::Missing(vec![e.to_string()]),
        }),
    }

    match settings.effective_model() {
        Ok(model) => {
            let status = if Path::new(&model).is_file() {
                let mb = std::fs::metadata(&model).map_or(0, |m| m.len() / 1_048_576);
                DepStatus::Found(format!("{model} ({mb} MB)"))
            } else {
                DepStatus::Missing(vec![
                    "Download the model:".to_string(),
                    format!("  mkdir -p \"{}\"", model.rsplit_once('/').map_or(".", |(d, _)| d)),
                    format!("  wget -c -O \"{model}\" \\"),
                    "    https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin".to_string(),
                    "or point \"whisper_model\" in the config file at the model you have.".to_string(),
                ])
            };
            items.push(DepItem {
                name: "whisper model".into(),
                required: true,
                status,
            });
        }
        Err(e) => items.push(DepItem {
            name: "whisper model".into(),
            required: true,
            status: DepStatus::Missing(vec![e.to_string()]),
        }),
    }

    let status = match legacy::locate(settings) {
        Ok(found) => DepStatus::Found(format!("{} ({})", found.value, found.source)),
        Err(e) => DepStatus::Missing(vec![e.to_string()]),
    };
    items.push(DepItem {
        name: "legacy scripts".into(),
        required: true,
        status,
    });

    let status = match settings.effective_python() {
        Ok(py) => match which(settings, &py) {
            Some(path) => DepStatus::Found(path),
            None => DepStatus::Missing(vec![format!(
                "{py} was not found. It runs Index and the alias report; ChromaDB must be installed for it."
            )]),
        },
        Err(e) => DepStatus::Missing(vec![e.to_string()]),
    };
    items.push(DepItem {
        name: "python for Index".into(),
        required: false,
        status,
    });

    let status = match which(settings, "xdg-open") {
        Some(path) => DepStatus::Found(path),
        None => DepStatus::Missing(vec!["Needed only for the Open folder button.".to_string()]),
    };
    items.push(DepItem {
        name: "xdg-open".into(),
        required: false,
        status,
    });

    PreflightReport { items }
}

/// Plain-text rendering, in the style of `transcribe.sh --check`.
pub fn render(report: &PreflightReport) -> String {
    let mut out = String::new();
    for item in &report.items {
        match &item.status {
            DepStatus::Found(where_) => out.push_str(&format!("  [OK] {}: {where_}\n", item.name)),
            DepStatus::Missing(lines) => {
                let tag = if item.required { "MISSING" } else { "optional" };
                out.push_str(&format!("  [{tag}] {}\n", item.name));
                for l in lines {
                    out.push_str(&format!("            {l}\n"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    fn settings_with_path(dir: &Path, extra: &[(&str, &str)]) -> Settings {
        let mut env: HashMap<String, String> = HashMap::from([
            ("HOME".to_string(), dir.to_string_lossy().into_owned()),
            ("PATH".to_string(), dir.join("bin").to_string_lossy().into_owned()),
        ]);
        env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Settings::from_env_map(env, "ttrpg-tui")
    }

    fn install(dir: &Path, name: &str) {
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let p = bin.join(name);
        fs::write(&p, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn which_finds_executables_only() {
        let tmp = TempDir::new().unwrap();
        install(tmp.path(), "tool");
        fs::write(tmp.path().join("bin/plain"), "x").unwrap();
        let s = settings_with_path(tmp.path(), &[]);
        assert_eq!(which(&s, "tool"), Some(format!("{}/bin/tool", tmp.path().display())));
        assert_eq!(which(&s, "plain"), None, "not executable");
        assert_eq!(which(&s, "absent"), None);
        assert!(which(&s, &format!("{}/bin/tool", tmp.path().display())).is_some());
    }

    #[test]
    fn an_empty_machine_is_missing_everything_required() {
        let tmp = TempDir::new().unwrap();
        let s = settings_with_path(tmp.path(), &[]);
        let r = preflight(&s);
        assert!(!r.ok());
        let missing = r.missing_names();
        for name in ["bash", "python3", "ffmpeg", "jq", "whisper-cli", "whisper model"] {
            assert!(missing.contains(name), "{missing}");
        }
        assert!(!missing.contains("xdg-open"), "optional items never block");
        assert!(!missing.contains("legacy scripts"), "found via the checkout: {missing}");
    }

    #[test]
    fn a_complete_machine_passes() {
        let tmp = TempDir::new().unwrap();
        for t in ["bash", "python3", "ffmpeg", "jq", "my-whisper"] {
            install(tmp.path(), t);
        }
        let model = tmp.path().join("m.bin");
        fs::write(&model, "x").unwrap();
        let s = settings_with_path(
            tmp.path(),
            &[
                ("GM_WHISPER_BIN", "my-whisper"),
                ("GM_WHISPER_MODEL", model.to_str().unwrap()),
            ],
        );
        let r = preflight(&s);
        assert!(r.ok(), "{}", render(&r));
        assert!(render(&r).contains("[OK] my-whisper"));
        assert!(render(&r).contains("[optional] xdg-open"));
    }

    #[test]
    fn hints_follow_the_package_manager() {
        let tmp = TempDir::new().unwrap();
        install(tmp.path(), "pacman");
        let r = preflight(&settings_with_path(tmp.path(), &[]));
        let text = render(&r);
        assert!(text.contains("sudo pacman -S --needed ffmpeg"), "{text}");
        assert!(text.contains("sudo pacman -S --needed python"), "{text}");
    }
}
