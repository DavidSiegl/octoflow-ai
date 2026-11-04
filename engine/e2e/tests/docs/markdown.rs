//! The parts of a markdown document the checks read: its lines, outside or inside fenced code, and in a line its code
//! spans, flags, links and, in a heading, the anchor GitHub gives it.

use std::collections::{HashMap, HashSet};

/// A line of a markdown document, but no fence line (```) itself.
#[derive(Clone, Copy)]
pub struct Line<'a> {
    /// The line's number, from 1.
    pub number: usize,
    pub text: &'a str,
    /// true means the line is inside a fenced code block.
    pub fenced: bool,
}

/// The lines of a markdown document's text, the fence lines left out.
pub fn lines(text: &str) -> Vec<Line<'_>> {
    let mut fenced = false;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        out.push(Line { number: i + 1, text: line, fenced });
    }
    out
}

/// The texts of a line's code spans, each between a backtick and the next, left to right; an empty pair (``) is no
/// span, and its second backtick may open one.
pub fn code_spans(line: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut from = 0;
    while let Some(open) = line[from..].find('`').map(|i| from + i) {
        let Some(close) = line[open + 1..].find('`').map(|i| open + 1 + i) else { break };
        if close == open + 1 {
            from = close;
        } else {
            spans.push(&line[open + 1..close]);
            from = close + 1;
        }
    }
    spans
}

/// The line with each code span, empty ones too, taken out; an unpaired backtick stays.
pub fn without_code(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('`') {
        let Some(close) = rest[open + 1..].find('`') else { break };
        out.push_str(&rest[..open]);
        rest = &rest[open + 1 + close + 1..];
    }
    out.push_str(rest);
    out
}

/// Whether c is a character of a word: a letter, a digit or _.
pub fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The long flags text names, in order: `--` and a lowercase letter, then lowercase letters, digits and hyphens, where
/// the `--` follows no word character and no hyphen.
pub fn flags(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut prev: Option<char> = None;
    for (i, c) in text.char_indices() {
        let free = !prev.is_some_and(|p| word_char(p) || p == '-');
        prev = Some(c);
        if c != '-' || !free || !text[i..].starts_with("--") {
            continue;
        }
        let rest = &text[i + 2..];
        if !rest.starts_with(|c: char| c.is_ascii_lowercase()) {
            continue;
        }
        let len = rest.find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        let len = len.unwrap_or(rest.len());
        found.push(&text[i..i + 2 + len]);
    }
    found
}

/// The targets of a line's links: each `](target)`, where target holds no space and no `)`, and each `href="..."` and
/// `src="..."`.
pub fn link_targets(line: &str) -> Vec<&str> {
    let mut targets = Vec::new();
    for (i, _) in line.match_indices("](") {
        let rest = &line[i + 2..];
        let len = rest.find(|c: char| c == ')' || c.is_whitespace()).unwrap_or(rest.len());
        if len > 0 && rest[len..].starts_with(')') {
            targets.push(&rest[..len]);
        }
    }
    for attribute in ["href=\"", "src=\""] {
        for (i, _) in line.match_indices(attribute) {
            let rest = &line[i + attribute.len()..];
            if let Some(len) = rest.find('"').filter(|&len| len > 0) {
                targets.push(&rest[..len]);
            }
        }
    }
    targets
}

/// The #anchors GitHub gives the headings of a markdown document's text: the heading lowercased, all but letters,
/// digits, `_`, `-` and spaces dropped, spaces as hyphens, and -1, -2, ... after one already given.
pub fn anchors(text: &str) -> HashSet<String> {
    let mut anchors = HashSet::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for line in lines(text).into_iter().filter(|l| !l.fenced) {
        let Some(heading) = heading(line.text) else { continue };
        let slug: String = heading
            .to_lowercase()
            .chars()
            .filter(|&c| word_char(c) || c == '-' || c == ' ')
            .map(|c| if c == ' ' { '-' } else { c })
            .collect();
        let count = seen.entry(slug.clone()).or_insert(0);
        anchors.insert(if *count == 0 { slug } else { format!("{slug}-{count}") });
        *count += 1;
    }
    anchors
}

/// The text of a heading line, as `## Text ##` gives `Text`; None for a line that is no heading: one opening with
/// other than 1 to 6 `#` and a space.
fn heading(line: &str) -> Option<&str> {
    let hashes = line.len() - line.trim_start_matches('#').len();
    let rest = &line[hashes..];
    if !(1..=6).contains(&hashes) || !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(rest.trim().trim_end_matches('#').trim_end())
}
