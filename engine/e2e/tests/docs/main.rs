//! The README and docs/ against the code they describe: the commands and flags they give, the names and the links.
//!
//! The commands are the orchestrator's, read from its help (cli.rs), the binary E2E_ENGINE names or else the
//! checkout's, built with cargo (octoflow_e2e::engine). The names are those the engine's and the desktop shell's Rust
//! define, read by parsing it (names.rs).
//!
//! Each failure names the document and line that is wrong, as `file:line: what`.

mod cli;
mod markdown;
mod names;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::LazyLock;

use markdown::{code_spans, flags, word_char, Line};

/// The repository's root.
static ROOT: LazyLock<PathBuf> = LazyLock::new(octoflow_e2e::checkout);

/// Commands and flags the docs give that the orchestrator does not have, as (document, command, flag); a flag of
/// None stands for the command itself.
const NOT_IN_THE_PARSER: &[(&str, &str, Option<&str>, &str)] = &[
    ("roadmap.md", "resume", Some("--workflow"), "a planned feature, K"),
    ("architecture.md", "run", Some("--run-id"), "hidden from run's help: serve's, for the runs POST /run starts"),
];

/// Backticked ALL_CAPS names the docs give that are not the Rust's, with where they come from.
const OTHER_PROGRAMS_NAMES: &[(&str, &str)] = &[
    ("PATH", "an environment variable"),
    ("PWD", "an environment variable"),
    ("OWD", "the AppImage runtime's environment variable"),
    ("PYTHONHOME", "Python's environment variable"),
    ("LD_LIBRARY_PATH", "the dynamic loader's environment variable"),
    ("PYTEST_ADDOPTS", "pytest's environment variable"),
    ("CARGO_HOME", "cargo's environment variable"),
    ("HERDR_SESSION", "the herdr CLI's environment variable, which picks its session"),
    ("SSL_CERT_FILE", "OpenSSL's environment variable, which the Rust engine's HttpClient reads too"),
    ("SSL_CERT_DIR", "OpenSSL's environment variable, which the Rust engine's HttpClient reads too"),
    ("E2E_ENGINE", "the e2e package's environment variable, engine/e2e/src/engine.rs"),
    ("E2E_RECORD", "the e2e scenarios' environment variable, engine/e2e/tests/e2e/main.rs"),
    ("EVAL_ENGINE", "the eval runner's environment variable, xtask/src/eval.rs"),
    ("OCTOFLOW_TEST_SEED", "the engine's tests' environment variable, engine/src/test_support.rs"),
    ("HARNESSES", "the Rust engine's table of the agent harnesses, engine/src/harness.rs"),
    ("CI", "the Python engine's Jenkins client, which engine/src/ci.rs ports as Jenkins"),
    ("PR_STATUS_CREATOR", "the Jenkinsfile's"),
    ("GIT_REF", "the quality job's parameter, which the Jenkinsfile declares and the engine's tests send"),
    ("CHANGE_TARGET", "an environment variable Jenkins sets"),
    ("OPTIONS", "an HTTP method"),
    ("NAME", "the README's placeholder for a prompt's name"),
];

/// Documents that describe the code as it was, whose commands and names are not checked against today's, with why.
const HISTORICAL_DOCS: &[(&str, &str)] =
    &[("ui-options.md", "an assessment of 2026-10-24 of the Python engine's tk window, which 0.49.0 removed")];

/// Backticked names the docs give that read as Type.member but are file names, with whose files they are.
const FILE_NAMES: &[(&str, &str)] = &[("Cargo.toml", "cargo's manifest"), ("Cargo.lock", "cargo's lock file")];

/// A markdown document the check reads: README.md or one of docs/*.md.
struct Doc {
    path: PathBuf,
    /// The path from the repository's root, as a failure names it.
    shown: String,
    text: String,
}

impl Doc {
    fn name(&self) -> &str {
        self.path.file_name().and_then(|n| n.to_str()).unwrap_or("")
    }

    fn historical(&self) -> bool {
        HISTORICAL_DOCS.iter().any(|(name, _)| *name == self.name())
    }

    fn lines(&self) -> Vec<Line<'_>> {
        markdown::lines(&self.text)
    }

    fn at(&self, line: &Line) -> String {
        format!("{}:{}", self.shown, line.number)
    }
}

/// README.md and docs/*.md, in that order and docs/ by name. Panics when one cannot be read.
fn docs() -> Vec<Doc> {
    let dir = ROOT.join("docs");
    let entries = fs::read_dir(&dir).unwrap_or_else(|e| panic!("listing {}: {e}", dir.display()));
    let mut paths: Vec<PathBuf> = entries
        .map(|e| e.unwrap_or_else(|e| panic!("listing {}: {e}", dir.display())).path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    paths.sort();
    paths.insert(0, ROOT.join("README.md"));
    paths.into_iter().map(read_doc).collect()
}

fn read_doc(path: PathBuf) -> Doc {
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let shown = path.strip_prefix(&*ROOT).unwrap_or(&path).display().to_string();
    Doc { path, shown, text }
}

/// Fail with errors, one a line, unless there are none.
fn assert_no_errors(errors: &[String]) {
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

/// (line, command, the text of its arguments) for each command the document shows being run, in a code span or a
/// fenced line: after `orchestrator` or the Python engine's `orchestrator.py`; and, in a code span, a command opening
/// it with a flag after, as in `run --spec`. Shell comments, and what follows a pipe, `&&` or `;`, are left out.
fn invocations<'a>(doc: &'a Doc, commands: &BTreeMap<String, BTreeSet<String>>) -> Vec<(Line<'a>, String, String)> {
    let mut found = Vec::new();
    for line in doc.lines() {
        let texts = if !line.fenced {
            code_spans(line.text)
        } else if line.text.trim_start().starts_with('#') {
            Vec::new()
        } else {
            vec![line.text]
        };
        for text in texts.into_iter().map(first_command) {
            if let Some((command, rest)) = orchestrator_command(text) {
                found.push((line, command.to_string(), rest.to_string()));
            }
            let mut words = text.split_whitespace();
            if let (false, Some(first), Some(second)) = (line.fenced, words.next(), words.next()) {
                if commands.contains_key(first) && second.starts_with("--") {
                    found.push((line, first.to_string(), text.trim_start()[first.len()..].to_string()));
                }
            }
        }
    }
    found
}

/// text up to its first pipe, `&&`, `;` or a `#` after whitespace, which starts a shell comment.
fn first_command(text: &str) -> &str {
    let mut end = text.len();
    for stop in ["|", "&&", ";"] {
        end = end.min(text.find(stop).unwrap_or(end));
    }
    let comment = text.char_indices().zip(text.chars().skip(1)).find(|((_, c), d)| c.is_whitespace() && *d == '#');
    if let Some(((i, _), _)) = comment {
        end = end.min(i);
    }
    &text[..end]
}

/// Whether the word at byte i of text starts there: no word character before it.
fn starts_word(text: &str, i: usize) -> bool {
    !text[..i].chars().next_back().is_some_and(word_char)
}

/// The first `orchestrator <command>` or `orchestrator.py <command>` in text, where orchestrator starts a word and the
/// command, a lowercase letter then lowercase letters and hyphens, follows after whitespace: the command and the rest
/// of text after it.
fn orchestrator_command(text: &str) -> Option<(&str, &str)> {
    const PROGRAM: &str = "orchestrator";
    text.match_indices(PROGRAM).filter(|&(i, _)| starts_word(text, i)).find_map(|(i, _)| {
        let rest = &text[i + PROGRAM.len()..];
        let rest = rest.strip_prefix(".py").unwrap_or(rest);
        let args = rest.trim_start();
        if args.len() == rest.len() || !args.starts_with(|c: char| c.is_ascii_lowercase()) {
            return None;
        }
        let len = args.find(|c: char| !(c.is_ascii_lowercase() || c == '-')).unwrap_or(args.len());
        Some(args.split_at(len))
    })
}

#[test]
fn each_command_the_docs_run_takes_the_flags_they_give_it() {
    let commands = cli::commands();
    let mut errors = Vec::new();
    for doc in docs().iter().filter(|d| !d.historical()) {
        for (line, command, rest) in invocations(doc, commands) {
            let excused = |flag: Option<&str>| {
                NOT_IN_THE_PARSER.iter().any(|&(d, c, f, _)| d == doc.name() && c == command && f == flag)
            };
            let Some(defined) = commands.get(&command) else {
                if !excused(None) {
                    errors.push(format!("{}: the orchestrator has no command {command}", doc.at(&line)));
                }
                continue;
            };
            for flag in flags(&rest) {
                if !defined.contains(flag) && !excused(Some(flag)) {
                    errors.push(format!("{}: {command} takes no {flag}", doc.at(&line)));
                }
            }
        }
    }
    assert_no_errors(&errors);
}

/// A row of README.md's `| Flag | Description |` table.
struct FlagRow<'a> {
    line: Line<'a>,
    flags: Vec<&'a str>,
    description: String,
}

/// The rows of the document's table headed `| Flag | Description |`, its separator row left out.
fn flag_table(doc: &Doc) -> Vec<FlagRow<'_>> {
    let mut rows = Vec::new();
    let mut inside = false;
    for line in doc.lines() {
        if line.text.starts_with("| Flag | Description |") {
            inside = true;
        } else if inside && line.text.starts_with('|') {
            let cells: Vec<&str> = line.text.split('|').collect();
            let first = cells.get(1).copied().unwrap_or("");
            if !first.trim().chars().all(|c| c == '-') {
                let description = cells[2..cells.len().saturating_sub(1).max(2)].join("|").trim().to_string();
                rows.push(FlagRow { flags: flags(first), description, line });
            }
        } else if inside {
            break;
        }
    }
    rows
}

/// The commands a flag row's description opens with, as "`run` and `drain`:" or "`run` only:", the names in
/// backticks joined by `, `, ` and `, ` or `, `, and `, `, or ` or `/`, then a colon: the commands, and whether
/// "only" says no other takes the flag. None when the description opens otherwise.
fn row_scope(description: &str) -> Option<(BTreeSet<String>, bool)> {
    const SEPARATORS: [&str; 6] = [", and ", ", or ", " and ", " or ", ", ", "/"];
    let mut scope = BTreeSet::new();
    let mut rest = description;
    loop {
        let name = rest.strip_prefix('`')?;
        let len = name.find(|c: char| !(c.is_ascii_lowercase() || c == '-'))?;
        let after = name[len..].strip_prefix('`').filter(|_| len > 0)?;
        scope.insert(name[..len].to_string());
        rest = after;
        match SEPARATORS.iter().find_map(|s| after.strip_prefix(s)).filter(|next| next.starts_with('`')) {
            Some(next) => rest = next,
            None => break,
        }
    }
    let only = rest.starts_with(" only");
    let rest = if only { &rest[" only".len()..] } else { rest };
    rest.starts_with(':').then_some((scope, only))
}

#[test]
fn the_readme_flag_table_gives_each_flag_the_commands_that_take_it() {
    let commands = cli::commands();
    let readme = read_doc(ROOT.join("README.md"));
    let rows = flag_table(&readme);
    assert!(rows.len() > 10, "README.md's | Flag | Description | table has {} rows", rows.len());
    let mut errors = Vec::new();
    for row in &rows {
        let at = readme.at(&row.line);
        let scope = row_scope(&row.description);
        // A command row_scope can read: lowercase letters and hyphens.
        let opening = code_spans(&row.description)
            .first()
            .copied()
            .filter(|c| row.description.starts_with('`') && c.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
        if let (None, Some(command)) = (&scope, opening.filter(|c| commands.contains_key(*c))) {
            errors.push(format!(
                "{at}: the row opens with the command {command}, but not as a list of commands and a colon, which \
                 this test reads"
            ));
        }
        let (scope, only) = scope.unwrap_or_default();
        for flag in &row.flags {
            let taking: BTreeSet<String> =
                commands.iter().filter(|(_, defined)| defined.contains(*flag)).map(|(name, _)| name.clone()).collect();
            let list = |names: &BTreeSet<String>| names.iter().cloned().collect::<Vec<_>>().join(", ");
            if taking.is_empty() {
                errors.push(format!("{at}: no command takes {flag}"));
            } else if !scope.is_subset(&taking) {
                errors.push(format!(
                    "{at}: {flag} is said to be for {}, but the parser gives it to {}",
                    list(&scope),
                    list(&taking)
                ));
            } else if only && taking != scope {
                errors.push(format!(
                    "{at}: {flag} is said to be for {} only, but the parser gives it to {}",
                    list(&scope),
                    list(&taking)
                ));
            }
        }
    }
    assert_no_errors(&errors);
}

// In the README or, for a command the README's table leaves out, in docs/. A flag counts as named
// wherever its name appears, said of any command or program. A missing flag has no line of its own, so the failure
// names the line of the README's flag table, where a flag of the orchestrator's is documented.
#[test]
fn each_flag_is_documented() {
    let docs = docs();
    let documented: BTreeSet<&str> = docs.iter().flat_map(|d| flags(&d.text)).collect();
    let readme = &docs[0];
    let table = readme.lines().into_iter().find(|l| l.text.starts_with("| Flag | Description |"));
    let at = table.map_or_else(|| readme.shown.clone(), |l| readme.at(&l));
    let errors: Vec<String> = cli::commands()
        .iter()
        .flat_map(|(name, defined)| defined.iter().map(move |flag| (name, flag)))
        .filter(|(_, flag)| *flag != "--help" && !documented.contains(flag.as_str()))
        .map(|(name, flag)| format!("{at}: {name} takes {flag}, which neither README.md nor docs/*.md names"))
        .collect();
    assert_no_errors(&errors);
}

/// The parts of a code span that names a member, `Type.member`, `Type::member` and either with `()` after: the type
/// and the member; None for any other span.
fn member_name(code: &str) -> Option<(&str, &str)> {
    let code = code.strip_suffix("()").unwrap_or(code);
    let (owner, member) = code.split_once("::").or_else(|| code.split_once('.'))?;
    let word = |s: &str| !s.is_empty() && s.chars().all(word_char);
    let owner_ok = word(owner) && owner.starts_with(|c: char| c.is_ascii_uppercase());
    let member_ok = word(member) && member.starts_with(|c: char| c.is_ascii_lowercase() || c == '_');
    (owner_ok && member_ok).then_some((owner, member))
}

/// Whether code is all capitals, digits and `_`, opening with a capital, as a constant or an environment variable,
/// but not a linter's rule code, capitals then digits only, such as C901 or BLE001.
fn constant_name(code: &str) -> bool {
    let caps = code.len() > 1
        && code.starts_with(|c: char| c.is_ascii_uppercase())
        && code.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    let letters = code.trim_end_matches(|c: char| c.is_ascii_digit());
    let rule_code = letters.len() < code.len() && letters.chars().all(|c| c.is_ascii_uppercase());
    caps && !rule_code
}

#[test]
fn the_names_the_docs_give_exist_in_the_orchestrator() {
    let names = names::defined();
    let types: BTreeSet<&str> = names.members.iter().filter_map(|m| m.split_once('.')).map(|(t, _)| t).collect();
    let methods: BTreeSet<&str> = names.members.iter().filter_map(|m| m.split_once('.')).map(|(_, m)| m).collect();
    let mut errors = Vec::new();
    for doc in docs().iter().filter(|d| !d.historical()) {
        for line in doc.lines() {
            for code in code_spans(line.text) {
                let at = doc.at(&line);
                if constant_name(code) {
                    let other = OTHER_PROGRAMS_NAMES.iter().any(|(name, _)| *name == code);
                    if !names.items.contains(code) && !names.strings.contains(code) && !other {
                        errors.push(format!("{at}: the Rust has no {code}"));
                    }
                } else if let Some(function) =
                    code.strip_suffix("()").filter(|f| !f.is_empty() && f.chars().all(word_char))
                {
                    if !names.items.contains(function) && !methods.contains(function) {
                        errors.push(format!("{at}: the Rust has no function or method {code}"));
                    }
                // A member of a type the code defines, or of a name with a lowercase letter: not CONSTANT.md, a
                // prompt's file, nor one of FILE_NAMES, nor Self's.
                } else if let Some((owner, member)) = member_name(code) {
                    let file = FILE_NAMES.iter().any(|(name, _)| *name == code);
                    let typed = types.contains(owner) || owner.chars().any(|c| c.is_lowercase());
                    if !file && owner != "Self" && typed && !names.members.contains(&format!("{owner}.{member}")) {
                        errors.push(format!("{at}: the Rust has no {owner}.{member}"));
                    }
                }
            }
        }
    }
    assert_no_errors(&errors);
}

#[test]
fn each_relative_link_resolves_to_a_file_and_its_anchor_to_a_heading() {
    let mut errors = Vec::new();
    // The anchors of each markdown file a link names, read once.
    let mut anchors: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    for doc in docs() {
        for line in doc.lines().into_iter().filter(|l| !l.fenced) {
            let prose = markdown::without_code(line.text);
            for target in markdown::link_targets(&prose) {
                if is_url(target) {
                    continue;
                }
                let (path, anchor) = target.split_once('#').unwrap_or((target, ""));
                // GitHub takes a path that starts with / from the repository's root.
                let dest = if path.is_empty() {
                    doc.path.clone()
                } else if let Some(from_root) = path.strip_prefix('/') {
                    ROOT.join(unescape(from_root.trim_start_matches('/')))
                } else {
                    doc.path.parent().unwrap_or(&ROOT).join(unescape(path))
                };
                let at = doc.at(&line);
                if !dest.exists() {
                    errors.push(format!("{at}: {target} names no file"));
                } else if !anchor.is_empty() && dest.extension().is_some_and(|e| e == "md") {
                    // The anchor of a file other than markdown, such as #L10 of a source file, names a line.
                    let headings = anchors.entry(dest.clone()).or_insert_with(|| {
                        let text = fs::read_to_string(&dest).unwrap_or_else(|e| panic!("reading {}: {e}", dest.display()));
                        markdown::anchors(&text)
                    });
                    if !headings.contains(anchor) {
                        let shown = dest.strip_prefix(&*ROOT).unwrap_or(&dest).display();
                        errors.push(format!("{at}: {target} names no heading of {shown}"));
                    }
                }
            }
        }
    }
    assert_no_errors(&errors);
}

/// Whether a link's target is a URL, opening with a scheme and a colon, such as https: or mailto:.
fn is_url(target: &str) -> bool {
    let scheme = target.find(':').map_or("", |i| &target[..i]);
    scheme.starts_with(|c: char| c.is_ascii_lowercase())
        && scheme.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c))
}

/// A link's path with its %XX escapes decoded, as GitHub reads it; an escape that is no hexadecimal pair stays.
fn unescape(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let pair = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        let hex = pair.filter(|h| h.chars().all(|c| c.is_ascii_hexdigit())).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
