//! The commands the docs may run and the long flags each takes: the orchestrator's, as its help lists them, and
//! gate.py's, a program of its own.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::process::Command;
use std::sync::OnceLock;

use octoflow_e2e::engine;

use crate::ROOT;

/// The command name the docs check gives gate.py.
pub const GATE: &str = "gate.py";

/// Each command of the orchestrator, by name, with the long flags its help lists, and gate.py's as the command
/// [`GATE`]. Read once per test process; panics when the orchestrator cannot be run or gate.py read.
pub fn commands() -> &'static BTreeMap<String, BTreeSet<String>> {
    static COMMANDS: OnceLock<BTreeMap<String, BTreeSet<String>>> = OnceLock::new();
    COMMANDS.get_or_init(|| {
        let top = help(&[]);
        let mut commands: BTreeMap<String, BTreeSet<String>> = section(&top, "Commands:")
            .iter()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|&name| name != "help")
            .map(|name| (name.to_string(), option_flags(&help(&[name]))))
            .collect();
        commands.insert(GATE.to_string(), gate_flags());
        commands
    })
}

/// What the orchestrator prints for `orchestrator <command...> --help`; panics when it cannot be run or fails.
fn help(command: &[&str]) -> String {
    let binary = engine::binary();
    let out = Command::new(&binary)
        .args(command)
        .arg("--help")
        .output()
        .unwrap_or_else(|e| panic!("running {} {command:?} --help: {e}", binary.display()));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{} {command:?} --help failed: {stderr}", binary.display());
    String::from_utf8(out.stdout).unwrap_or_else(|e| panic!("{command:?} --help printed no UTF-8: {e}"))
}

/// The lines of a clap help text's section under the line heading, such as "Commands:", up to the next blank line;
/// panics when the text has no such section.
fn section<'a>(text: &'a str, heading: &str) -> Vec<&'a str> {
    let mut lines = text.lines().skip_while(|&line| line != heading);
    assert!(lines.next().is_some(), "no {heading} in the orchestrator's help:\n{text}");
    lines.take_while(|line| !line.trim().is_empty()).collect()
}

/// The long flags a command's help lists under Options: each option's own line, as `  -h, --help` or
/// `      --cwd <cwd>`, indented 2 to 6 spaces; the description below it, indented further, may name other flags.
fn option_flags(help: &str) -> BTreeSet<String> {
    let Some((_, options)) = help.split_once("\nOptions:\n") else { return BTreeSet::new() };
    options
        .lines()
        .filter_map(|line| {
            let body = line.trim_start_matches(' ');
            if !(2..=6).contains(&(line.len() - body.len())) {
                return None;
            }
            let mut short = body.chars();
            let body = match (short.next(), short.next(), short.next(), short.next()) {
                (Some('-'), Some(c), Some(','), Some(' ')) if c.is_alphanumeric() || c == '_' => &body[4..],
                _ => body,
            };
            crate::markdown::flags(body).first().filter(|flag| body.starts_with(*flag)).map(|f| f.to_string())
        })
        .collect()
}

/// The long flags gate.py takes: each `--` string of an `add_argument` call in its `parse_args`, and argparse's
/// `--help`. Read as text, since gate.py is Python: its calls each name their flags first, and none hides one with
/// `help=argparse.SUPPRESS`, which this does not look for. Panics when gate.py cannot be read or has no `parse_args`.
fn gate_flags() -> BTreeSet<String> {
    let path = ROOT.join("gate.py");
    let source = fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let start = source.find("\ndef parse_args(").unwrap_or_else(|| panic!("no def parse_args in {}", path.display()));
    let body = &source[start + 1..];
    let body = &body[..body[1..].find("\ndef ").map_or(body.len(), |end| end + 1)];
    let mut flags: BTreeSet<String> = body
        .match_indices("add_argument(\"--")
        .filter_map(|(i, call)| {
            let flag = &body[i + call.len() - 2..];
            flag.find('"').map(|end| flag[..end].to_string())
        })
        .collect();
    flags.insert("--help".to_string());
    flags
}
