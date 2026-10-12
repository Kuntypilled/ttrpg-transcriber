//! `transcription_state.csv`, the per-file record a run appends to (PORTING.md section
//! 4.2). Every field is in double quotes and quotes inside them are not escaped (the
//! original wrote them raw; the Rust run turns them into single quotes), so this reads
//! it with that assumption rather than as general CSV.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRow {
    pub file: String,
    pub size: String,
    pub seconds: String,
    pub status: String,
    pub timestamp: String,
    pub player: String,
    pub error: String,
}

impl StateRow {
    pub fn succeeded(&self) -> bool {
        self.status == "Success"
    }
}

/// `"a","b",...,"g"` -> row. The header row and anything malformed give `None`.
/// Any extra `","` inside the last field stays in the error message.
pub fn parse_state_line(line: &str) -> Option<StateRow> {
    let inner = line
        .trim_end_matches(['\r', '\n'])
        .strip_prefix('"')?
        .strip_suffix('"')?;
    let mut f = inner.splitn(7, "\",\"");
    let mut next = || f.next().map(str::to_string);
    let row = StateRow {
        file: next()?,
        size: next()?,
        seconds: next()?,
        status: next()?,
        timestamp: next()?,
        player: next()?,
        error: next()?,
    };
    Some(row)
}

/// All rows of the state file in order. A missing or unreadable file has none.
pub fn read_state_rows(path: &str) -> Vec<StateRow> {
    std::fs::read(path)
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .lines()
                .filter_map(parse_state_line)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_success_row() {
        let r =
            parse_state_line("\"1-a.flac\",\"14420\",\"27\",\"Success\",\"2026-08-03 21:22:54\",\"GM\",\"\"").unwrap();
        assert_eq!(
            (r.file.as_str(), r.status.as_str(), r.player.as_str(), r.error.as_str()),
            ("1-a.flac", "Success", "GM", "")
        );
        assert!(r.succeeded());
    }

    #[test]
    fn error_messages_keep_commas_and_extra_separators() {
        let r = parse_state_line("\"a.flac\",\"1\",\"2\",\"Error\",\"t\",\"p\",\"bad, very\",\"bad\"").unwrap();
        assert_eq!(r.error, "bad, very\",\"bad");
        assert!(!r.succeeded());
    }

    #[test]
    fn header_and_junk_are_not_rows() {
        for line in [
            "FileName,FileSize,ProcessingTime,Status,Timestamp,PlayerName,ErrorMessage",
            "",
            "\"a\",\"b\"",
            "x",
        ] {
            assert_eq!(parse_state_line(line), None, "{line:?}");
        }
    }

    #[test]
    fn missing_file_has_no_rows() {
        assert!(read_state_rows("/nonexistent/state.csv").is_empty());
    }
}
