//! Startup and runtime log, so a panicked TUI leaves a trace after the alternate
//! screen closes (PORTING.md section 8). Same directory and override variable as
//! the GUI's log; a different default file name so the two never interleave.

use std::fs::OpenOptions;
use std::io::Write;

use ttrpg_core::{Settings, pyish};

pub fn log_path(settings: &Settings) -> String {
    match settings.env("GM_TRANSCRIBER_LOG") {
        Some(over) => settings.expand_abs(over),
        None => pyish::join(&settings.xdg_state_home(), "ttrpg-transcriber/ttrpg_tui.log"),
    }
}

/// Append one timestamped line. Logging must never take the program down.
pub fn append(path: &str, msg: &str) {
    let _ = try_append(path, msg);
}

fn try_append(path: &str, msg: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "[{}] {msg}", utc_timestamp())
}

/// `YYYY-MM-DDTHH:MM:SSZ`. UTC, because local time needs a timezone database.
fn utc_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_utc(secs)
}

fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn formats_known_instants() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc(1_791_849_600 + 3_661), "2026-10-13T01:01:01Z");
    }

    #[test]
    fn path_honours_the_override_and_xdg() {
        let env = |pairs: &[(&str, &str)]| {
            Settings::from_env_map(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect::<HashMap<_, _>>(),
                "t",
            )
        };
        assert_eq!(log_path(&env(&[("GM_TRANSCRIBER_LOG", "/x/y.log")])), "/x/y.log");
        assert_eq!(
            log_path(&env(&[("XDG_STATE_HOME", "/state")])),
            "/state/ttrpg-transcriber/ttrpg_tui.log"
        );
        assert_eq!(
            log_path(&env(&[("HOME", "/h")])),
            "/h/.local/state/ttrpg-transcriber/ttrpg_tui.log"
        );
    }
}
