//! Tab completion, computed by the program: the shell's registration script runs `octoflow` again on each Tab, with
//! COMPLETE set to the shell's name and the words of the line after `--` (clap_complete's [`CompleteEnv`]), and
//! [`answer_shell`] answers it from [`parser`]'s command tree, so completion cannot drift from what parses. Beyond
//! that tree it adds the saved runs of the project as the candidates of each command's RUN.
//!
//! Completion is read-only and stays local: no herdr, no signal handling, no move of an old project directory, nothing
//! written, and no run ids for a line with --machine, whose runs would be read over ssh while the shell waits.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;

use clap::builder::StyledStr;
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use clap_complete::CompleteEnv;

use super::args::parser;
use super::PROGRAM;
use crate::env::ConfigDir;
use crate::host::{Host, ProcessHost};
use crate::interrupt::Interrupt;
use crate::state::run_key;

/// When the environment's COMPLETE names a shell (bash, zsh, fish, elvish or powershell), answer it on standard output
/// and exit the process with status 0: with no words after `--` in the arguments, the shell's registration script,
/// which names the completer `octoflow` as PATH finds it, not the path this binary was run by; else the candidates for
/// the line those words are. A COMPLETE naming no shell it knows prints clap's error and exits with its status.
/// Otherwise, COMPLETE unset, empty or `0`, returns having done nothing.
///
/// Call it first in main, before anything is printed to standard output or a signal handler is installed: an answer
/// is all a completing process prints, and a Tab must not wait on anything but reading the runs.
pub fn answer_shell() {
    CompleteEnv::with_factory(|| {
        let words: Vec<OsString> = std::env::args_os().skip_while(|a| a != "--").skip(1).collect();
        // bash's and zsh's registration scripts say which word the cursor is on, as clap_complete reads it; fish's
        // passes the line up to the cursor only.
        let cursor = std::env::var("_CLAP_COMPLETE_INDEX").ok().and_then(|i| i.parse().ok());
        let command = parser(&ConfigDir::from_env());
        let target = LineTarget::scan(&command, &words, cursor);
        with_run_candidates(command, target)
    })
    .completer(PROGRAM)
    .bin(PROGRAM)
    .complete();
}

/// Where the line being completed reads its runs, as its words name it. A closure that gives RUN's candidates sees
/// none of the line (clap_complete's ValueCandidates takes no arguments), so the words are parsed before the command
/// tree is built.
#[derive(Debug, PartialEq, Eq)]
struct LineTarget {
    /// The --cwd given, as it was typed, a leading ~ not yet expanded; None for none, the shell's current directory.
    cwd: Option<String>,
    /// Whether the line gives a --machine, which an empty value does not, as when it parses (args.rs, target).
    machine: bool,
}

impl LineTarget {
    /// The target of the line words, the words the shell passes the completer, the program's name first. The word
    /// being completed, at index cursor (None for the last), is left out, and the others, those after it too, are
    /// parsed by command, parser's tree, as running the line would parse them, errors ignored, so an abbreviated --cw
    /// or --mach counts as it does there; bash's split of
    /// `--cwd=DIR` into `--cwd`, `=` and `DIR` (`=` is one of its COMP_WORDBREAKS) is joined back first. A word is
    /// taken as typed: quotes, backslashes and variables are not undone, as the shell would (bash and zsh pass the
    /// line's words raw).
    fn scan(command: &clap::Command, words: &[OsString], cursor: Option<usize>) -> LineTarget {
        let completed = cursor.unwrap_or(words.len().saturating_sub(1));
        let mut line: Vec<OsString> = vec![];
        let mut typed = words.iter().enumerate().filter(|(i, _)| *i != completed).map(|(_, w)| w);
        while let Some(word) = typed.next() {
            match line.last_mut() {
                Some(flag) if word == "=" && flag.to_str().is_some_and(|f| f.starts_with("--")) => {
                    flag.push("=");
                    flag.push(typed.next().map_or(OsString::new(), OsString::clone));
                }
                _ => line.push(word.clone()),
            }
        }
        let none = LineTarget { cwd: None, machine: false };
        let Ok(matches) = command.clone().ignore_errors(true).try_get_matches_from(line) else { return none };
        let Some((_, sub)) = matches.subcommand() else { return none };
        let flag = |id| sub.try_get_one::<String>(id).ok().flatten().filter(|v| !v.is_empty()).cloned();
        LineTarget { cwd: flag("cwd"), machine: flag("machine").is_some() }
    }
}

/// command, parser's tree, with the saved runs of target's project as the candidates of each subcommand's RUN
/// (`run_ref`): none for a target with --machine. The runs are read only when a RUN is completed.
fn with_run_candidates(command: clap::Command, target: LineTarget) -> clap::Command {
    let candidates = ArgValueCandidates::new(move || {
        if target.machine {
            return vec![];
        }
        let host = ProcessHost::local(&Interrupt::new());
        let dir = match &target.cwd {
            Some(dir) => dir.clone(),
            None => match std::env::current_dir() {
                Ok(dir) => dir.to_string_lossy().into_owned(),
                Err(_) => return vec![],
            },
        };
        run_candidates(&host, &dir)
    });
    command.mut_subcommands(|sub| {
        if sub.get_arguments().any(|a| a.get_id() == "run_ref") {
            sub.mut_arg("run_ref", |a| a.add(candidates.clone()))
        } else {
            sub
        }
    })
}

/// RUN's candidates in the project at dir, on host, dir as --cwd takes it (a leading ~ is the home directory): every
/// run Host::run_states finds, readable or not, newest first, each by its key, or by its full id where another run
/// has its key, since find_run takes such a key for neither; each with its phase and the first line of its task as its
/// help. None when dir is not a directory, or the runs cannot be read.
fn run_candidates(host: &dyn Host, dir: &str) -> Vec<CompletionCandidate> {
    let Ok(cwd) = host.resolve_dir(dir) else { return vec![] };
    let Ok(runs) = host.run_states(&cwd) else { return vec![] };
    let mut runs_by_key: HashMap<&str, usize> = HashMap::new();
    for run in &runs {
        *runs_by_key.entry(run_key(&run.shown().run_id)).or_default() += 1;
    }
    // run_states gives the runs oldest first, as their ids, which start with when they started, sort. Two run
    // directories whose state.json name one run, as a copy does, give it once.
    let mut offered = HashSet::new();
    runs.iter()
        .rev()
        .filter(|run| offered.insert(run.shown().run_id.as_str()))
        .map(|run| {
            let s = run.shown();
            let key = run_key(&s.run_id);
            let named_by = if runs_by_key[key] > 1 { s.run_id.as_str() } else { key };
            let phase = if run.problem().is_some() { "unreadable" } else { s.phase.as_str() };
            let task = s.task.lines().next().unwrap_or("");
            CompletionCandidate::new(named_by).help(Some(StyledStr::from(format!("{phase}: {task}"))))
        })
        .collect()
}

#[cfg(test)]
mod tests;
