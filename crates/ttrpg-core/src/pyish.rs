//! Helpers that reproduce the Python behaviour `campaign.py` relied on.
//!
//! The paths computed by the original point at data that already exists (vault
//! transcripts, an existing Chroma store), so `os.path.abspath`,
//! `os.path.expanduser`, `os.path.join` and `str.strip` are ported by their
//! documented algorithms instead of being approximated with `std::path`.
//! Paths are plain `&str` throughout: this is Linux-only and UTF-8-only.

use serde_json::Value;

/// `str.isspace()`: Unicode White_Space plus the four ASCII separators U+001C..U+001F.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

/// `str.strip()` with no argument.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `posixpath.isabs`.
pub fn is_abs(path: &str) -> bool {
    path.starts_with('/')
}

/// `posixpath.join(a, b)`.
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `posixpath.join(first, *rest)`.
pub fn join_all(first: &str, rest: &[&str]) -> String {
    rest.iter().fold(first.to_string(), |acc, part| join(&acc, part))
}

/// `posixpath.normpath`: lexical only, never touches the filesystem.
pub fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    // POSIX allows exactly two leading slashes to be significant; three or more are one.
    let mut initial = usize::from(path.starts_with('/'));
    if path.starts_with("//") && !path.starts_with("///") {
        initial = 2;
    }
    let mut out: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        let keep = comp != ".." || (initial == 0 && out.is_empty()) || out.last().is_some_and(|last| *last == "..");
        if keep {
            out.push(comp);
        } else if !out.is_empty() {
            out.pop();
        }
    }
    let joined = format!("{}{}", "/".repeat(initial), out.join("/"));
    if joined.is_empty() { ".".to_string() } else { joined }
}

/// `posixpath.abspath`, with the working directory passed in.
pub fn abspath(path: &str, cwd: &str) -> String {
    if is_abs(path) {
        normpath(path)
    } else {
        normpath(&join(cwd, path))
    }
}

/// `posixpath.expanduser`. `home` is the value of `$HOME` if it is set (even when empty).
pub fn expanduser(path: &str, home: Option<&str>) -> String {
    if !path.starts_with('~') {
        return path.to_string();
    }
    let end = path[1..].find('/').map_or(path.len(), |i| i + 1);
    let userhome = if end == 1 {
        match home {
            Some(h) => h.to_string(),
            None => match current_user_home() {
                Some(h) => h,
                None => return path.to_string(),
            },
        }
    } else {
        match passwd_home(&path[1..end]) {
            Some(h) => h,
            None => return path.to_string(),
        }
    };
    let userhome = userhome.trim_end_matches('/');
    let joined = format!("{userhome}{}", &path[end..]);
    if joined.is_empty() { "/".to_string() } else { joined }
}

#[allow(deprecated)]
fn current_user_home() -> Option<String> {
    std::env::home_dir().map(|p| p.to_string_lossy().into_owned())
}

fn passwd_home(name: &str) -> Option<String> {
    let text = std::fs::read_to_string("/etc/passwd").ok()?;
    text.lines().find_map(|line| {
        let f: Vec<&str> = line.split(':').collect();
        (f.len() >= 6 && f[0] == name).then(|| f[5].to_string())
    })
}

/// `os.path.abspath(os.path.expanduser(value))`.
pub fn expand_abs(value: &str, home: Option<&str>) -> String {
    abspath(&expanduser(value, home), &cwd())
}

pub fn cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".to_string())
}

/// `re.sub(r"\s+", "_", str(name).strip())`, the character identity used in
/// metadata and speaker labels ("Mara Quill" -> "Mara_Quill").
pub fn normalize_character(name: &str) -> String {
    let s = strip(name);
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if is_space(c) {
            if !in_space {
                out.push('_');
                in_space = true;
            }
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// Python truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_none_or(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `repr()` of a Python string, for the places the original interpolates `{x!r}`.
/// Non-ASCII whitespace and control characters are escaped; other characters are
/// kept (this approximates `str.isprintable` without a Unicode category table).
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() || (c.is_whitespace() && c != ' ') => {
                let n = c as u32;
                if n <= 0xff {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n <= 0xffff {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `repr()` of a JSON value as Python would print the parsed object.
pub fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() => format!("{f:?}"),
            _ => n.to_string(),
        },
        Value::String(s) => repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", repr_str(k), repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `str()` of a JSON value: strings are themselves, everything else is its repr.
pub fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => repr(other),
    }
}

/// `int(str(value).strip())` for the cases that matter: optional sign, decimal
/// digits, single underscores between digits.
pub fn parse_int(s: &str) -> Option<i128> {
    let t = strip(s);
    let (neg, body) = match t.as_bytes().first()? {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut n: i128 = 0;
    for b in body.bytes() {
        if b == b'_' {
            continue;
        }
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
    }
    Some(if neg { -n } else { n })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normpath_matches_posixpath() {
        let cases = [
            ("", "."),
            (".", "."),
            ("/", "/"),
            ("//", "//"),
            ("///", "/"),
            ("////a", "/a"),
            ("//a//b", "//a/b"),
            ("/a/./b/../c", "/a/c"),
            ("/..", "/"),
            ("/../..", "/"),
            ("a/../..", ".."),
            ("../a", "../a"),
            ("a//b///c/", "a/b/c"),
            ("/a/b/..", "/a"),
        ];
        for (input, want) in cases {
            assert_eq!(normpath(input), want, "normpath({input:?})");
        }
    }

    #[test]
    fn join_matches_posixpath() {
        assert_eq!(join("a", "b"), "a/b");
        assert_eq!(join("a/", "b"), "a/b");
        assert_eq!(join("", "b"), "b");
        assert_eq!(join("a", "/b"), "/b");
        // posixpath.join('/x', 'y', '', 'z'): the empty part leaves a trailing slash
        assert_eq!(join_all("/x", &["y", "", "z"]), "/x/y/z");
    }

    #[test]
    fn expanduser_cases() {
        assert_eq!(expanduser("~", Some("/home/a")), "/home/a");
        assert_eq!(expanduser("~/x", Some("/home/a/")), "/home/a/x");
        assert_eq!(expanduser("~/x", Some("")), "/x");
        assert_eq!(expanduser("~", Some("")), "/");
        assert_eq!(expanduser("a/~", Some("/home/a")), "a/~");
        assert_eq!(
            expanduser("~definitely-no-such-user-xyz/x", Some("/h")),
            "~definitely-no-such-user-xyz/x"
        );
    }

    #[test]
    fn normalize_character_collapses_whitespace() {
        assert_eq!(normalize_character("Mara Quill"), "Mara_Quill");
        assert_eq!(normalize_character("  A \t B\u{a0}\u{a0}C  "), "A_B_C");
        assert_eq!(normalize_character(""), "");
    }

    #[test]
    fn int_parsing_matches_python() {
        assert_eq!(parse_int("0"), Some(0));
        assert_eq!(parse_int(" 12 "), Some(12));
        assert_eq!(parse_int("+3"), Some(3));
        assert_eq!(parse_int("-0"), Some(0));
        assert_eq!(parse_int("1_0"), Some(10));
        assert_eq!(parse_int("1__0"), None);
        assert_eq!(parse_int("_1"), None);
        assert_eq!(parse_int("1.5"), None);
        assert_eq!(parse_int("abc"), None);
        assert_eq!(parse_int(""), None);
    }

    #[test]
    fn repr_str_quotes_like_python() {
        assert_eq!(repr_str("abc"), "'abc'");
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("it's \"x\""), "'it\\'s \"x\"'");
        assert_eq!(repr_str("a\\b\n"), "'a\\\\b\\n'");
    }
}
