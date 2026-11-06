//! The commands the docs may run and the long flags each takes: the orchestrator's, as its help lists them.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;
use std::sync::OnceLock;

use octoflow_e2e::engine;

/// Each command of the orchestrator, by name, with the long flags its help lists. Read once per test process; panics
/// when the orchestrator cannot be run.
pub fn commands() -> &'static BTreeMap<String, BTreeSet<String>> {
    static COMMANDS: OnceLock<BTreeMap<String, BTreeSet<String>>> = OnceLock::new();
    COMMANDS.get_or_init(|| {
        let top = help(&[]);
        section(&top, "Commands:")
            .iter()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|&name| name != "help")
            .map(|name| (name.to_string(), option_flags(&help(&[name]))))
            .collect()
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
