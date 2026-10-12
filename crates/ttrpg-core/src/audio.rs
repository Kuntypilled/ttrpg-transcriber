//! Audio files and who is speaking on them.
//!
//! One speaker-resolution function serves both the pipeline and the UI. The transcription run ([`crate::transcribe`]) and the screen both call
//! it, and `tests/job.rs` checks the labels it gives against the original script's
//! output.

use std::cmp::Ordering;
use std::path::Path;

use crate::campaign::Campaign;

/// One list for the file picker and for deciding what counts as a track. The
/// original had two that disagreed (the script also found `.mp4`).
pub const AUDIO_EXTENSIONS: [&str; 8] = ["flac", "mp3", "wav", "m4a", "ogg", "aac", "mp4", "wma"];

/// Case-insensitive extension test.
pub fn is_audio_file_name(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| AUDIO_EXTENSIONS.iter().any(|a| ext.eq_ignore_ascii_case(a)))
}

fn strip_extension(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// Craig names tracks `N-discordusername.ext`. The `discord_names` key is what is
/// left after the extension and everything through the first `-` are removed:
/// `1-someuser.flac` -> `someuser`. Real Craig exports carry no `_M` suffix.
pub fn discord_stem(file_name: &str) -> String {
    let base = strip_extension(file_name);
    base.split_once('-').map_or(base, |(_, rest)| rest).to_string()
}

/// What the original `transcribe.sh` called a track that `discord_names` does not resolve: the
/// name from `N-name_M.ext` when the file name has that shape, else the whole
/// base name.
pub fn fallback_label(file_name: &str) -> String {
    fallback_label_of_base(strip_extension(file_name))
}

/// [`fallback_label`] for a name that has no extension to remove (the saved whisper files).
pub fn fallback_label_of_base(base: &str) -> String {
    let named = || -> Option<&str> {
        let (num, rest) = base.split_once('-')?;
        if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let underscore = rest.find('_')?;
        let (name, tail) = rest.split_at(underscore);
        let digits = tail.trim_start_matches('_');
        (!name.is_empty() && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(name)
    };
    named().unwrap_or(base).to_string()
}

/// Who is on a track, as far as the campaign's `discord_names` knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Speaker {
    Character {
        character: String,
        player: Option<String>,
    },
    Gm {
        player: Option<String>,
    },
    /// Not in `discord_names`, or listed without a character or a GM role.
    /// `label` is what the script will call the track.
    Unknown {
        stem: String,
        label: String,
    },
}

impl Speaker {
    pub fn is_resolved(&self) -> bool {
        !matches!(self, Speaker::Unknown { .. })
    }

    /// The speaker label written into `transcript.md`.
    pub fn label(&self) -> String {
        match self {
            Speaker::Character { character, .. } => character.clone(),
            Speaker::Gm { .. } => "GM".to_string(),
            Speaker::Unknown { label, .. } => label.clone(),
        }
    }
}

/// The `discord_names` key for a name that has no extension to remove.
pub fn discord_stem_of_base(base: &str) -> String {
    base.split_once('-').map_or(base, |(_, rest)| rest).to_string()
}

/// The character exactly as the alias file spells it. The transcript label is this
/// string and not the underscore-normalised one the chunker uses.
fn raw_character(campaign: &Campaign, stem: &str) -> Option<String> {
    let entry = campaign.data().get("discord_names")?.get(stem)?;
    match entry.get("character")? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn resolve_stem(campaign: &Campaign, stem: String, fallback: impl FnOnce() -> String) -> Speaker {
    let names = campaign.discord_names();
    match (names.get(&stem), raw_character(campaign, &stem)) {
        (Some(e), Some(character)) => Speaker::Character {
            character,
            player: e.player.clone(),
        },
        (Some(e), None) if e.role.as_deref() == Some("GM") => Speaker::Gm {
            player: e.player.clone(),
        },
        _ => Speaker::Unknown {
            stem,
            label: fallback(),
        },
    }
}

/// Resolve one audio file name against the campaign.
pub fn resolve_speaker(campaign: &Campaign, file_name: &str) -> Speaker {
    resolve_stem(campaign, discord_stem(file_name), || fallback_label(file_name))
}

/// [`resolve_speaker`] for a name without an extension, as the saved whisper files have.
/// The original removed an "extension" from these anyway, which cut a dotted Discord
/// name in two.
pub fn resolve_base(campaign: &Campaign, base: &str) -> Speaker {
    resolve_stem(campaign, discord_stem_of_base(base), || fallback_label_of_base(base))
}

/// Natural order: digit runs compare as numbers, everything else case-insensitively,
/// so Craig's `2-a.flac` sorts before `10-b.flac`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    fn digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
        let mut s = String::new();
        while let Some(c) = it.next_if(char::is_ascii_digit) {
            s.push(c);
        }
        s
    }
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let (dp, dq) = (digits(&mut x), digits(&mut y));
                let (tp, tq) = (dp.trim_start_matches('0'), dq.trim_start_matches('0'));
                match tp.len().cmp(&tq.len()).then_with(|| tp.cmp(tq)) {
                    Ordering::Equal => {}
                    other => return other,
                }
            }
            (Some(p), Some(q)) => {
                match p.to_lowercase().cmp(q.to_lowercase()) {
                    Ordering::Equal => {}
                    other => return other,
                }
                x.next();
                y.next();
            }
        }
    }
}

/// Audio files directly inside `dir`, sorted by name. A missing directory is empty.
pub fn list_audio_files(dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| Path::new(dir).join(e.file_name()).is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_audio_file_name(n))
        .collect();
    names.sort_by(|a, b| natural_cmp(a, b));
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_are_case_insensitive_and_include_mp4() {
        for ok in [
            "a.flac",
            "A.FLAC",
            "x.Mp3",
            "t.wav",
            "t.m4a",
            "t.ogg",
            "t.aac",
            "t.mp4",
            "t.wma",
            "1-a.b.flac",
        ] {
            assert!(is_audio_file_name(ok), "{ok}");
        }
        for no in ["a.txt", "flac", "a.flac.zip", "a.", "", "a.json"] {
            assert!(!is_audio_file_name(no), "{no}");
        }
    }

    #[test]
    fn stem_is_after_the_first_dash_minus_the_extension() {
        assert_eq!(discord_stem("1-someuser.flac"), "someuser");
        assert_eq!(discord_stem("12-some_user.flac"), "some_user");
        assert_eq!(discord_stem("2-a-b.flac"), "a-b", "only the first dash counts");
        assert_eq!(discord_stem("nodash.flac"), "nodash");
        assert_eq!(discord_stem("3-some.user.flac"), "some.user");
        assert_eq!(discord_stem("4-user"), "user", "no extension");
    }

    #[test]
    fn natural_order_puts_track_ten_after_track_nine() {
        let mut v = vec!["10-b.flac", "2-a.flac", "1-z.flac", "9-c.flac", "02-d.flac", "1-A.flac"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            v,
            ["1-A.flac", "1-z.flac", "2-a.flac", "02-d.flac", "9-c.flac", "10-b.flac"]
        );
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(
            natural_cmp("a1", "a01"),
            "a1".cmp("a01"),
            "equal numbers fall back to plain order"
        );
    }

    #[test]
    fn fallback_label_follows_the_original_pattern() {
        // N-name_M: the name between the dash and the first underscore
        assert_eq!(fallback_label("2-playerone_0.flac"), "playerone");
        assert_eq!(fallback_label("6-gmhandle__0.mp3"), "gmhandle");
        // anything else: the whole base name
        assert_eq!(fallback_label("1-some_user.flac"), "1-some_user");
        assert_eq!(fallback_label("3-plain.flac"), "3-plain");
        assert_eq!(
            fallback_label("x-name_0.flac"),
            "x-name_0",
            "track number must be digits"
        );
        assert_eq!(
            fallback_label("1-name_.flac"),
            "1-name_",
            "digits required after the underscore"
        );
        assert_eq!(fallback_label("1-name_1a.flac"), "1-name_1a");
        assert_eq!(fallback_label("1-_0.flac"), "1-_0", "name part must be non-empty");
    }
}
