//! The template language of prompts, handoff file names and proof commands: `{name}` is replaced by the value of
//! the placeholder of that name, and `{{` and `}}` stand for a literal brace. That is all of it. A name is as is_name
//! says, as the names of workflows, roles and steps are, since `{<step>_path}` holds a step's. Any other use of a
//! brace is an error naming the placeholder and where it starts, so that a template meant for Python's `str.format` — with a conversion (`!r`), a
//! format spec (`:>10`), an attribute (`.upper`), an index (`[0]`) or a positional field (`{}`, `{0}`) — is refused
//! when the file holding it is read, not filled in some other way.

use std::fmt;

/// A piece of a template, in the order it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    /// Text as it is filled in, each `{{` and `}}` already made a single brace.
    Text(String),
    /// A placeholder, by its name without the braces.
    Placeholder(String),
}

/// Why a template cannot be read or filled in. Its message names the placeholder or the brace and its line and column
/// in the template, both counted from 1 in characters; the template's own name is the caller's to add.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateError(String);

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TemplateError {}

/// template as its pieces; adjacent text is one piece, and a template without placeholders is at most one piece.
///
/// Errors: a `{` with no `}` after it, a `}` that closes no placeholder and is not doubled, and a placeholder that is
/// not a name alone, as the module's comment lists them.
pub fn parse(template: &str) -> Result<Vec<Piece>, TemplateError> {
    Ok(scan(template)?.0)
}

/// parse's pieces, and the byte offset in template of each placeholder's `{`, in the order written.
fn scan(template: &str) -> Result<(Vec<Piece>, Vec<usize>), TemplateError> {
    let mut pieces = Vec::new();
    let mut starts = Vec::new();
    let mut text = String::new();
    let mut at = 0;
    while let Some(i) = template[at..].find(['{', '}']) {
        let brace = at + i;
        text.push_str(&template[at..brace]);
        let (placeholder, next) = read_brace(template, brace)?;
        match placeholder {
            None => text.push_str(&template[brace..=brace]),
            Some(name) => {
                if !text.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                }
                pieces.push(Piece::Placeholder(name.to_string()));
                starts.push(brace);
            }
        }
        at = next;
    }
    text.push_str(&template[at..]);
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
    Ok((pieces, starts))
}

/// What the brace at byte offset at of template starts: the name of a placeholder, or None for a doubled brace, which
/// stands for one; and the offset just past it.
///
/// Errors: those of parse, for this brace.
fn read_brace(template: &str, at: usize) -> Result<(Option<&str>, usize), TemplateError> {
    let brace = &template[at..=at];
    if template[at + 1..].starts_with(brace) {
        return Ok((None, at + 2));
    }
    if brace == "}" {
        return Err(error(template, at, "}", "closes no placeholder; write a literal brace as }}"));
    }
    // A placeholder is a name, so it cannot span lines: a { with no } on its line is a stray brace.
    let line_end = template[at..].find('\n').map_or(template.len(), |i| at + i);
    let Some(end) = template[at..line_end].find('}').map(|i| at + i) else {
        return Err(error(template, at, "{", "is not closed on its line; write a literal brace as {{"));
    };
    let name = &template[at + 1..end];
    if let Some(why) = name_problem(name) {
        return Err(error(template, at, &format!("placeholder {}", &template[at..=end]), why));
    }
    Ok((Some(name), end + 1))
}

/// The names of template's placeholders in the order written, a name used twice there twice.
///
/// Errors: those of parse.
pub fn placeholders(template: &str) -> Result<Vec<String>, TemplateError> {
    Ok(parse(template)?
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Placeholder(name) => Some(name),
            Piece::Text(_) => None,
        })
        .collect())
}

/// template with each placeholder replaced by its value, which values gives by name, or None for a name the template
/// may not use. Filled with empty values for the names it may use, it checks a template as it is read.
///
/// Errors: those of parse, and then the first placeholder, in the order written, that values has no value for.
pub fn fill(template: &str, values: &dyn Fn(&str) -> Option<String>) -> Result<String, TemplateError> {
    let (pieces, starts) = scan(template)?;
    let mut starts = starts.into_iter();
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push_str(&text),
            Piece::Placeholder(name) => {
                let at = starts.next().expect("scan gives each placeholder's start");
                let unknown = || error(template, at, &format!("placeholder {{{name}}}"), "is not one this template takes");
                out.push_str(&values(&name).ok_or_else(unknown)?);
            }
        }
    }
    Ok(out)
}

/// Whether text is a name: one or more ASCII letters, digits, `-` and `_`. The names of roles, steps and workflows are
/// held to it too, since they name agents, panes, phases and prompt placeholders.
pub fn is_name(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_name_char)
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// What is wrong with name, written between a placeholder's braces, as the rest of a sentence that starts with the
/// placeholder; None when it is a name. The first character outside a name decides what the template asked for.
fn name_problem(name: &str) -> Option<&'static str> {
    let end = name.find(|c| !is_name_char(c)).unwrap_or(name.len());
    let (head, tail) = name.split_at(end);
    let accessor = |c: char| matches!(c, '!' | ':' | '.' | '[');
    let positional = head.bytes().all(|b| b.is_ascii_digit()) && tail.chars().next().is_none_or(accessor);
    if positional {
        return Some("is a positional field; a placeholder is a name");
    }
    match tail.chars().next()? {
        '!' => Some("has a conversion; a placeholder takes none"),
        ':' => Some("has a format spec; a placeholder takes none"),
        '.' => Some("has an attribute; a placeholder takes none"),
        '[' => Some("has an index; a placeholder takes none"),
        _ => Some("is not a name; a name may use only letters, digits, - and _"),
    }
}

/// The error that what, written at byte offset at of template, has the problem why.
fn error(template: &str, at: usize, what: &str, why: &str) -> TemplateError {
    let before = &template[..at];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    TemplateError(format!("{what} at line {line}, column {column} {why}"))
}

#[cfg(test)]
mod tests;
