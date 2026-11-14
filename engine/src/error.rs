//! The error that ends a run.

use std::fmt;

/// A failure that ends the run; the message is shown to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratorError(pub String);

impl OrchestratorError {
    pub fn new(message: impl Into<String>) -> Self {
        OrchestratorError(message.into())
    }
}

impl fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OrchestratorError {}

/// An I/O error as the engine's messages word it: `[Errno 2] No such file or directory`, then `: "<path>"`, the path
/// quoted as Rust's Debug quotes a str, for an error that names one, as opening a file's does and reading an open one's does not; an
/// error that is not the system's as Rust words it.
pub fn os_error(e: &std::io::Error, path: Option<&str>) -> String {
    match (e.raw_os_error(), path) {
        (Some(errno), Some(path)) => format!("[Errno {errno}] {}: {path:?}", strerror(e)),
        (Some(errno), None) => format!("[Errno {errno}] {}", strerror(e)),
        (None, _) => e.to_string(),
    }
}

/// What an I/O error says, without Rust's " (os error N)": the system's description of its errno alone.
pub fn strerror(e: &std::io::Error) -> String {
    let text = e.to_string();
    match text.rfind(" (os error ") {
        Some(i) if e.raw_os_error().is_some() => text[..i].to_string(),
        _ => text,
    }
}

/// Why text is not TOML, on one line: the parser's message, then `(at line L, column C)` where it found the problem,
/// both counted from 1, the column in characters; the message alone when the parser names no place.
pub fn toml_syntax_error(text: &str, e: &toml::de::Error) -> String {
    let message = e.message().trim_end();
    match e.span() {
        Some(span) => {
            let before = &text[..span.start.min(text.len())];
            let line = before.matches('\n').count() + 1;
            let column = before.rsplit('\n').next().unwrap_or(before).chars().count() + 1;
            format!("{message} (at line {line}, column {column})")
        }
        None => message.to_string(),
    }
}

/// text as a message quotes it, cut to at most n characters: as it is when it fits, else its first n - 1 characters
/// and "…". n is at least 1.
pub fn shortened(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        return text.to_string();
    }
    let mut out: String = text.chars().take(n - 1).collect();
    out.push('…');
    out
}

/// The one of names an error offers as "did you mean" for word: the fewest single-character insertions, deletions
/// and substitutions away from word (the Levenshtein distance), the first in names' order on a tie. None when even
/// that one is further than half the length, in characters, of the longer of the two, as "x" is from "y": a name
/// that far is no likelier a misspelling than any other.
pub fn closest_name<'a>(word: &str, names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let word: Vec<char> = word.chars().collect();
    let mut best: Option<(usize, &'a str)> = None;
    for name in names {
        let chars: Vec<char> = name.chars().collect();
        let distance = edit_distance(&word, &chars);
        if 2 * distance > word.len().max(chars.len()) || best.is_some_and(|(d, _)| d <= distance) {
            continue;
        }
        best = Some((distance, name));
    }
    best.map(|(_, name)| name)
}

/// The Levenshtein distance from a to b, by the row-at-a-time dynamic programme.
fn edit_distance(a: &[char], b: &[char]) -> usize {
    // row[j] is the distance from the part of a done so far to b[..j].
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substituted = diagonal + usize::from(ca != cb);
            diagonal = row[j + 1];
            row[j + 1] = substituted.min(row[j] + 1).min(diagonal + 1);
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests;
