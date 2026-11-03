//! The target of a request serve answers, split into its parts ([`urlsplit`]) and its query into its parameters
//! ([`parse_qsl`]).

/// The parts of a URL, each as it came, its % escapes kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitUrl {
    /// Before the first `:`, when that is a scheme (a letter, then letters, digits, `+`, `-` and `.`), in lower case;
    /// empty when there is none.
    pub scheme: String,
    /// After a `//` that starts what follows the scheme, up to the first `/`, `?` or `#`; empty without the `//`.
    pub netloc: String,
    /// What is left before the first `?` and `#`.
    pub path: String,
    /// Between the first `?` and the first `#` after it; empty without a `?`.
    pub query: String,
    /// After the first `#`; empty without one.
    pub fragment: String,
}

/// url split into its parts, after its leading C0 controls and blanks are dropped and every tab, CR and LF in it
/// removed. A netloc is kept as it is, whatever it holds.
pub fn urlsplit(url: &str) -> SplitUrl {
    let url: String = url.trim_start_matches(|c: char| c <= ' ').chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let mut rest = url.as_str();
    let mut scheme = String::new();
    if let Some(i) = rest.find(':') {
        let candidate = &rest[..i];
        let is_scheme = candidate.starts_with(|c: char| c.is_ascii_alphabetic())
            && candidate.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if is_scheme {
            scheme = candidate.to_ascii_lowercase();
            rest = &rest[i + 1..];
        }
    }
    let mut netloc = "";
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        (netloc, rest) = after.split_at(end);
    }
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    SplitUrl {
        scheme,
        netloc: netloc.to_string(),
        path: path.to_string(),
        query: query.to_string(),
        fragment: fragment.to_string(),
    }
}

/// The name and value pairs of a query string, in order: split at each `&`, empty pieces skipped, a piece without `=`
/// a name with an empty value, each name and value unquoted by [`unquote_plus`].
pub fn parse_qsl(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|piece| !piece.is_empty())
        .map(|piece| {
            let (name, value) = piece.split_once('=').unwrap_or((piece, ""));
            (unquote_plus(name), unquote_plus(value))
        })
        .collect()
}

/// s with each `+` a blank and each `%XX` (two hex digits, either case) the byte it names, the bytes then read as
/// UTF-8 with each invalid sequence U+FFFD; a `%` without two hex digits after it stays as it is, and a character
/// beyond ASCII as it is.
pub fn unquote_plus(s: &str) -> String {
    let s = s.replace('+', " ");
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(high), Some(low)) => {
                // Two hex digits make a value below 256.
                out.push(u8::try_from(high * 16 + low).unwrap_or(u8::MAX));
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests;
