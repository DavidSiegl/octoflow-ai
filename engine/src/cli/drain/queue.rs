//! What drain runs: the specs in DIR, DIR/<slug>/spec.md, and the tickets beside them,
//! DIR/<slug>/issues/<NN>-<name>.md. Each has a header of `<key>: <entry>` lines before its first `## ` heading
//! (header_field, with_header_field); they are read into one queue whose blockers are checked (read_queue), and each
//! one's header changes are written back as drain makes them (Queued).

use std::collections::HashSet;

use crate::error::{strerror, OrchestratorError};
use crate::human::Human;
use crate::paths::{file_name, replace_local_file};
use crate::spec_file::{read_spec_file, SpecFile};

use super::{SPEC_DONE, TICKET_RESOLVED};

/// The keys of the header lines that drain reads. It writes all but BLOCKED_BY_KEY. Blocked by: is a comma-separated
/// list of what must be finished first (Queued::finished), each explanation in parentheses dropped first
/// (blocker_entries): a spec's names specs, each by its slug or a path ending in <slug>/spec.md (blocker_slugs); a
/// ticket's names tickets, each by its number or its file name without .md, or <slug>/<either> for a ticket of another
/// spec (ticket_blockers). Run: names the run drain started for it, and PR: that run's pull request.
pub const STATUS_KEY: &str = "Status";
pub const BLOCKED_BY_KEY: &str = "Blocked by";
pub const RUN_KEY: &str = "Run";
pub const PR_KEY: &str = "PR";

/// A Blocked by: value that names no blocker, as the specs and tickets here write it, in lowercase.
const NO_BLOCKERS: [&str; 2] = ["-", "none"];

/// Header lines to set, each a key of the header and its entry, in the order set; a key set again keeps its place.
pub type Entries = Vec<(&'static str, String)>;

/// Set each of more in entries: a key entries has keeps its place and takes the new entry.
pub fn merge(entries: &mut Entries, more: Entries) {
    for (key, entry) in more {
        match entries.iter_mut().find(|(k, _)| *k == key) {
            Some(found) => found.1 = entry,
            None => entries.push((key, entry)),
        }
    }
}

/// The entry entries sets for key, if it sets one.
pub fn entry<'e>(entries: &'e Entries, key: &str) -> Option<&'e str> {
    entries.iter().find(|(k, _)| *k == key).map(|(_, e)| e.as_str())
}

/// The index of the first line of lines that starts the spec's body, a `## ` heading; lines.len() without one.
fn header_end(lines: &[&str]) -> usize {
    lines.iter().position(|line| line.starts_with("## ")).unwrap_or(lines.len())
}

/// The entry on line when it is key's, `<key>:` and the entry, blanks at its ends removed; None for another line.
fn keyed_text<'l>(line: &'l str, key: &str) -> Option<&'l str> {
    let body = line.trim_end_matches(['\r', '\n']);
    body.strip_prefix(key)?.strip_prefix(':').map(str::trim)
}

/// The entry on the spec header's first key line, blanks at its ends removed; None when the header has none.
pub fn header_field(text: &str, key: &str) -> Option<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    lines[..header_end(&lines)].iter().find_map(|line| keyed_text(line, key)).map(String::from)
}

/// text with the header's first key line made `<key>: <entry>`, or with that line added directly below its Status:
/// line when it has none. Every other line, and every line's ending, stays as it was; an added line ends as the
/// Status: line does, and below a Status: line that ends the file unterminated, ends the file so instead. Errors: the
/// header has neither line.
pub fn with_header_field(text: &str, key: &str, entry: &str) -> Result<String, OrchestratorError> {
    let mut lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();
    let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
    let end = header_end(&borrowed);
    let mut status_at = None;
    for (i, line) in borrowed[..end].iter().enumerate() {
        let ending = &line[line.trim_end_matches(['\r', '\n']).len()..];
        if keyed_text(line, key).is_some() {
            lines[i] = format!("{key}: {entry}{ending}");
            return Ok(lines.concat());
        }
        if status_at.is_none() && keyed_text(line, STATUS_KEY).is_some() {
            status_at = Some(i);
        }
    }
    let Some(status_at) = status_at else {
        return Err(OrchestratorError(format!("the spec has no {STATUS_KEY}: line to add {key}: below")));
    };
    let status_line = &lines[status_at];
    let ending = status_line[status_line.trim_end_matches(['\r', '\n']).len()..].to_string();
    if ending.is_empty() {
        // The Status: line ends the file without a line ending: the new line takes its place as the last.
        lines[status_at].push_str(if text.contains("\r\n") { "\r\n" } else { "\n" });
        lines.push(format!("{key}: {entry}"));
    } else {
        lines.insert(status_at + 1, format!("{key}: {entry}{ending}"));
    }
    Ok(lines.concat())
}

/// The entries of a Blocked by: line's value, in order, blanks at their ends removed. Each explanation in parentheses
/// is dropped before the value is split on commas, so a comma inside one makes no entry: `02 (same file, also the UI)`
/// is one entry, `02`. None without the line, or when what is left is one of NO_BLOCKERS and nothing else:
/// `none (independent)` names no blocker, but `none yet, 03` names `none yet` and `03`.
fn blocker_entries(line: Option<&str>) -> Vec<String> {
    let line = without_parentheses(line.unwrap_or(""));
    if NO_BLOCKERS.contains(&line.trim().to_lowercase().as_str()) {
        return vec![];
    }
    line.split(',').map(str::trim).filter(|e| !e.is_empty()).map(String::from).collect()
}

/// text without the parts in parentheses, each with its parentheses, nested ones included; a `(` never closed drops
/// the rest of text, and a `)` that closes nothing stays.
fn without_parentheses(text: &str) -> String {
    let mut depth = 0;
    let mut kept = String::new();
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => kept.push(c),
            _ => {}
        }
    }
    kept
}

/// The slugs a spec's Blocked by: line's value names, in order (BLOCKED_BY_KEY).
pub fn blocker_slugs(line: Option<&str>) -> Vec<String> {
    blocker_entries(line)
        .into_iter()
        .map(|e| if e.ends_with("/spec.md") { file_name(dirname(&e)).to_string() } else { e })
        .collect()
}

/// The number a ticket's file name without .md starts with, as in 03-close-dialog or 03; None for a name that does
/// not start with digits followed by a hyphen or nothing, which is no ticket.
pub fn ticket_number(stem: &str) -> Option<u64> {
    let digits = stem.len() - stem.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let rest = &stem[digits..];
    (digits > 0 && (rest.is_empty() || rest.starts_with('-'))).then(|| stem[..digits].parse().ok()).flatten()
}

/// What a Blocked by: line's value names for a ticket of spec own (BLOCKED_BY_KEY): the slugs, among queue, of the
/// tickets it names, in order; and, for each entry that names no single ticket of queue, why, for the human. An entry's
/// first word counts. A number names the ticket of own with that number (ticket_number), so a ticket renamed since
/// still counts; a file name without .md only the ticket of that name, so `02-old-name` names no ticket once 02 is
/// renamed. <slug>/<either> names one of spec slug's so, whatever that spec's status.
fn ticket_blockers(own: &str, line: Option<&str>, queue: &[Queued]) -> (Vec<String>, Vec<String>) {
    let (mut slugs, mut unread) = (vec![], vec![]);
    for entry in blocker_entries(line) {
        let word = entry.split_whitespace().next().unwrap_or_default();
        let (spec, name) = word.rsplit_once('/').unwrap_or((own, word));
        let by_number = !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit());
        let of_spec = Kind::Ticket(spec.to_string());
        let named: Vec<&str> = queue
            .iter()
            .filter(|q| q.kind == of_spec)
            .filter(|q| {
                let stem = file_name(&q.slug);
                if by_number {
                    ticket_number(stem) == ticket_number(name)
                } else {
                    stem == name
                }
            })
            .map(|q| q.slug.as_str())
            .collect();
        match named.as_slice() {
            [one] => slugs.push(one.to_string()),
            [] => unread.push(format!("its {BLOCKED_BY_KEY}: entry `{entry}` names no ticket")),
            many => {
                unread.push(format!("its {BLOCKED_BY_KEY}: entry `{entry}` names more than one: {}", many.join(", ")))
            }
        }
    }
    (slugs, unread)
}

/// The tickets in spec_dir/issues, each its file name without .md and its path, in number order (ticket_number), then
/// by name; none without that directory. A name that is no ticket's, or not UTF-8, is left out. Errors: the directory
/// exists but cannot be listed.
fn ticket_files(spec_dir: &str) -> Result<Vec<(String, String)>, OrchestratorError> {
    let dir = format!("{spec_dir}/issues");
    let listing = |e: std::io::Error| OrchestratorError(format!("cannot list the tickets in {dir}: {}", strerror(&e)));
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(listing(e)),
    };
    let mut tickets = vec![];
    for entry in entries {
        let Ok(name) = entry.map_err(listing)?.file_name().into_string() else { continue };
        let Some(stem) = name.strip_suffix(".md") else { continue };
        let Some(number) = ticket_number(stem) else { continue };
        let path = format!("{dir}/{name}");
        if std::path::Path::new(&path).is_file() {
            tickets.push((number, stem.to_string(), path));
        }
    }
    tickets.sort();
    Ok(tickets.into_iter().map(|(_, stem, path)| (stem, path)).collect())
}

/// path's directory: up to its last slash, trailing slashes removed unless it is
/// all slashes.
fn dirname(path: &str) -> &str {
    let head = &path[..path.rfind('/').map_or(0, |i| i + 1)];
    let trimmed = head.trim_end_matches('/');
    if trimmed.is_empty() {
        head
    } else {
        trimmed
    }
}

/// What a file drain found is to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A spec drain runs whole: it has no tickets.
    Spec,
    /// A spec with tickets, which drain never runs: while it is SPEC_READY, drain runs its tickets instead, and makes
    /// it SPEC_DONE once each is TICKET_RESOLVED or TICKET_WONTFIX.
    Ticketed,
    /// A ticket, DIR/<slug>/issues/<NN>-<name>.md, of the spec whose slug this is, which drain runs only while that
    /// spec is SPEC_READY; the directory may have no spec.md.
    Ticket(String),
}

/// A spec or ticket that drain found, with what drain has done to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    /// A spec's directory name; a ticket's spec's slug, a slash, and its file name without .md.
    pub slug: String,
    /// Whether it is a spec drain runs whole, one it runs ticket by ticket, or a ticket.
    pub kind: Kind,
    /// Its path as drain found it, and its text as drain last read it, with drain's changes to its header since, line
    /// endings as in the file.
    pub file: SpecFile,
    /// The slugs its Blocked by: line names (BLOCKED_BY_KEY), each of one drain found.
    pub blockers: Vec<String>,
    /// Why drain holds it for the human, for what it could not read: for a ticket, each entry of its Blocked by: line
    /// that names no single ticket drain found (ticket_blockers); for a spec, each of its tickets that cannot be read
    /// (read_spec_dir). Empty for none. A spec's bad blockers stop the drain instead (read_queue).
    pub unread: Vec<String>,
    /// Its status as drain first read it (Queued::status).
    pub was: Option<String>,
    /// True once drain has changed its header, or would have but for --dry-run.
    pub changed: bool,
    /// Why the spec is gone: drain could not read it again (read_spec_file's error), deleted or emptied while drain
    /// ran, so it is neither run nor done, and file stays as drain last read it. None while it can be read.
    pub gone: Option<String>,
}

impl Queued {
    /// The one drain read from file, of kind, with blockers, as yet unchanged.
    fn new(slug: String, kind: Kind, file: SpecFile, blockers: Vec<String>) -> Queued {
        let mut queued = Queued { slug, kind, file, blockers, unread: vec![], was: None, changed: false, gone: None };
        queued.was = queued.status();
        queued
    }

    /// Its status as drain last read or set it, which a gone one keeps (status); None without a Status: line.
    pub fn status_as_read(&self) -> Option<String> {
        header_field(&self.file.text, STATUS_KEY).map(|line| status_word(&line).to_string())
    }

    /// Its status now: its Status: line's first word (status_word), as the tracker notes why after it; None without
    /// the line, or once it is gone.
    pub fn status(&self) -> Option<String> {
        self.field(STATUS_KEY).map(|line| status_word(&line).to_string())
    }

    /// What it is, for a message: spec or ticket.
    pub fn noun(&self) -> &'static str {
        if matches!(self.kind, Kind::Ticket(_)) {
            "ticket"
        } else {
            "spec"
        }
    }

    /// Whether what it blocks may start: a spec is SPEC_DONE, a ticket TICKET_RESOLVED.
    pub fn finished(&self) -> bool {
        let finished = if matches!(self.kind, Kind::Ticket(_)) { TICKET_RESOLVED } else { SPEC_DONE };
        self.status().as_deref() == Some(finished)
    }

    /// The run id on its Run: line; None without one, or once it is gone.
    pub fn run(&self) -> Option<String> {
        self.field(RUN_KEY)
    }

    /// The pull request's URL on its PR: line; None without one, or once it is gone.
    pub fn pr(&self) -> Option<String> {
        self.field(PR_KEY)
    }

    fn field(&self, key: &str) -> Option<String> {
        self.gone.is_none().then(|| header_field(&self.file.text, key)).flatten()
    }

    /// Take the file as it is on disk now, so an edit made while drain runs is kept, not overwritten, and return true;
    /// or return false, the spec gone, when it cannot be read. A gone spec read again is no longer gone. Its blockers
    /// stay as drain first read them.
    pub fn reread(&mut self) -> bool {
        match read_spec_file(&self.file.path) {
            Ok(file) => {
                self.file = file;
                self.gone = None;
                true
            }
            Err(e) => {
                self.gone = Some(e.0);
                false
            }
        }
    }

    /// Set each of the header's lines that entries names (with_header_field), and, with write, save the file as it is
    /// on disk now with those lines changed; return true. With write, a spec found gone is left as it is: it logs the
    /// entries it does not record to human and returns false. Errors: as with_header_field and replace_local_file.
    pub fn update(&mut self, entries: &Entries, write: bool, human: &dyn Human) -> Result<bool, OrchestratorError> {
        if write && !self.reread() {
            let listed: Vec<String> = entries.iter().map(|(k, v)| format!("{k}: {v}")).collect();
            let gone = self.gone.as_deref().unwrap_or_default();
            human.log(&format!("{}: {gone}; not recording {}", self.slug, listed.join(", ")));
            return Ok(false);
        }
        let mut text = self.file.text.clone();
        for (key, entry) in entries {
            text = with_header_field(&text, key, entry)?;
        }
        if write {
            replace_local_file(&self.file.path, &text)?;
        }
        self.file = SpecFile { path: self.file.path.clone(), text };
        self.changed = true;
        Ok(true)
    }

    /// The line drain prints for a spec it changed: its slug, old and new status, `gone` for a gone one, and pull
    /// request.
    pub fn change_line(&self) -> String {
        let new = if self.gone.is_some() { "gone".to_string() } else { status_text(self.status()) };
        let line = format!("{}: {} -> {new}", self.slug, status_text(self.was.clone()));
        match self.pr() {
            Some(pr) if !pr.is_empty() => format!("{line}  {pr}"),
            _ => line,
        }
    }
}

/// A header line's entry's first word, as drain reads a Status: line (Queued::status): `resolved` of
/// `resolved (PR #7 merged)`.
pub fn status_word(entry: &str) -> &str {
    entry.split_whitespace().next().unwrap_or_default()
}

/// A spec's status as drain writes it: "no status" for a spec without a Status: line, or with an empty one.
pub(super) fn status_text(value: Option<String>) -> String {
    value.filter(|s| !s.is_empty()).unwrap_or_else(|| "no status".to_string())
}

/// What each directory of spec_dir on this machine holds, ordered by its name (read_spec_dir): its spec.md, if it has
/// one, followed by its tickets, whatever the spec's status. A name in spec_dir that is not UTF-8 is left out. A
/// ticket's Blocked by: entries that name no single ticket, and a ticket that cannot be read, are kept as the unread of
/// the ticket and of its spec.
///
/// Errors, naming the specs or tickets: spec_dir or an issues directory cannot be listed, a spec cannot be read
/// (read_spec_file), a spec's Blocked by: line names a slug that has no spec.md, or what drain waits on forms a
/// cycle (waiting_edges).
pub fn read_queue(spec_dir: &str) -> Result<Vec<Queued>, OrchestratorError> {
    let listing =
        |e: std::io::Error| OrchestratorError(format!("cannot list the specs in {spec_dir}: {}", strerror(&e)));
    let mut names: Vec<String> = std::fs::read_dir(spec_dir)
        .map_err(listing)?
        .map(|entry| entry.map(|e| e.file_name().into_string().ok()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(listing)?
        .into_iter()
        .flatten()
        .collect();
    names.sort();
    let mut queue: Vec<Queued> = Vec::new();
    for name in names {
        queue.extend(read_spec_dir(spec_dir, &name)?);
    }
    check_spec_blockers(&queue, spec_dir)?;
    let tickets: Vec<(usize, String)> = (queue.iter().enumerate())
        .filter_map(|(i, q)| if let Kind::Ticket(spec) = &q.kind { Some((i, spec.clone())) } else { None })
        .collect();
    for (i, spec) in tickets {
        let line = header_field(&queue[i].file.text, BLOCKED_BY_KEY);
        (queue[i].blockers, queue[i].unread) = ticket_blockers(&spec, line.as_deref(), &queue);
    }
    let edges = waiting_edges(&queue);
    let blockers: Vec<(&str, &[String])> = edges.iter().map(|(slug, b)| (*slug, b.as_slice())).collect();
    if let Some(cycle) = blocker_cycle(&blockers) {
        return Err(OrchestratorError(format!("the blockers form a cycle: {}", cycle.join(" -> "))));
    }
    Ok(queue)
}

/// The spec spec_dir/name/spec.md, if there is one, then the tickets in spec_dir/name/issues (ticket_files) that can be
/// read, their blockers not yet read; none when name is no directory. The spec is Kind::Ticketed when there are
/// tickets, readable or not, and why each one cannot be read is in its unread. Without a spec.md, a ticket that cannot
/// be read is left out: no ticket of such a directory runs. Errors: as ticket_files, and as read_spec_file for the
/// spec.
fn read_spec_dir(spec_dir: &str, name: &str) -> Result<Vec<Queued>, OrchestratorError> {
    // One slash between DIR and the name, none added after an empty DIR or one ending in a slash.
    let sep = if spec_dir.is_empty() || spec_dir.ends_with('/') { "" } else { "/" };
    let dir = format!("{spec_dir}{sep}{name}");
    if !std::path::Path::new(&dir).is_dir() {
        return Ok(vec![]);
    }
    let tickets = ticket_files(&dir)?;
    let mut read = vec![];
    let path = format!("{dir}/spec.md");
    if std::path::Path::new(&path).is_file() {
        let file = read_spec_file(&path)?;
        let blockers = blocker_slugs(header_field(&file.text, BLOCKED_BY_KEY).as_deref());
        let kind = if tickets.is_empty() { Kind::Spec } else { Kind::Ticketed };
        read.push(Queued::new(name.to_string(), kind, file, blockers));
    }
    for (stem, path) in tickets {
        match read_spec_file(&path) {
            Ok(file) => read.push(Queued::new(format!("{name}/{stem}"), Kind::Ticket(name.to_string()), file, vec![])),
            Err(e) => {
                if let Some(spec) = read.first_mut().filter(|q| q.kind == Kind::Ticketed) {
                    spec.unread.push(e.0);
                }
            }
        }
    }
    Ok(read)
}

/// Errors: a spec of queue is blocked by a slug that has no spec.md, naming both; spec_dir is for the message.
fn check_spec_blockers(queue: &[Queued], spec_dir: &str) -> Result<(), OrchestratorError> {
    let is_spec = |q: &&Queued| !matches!(q.kind, Kind::Ticket(_));
    let slugs: HashSet<&str> = queue.iter().filter(is_spec).map(|q| q.slug.as_str()).collect();
    for spec in queue.iter().filter(is_spec) {
        let missing: Vec<&str> = spec.blockers.iter().map(String::as_str).filter(|b| !slugs.contains(b)).collect();
        if !missing.is_empty() {
            return Err(OrchestratorError(format!(
                "spec {} is blocked by {}, which has no spec.md in {spec_dir}",
                spec.slug,
                missing.join(", ")
            )));
        }
    }
    Ok(())
}

/// Each of queue by its slug, with the slugs of what drain waits on before it is finished or can start: its blockers;
/// for a ticket, its spec's too; for a Ticketed spec, its tickets.
fn waiting_edges(queue: &[Queued]) -> Vec<(&str, Vec<String>)> {
    let blockers_of =
        |slug: &str| queue.iter().find(|q| q.slug == slug).map(|q| q.blockers.clone()).unwrap_or_default();
    (queue.iter())
        .map(|q| {
            let mut waits = q.blockers.clone();
            match &q.kind {
                Kind::Ticket(spec) => waits.extend(blockers_of(spec)),
                Kind::Ticketed => {
                    let of = Kind::Ticket(q.slug.clone());
                    waits.extend(queue.iter().filter(|t| t.kind == of).map(|t| t.slug.clone()));
                }
                Kind::Spec => {}
            }
            (q.slug.as_str(), waits)
        })
        .collect()
}

/// A cycle among the specs and tickets, each by its slug with the slugs that block it, in order, as the slugs along it with the
/// first again at the end; None when there is none. Every blocker must be a slug of blockers.
pub fn blocker_cycle(blockers: &[(&str, &[String])]) -> Option<Vec<String>> {
    fn cycle_from<'b>(
        slug: &'b str,
        path: &mut Vec<&'b str>,
        blockers: &[(&'b str, &'b [String])],
        acyclic: &mut HashSet<&'b str>,
    ) -> Option<Vec<String>> {
        if let Some(at) = path.iter().position(|s| *s == slug) {
            return Some(path[at..].iter().chain([&slug]).map(|s| s.to_string()).collect());
        }
        if acyclic.contains(slug) {
            return None;
        }
        let of = blockers.iter().find(|(s, _)| *s == slug).map_or(&[][..], |(_, b)| *b);
        path.push(slug);
        for b in of {
            if let Some(found) = cycle_from(b, path, blockers, acyclic) {
                return Some(found);
            }
        }
        path.pop();
        acyclic.insert(slug);
        None
    }
    let mut acyclic = HashSet::new();
    blockers.iter().find_map(|(slug, _)| cycle_from(slug, &mut vec![], blockers, &mut acyclic))
}
