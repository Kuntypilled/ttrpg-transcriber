//! Are the tools the pipeline shells out to present?
//!
//! Transcribing needs ffmpeg, whisper-cli and a whisper model, and nothing else.
//! Python and the Chroma helper are for Index to Memory only, so they are optional
//! here: without them everything but Index works.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

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

/// Whether `python` can import the two packages the Chroma helper needs. Looking up a
/// module's location does not import it, so this is quick; it still gives up after 5 s.
fn python_has_chroma(settings: &Settings, python: &str) -> bool {
    python_has_chroma_within(settings, python, std::time::Duration::from_secs(5))
}

fn python_has_chroma_within(settings: &Settings, python: &str, limit: std::time::Duration) -> bool {
    const PROBE: &str = "import importlib.util as u, sys; \
        sys.exit(0 if all(u.find_spec(m) for m in ('chromadb', 'sentence_transformers')) else 1)";
    let Ok(mut child) = std::process::Command::new(python)
        .args(["-c", PROBE])
        .env_clear()
        .envs(settings.env_vars())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}

fn chroma_item(settings: &Settings) -> DepItem {
    let name = "ChromaDB for Index".to_string();
    let missing = |lines: Vec<String>| DepItem {
        name: name.clone(),
        required: false,
        status: DepStatus::Missing(lines),
    };
    let py = match settings.effective_python() {
        Ok(py) => py,
        Err(e) => return missing(vec![e.to_string()]),
    };
    let Some(path) = which(settings, &py) else {
        return missing(vec![format!(
            "{py} was not found. Index needs a Python with ChromaDB; set \"python\" in the config file."
        )]);
    };
    if python_has_chroma(settings, &path) {
        DepItem {
            name,
            required: false,
            status: DepStatus::Found(format!("{path} (chromadb and sentence-transformers found)")),
        }
    } else {
        missing(vec![
            format!("{path} cannot import chromadb and sentence-transformers. Only Index needs them. Install with:"),
            format!("  {path} -m pip install chromadb sentence-transformers"),
            "or set \"python\" in the config file to a Python that has them.".to_string(),
        ])
    }
}

pub fn preflight(settings: &Settings) -> PreflightReport {
    let mut items = vec![tool(
        settings,
        "ffmpeg",
        "ffmpeg",
        install_hint(settings, "ffmpeg", "ffmpeg", "ffmpeg", "ffmpeg"),
    )];

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

    items.push(chroma_item(settings));

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

    fn install_with(dir: &Path, name: &str, body: &str) {
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let p = bin.join(name);
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn chromadb_is_looked_for_in_the_python_that_is_set() {
        let tmp = TempDir::new().unwrap();
        install_with(tmp.path(), "py-with", "#!/bin/sh\n[ \"$1\" = -c ] && exit 0\nexit 9\n");
        install_with(tmp.path(), "py-without", "#!/bin/sh\nexit 1\n");
        let item = |py: &str| chroma_item(&settings_with_path(tmp.path(), &[("GM_PYTHON", py)]));

        let found = item("py-with");
        assert!(!found.required, "only Index needs it");
        assert!(matches!(&found.status, DepStatus::Found(w) if w.contains("chromadb and sentence-transformers found")));

        let DepStatus::Missing(lines) = item("py-without").status else {
            panic!("expected missing")
        };
        let text = lines.join("\n");
        assert!(text.contains("-m pip install chromadb sentence-transformers"), "{text}");
        assert!(text.contains("\"python\""), "{text}");

        let DepStatus::Missing(lines) = item("no-such-python").status else {
            panic!("expected missing")
        };
        assert!(lines[0].contains("no-such-python was not found"), "{lines:?}");
    }

    #[test]
    fn a_python_that_hangs_does_not_hang_the_check() {
        let tmp = TempDir::new().unwrap();
        install_with(tmp.path(), "py-hang", "#!/bin/sh\nPATH=/usr/bin:/bin exec sleep 60\n");
        let s = settings_with_path(tmp.path(), &[("GM_PYTHON", "py-hang")]);
        let py = which(&s, "py-hang").unwrap();
        let started = std::time::Instant::now();
        assert!(!python_has_chroma_within(
            &s,
            &py,
            std::time::Duration::from_millis(300)
        ));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
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
        for name in ["ffmpeg", "whisper-cli", "whisper model"] {
            assert!(missing.contains(name), "{missing}");
        }
        for optional in ["bash", "python", "jq", "xdg-open", "Index helper"] {
            assert!(
                !missing.contains(optional),
                "{optional} never blocks transcribing: {missing}"
            );
        }
    }

    #[test]
    fn a_complete_machine_passes() {
        let tmp = TempDir::new().unwrap();
        for t in ["ffmpeg", "my-whisper"] {
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
        assert!(!text.contains("jq"), "no longer needed: {text}");
    }
}
