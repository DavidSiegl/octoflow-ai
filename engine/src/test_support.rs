//! What the engine's unit tests share: a seeded generator of test inputs, in place of Python's Hypothesis, and a
//! temporary directory.
//!
//! The generated inputs are the same on every run unless the environment gives a run seed in OCTOFLOW_TEST_SEED,
//! which Jenkins's Test stage sets afresh for each build. A test that fails under a run seed prints it; run it
//! again on the same cases with `OCTOFLOW_TEST_SEED=<seed> cargo test <test's name>`.

/// The environment variable the engine's generated-input tests read their run seed from, a decimal u64 (unset or
/// empty, a run has none), and which a failing one names on its standard error (Rng), so that the case can be
/// generated again.
const RUN_SEED_VARIABLE: &str = "OCTOFLOW_TEST_SEED";

/// The run seed in var, which is what std::env::var gave for RUN_SEED_VARIABLE. Panics when the variable holds
/// anything but a decimal u64 or nothing, blanks around a number included.
fn run_seed(var: Result<String, std::env::VarError>) -> Option<u64> {
    match var {
        Ok(text) if text.is_empty() => None,
        Ok(text) => Some(text.parse().unwrap_or_else(|e| panic!("{RUN_SEED_VARIABLE}={text:?}: {e}"))),
        Err(std::env::VarError::NotPresent) => None,
        Err(e) => panic!("{RUN_SEED_VARIABLE}: {e}"),
    }
}

/// A deterministic stream of pseudo-random numbers (xorshift64*). Dropped while its thread panics, as when an
/// assert on what it generated fails, it prints the run seed it generated from to stderr, as the assignment to run
/// the test with again (the module comment); without a run seed it prints nothing, a plain rerun being the same
/// test.
pub struct Rng {
    /// xorshift64*'s state, never 0.
    state: u64,
    /// The run seed the state was started from; None when the environment gave none.
    run_seed: Option<u64>,
}

impl Rng {
    /// A stream that seed decides, together with the environment's run seed when there is one: two tests with
    /// different seeds generate different numbers under any run seed. Panics as run_seed does.
    pub fn new(seed: u64) -> Self {
        Self::under(seed, run_seed(std::env::var(RUN_SEED_VARIABLE)))
    }

    /// Rng::new(seed), with run_seed as what the environment gives.
    fn under(seed: u64, run_seed: Option<u64>) -> Self {
        let own = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let state = match run_seed {
            None => own,
            // splitmix64's finalizer, a bijection, so that run seeds one apart, or tests' seeds one apart under
            // one run seed, start streams with nothing in common.
            Some(run) => {
                let mut z = own.wrapping_add(run.wrapping_mul(0xD6E8_FEB8_6659_FD93));
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^ (z >> 31)
            }
        };
        Rng { state: state | 1, run_seed }
    }

    pub fn next(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number below n, which must not be 0.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// Text of up to max characters, none of them in excluded nor a NUL: mostly ASCII, with the characters that
    /// Python's and sh's notions of a line, a blank and a word disagree on mixed in.
    pub fn text(&mut self, max: usize, excluded: &str) -> String {
        const ODD: &[char] = &[
            '\n', '\r', '\t', '\x0b', '\x0c', '\x1c', '\x1d', '\x1e', '\x1f', '\u{85}', '\u{a0}', '\u{2028}',
            '\u{2029}', '\u{3000}', '\u{200b}', 'é', 'ß', '😀', '\u{0301}', '\u{0663}', '_', '#', '=', '\'', '"', '\\',
            '$', '`', '~', '{', '}', ':', '*', '>',
        ];
        let len = self.below(max + 1);
        let mut out = String::new();
        while out.chars().count() < len {
            let c = if self.below(3) == 0 {
                *self.pick(ODD)
            } else {
                char::from(b' ' + self.below(95) as u8)
            };
            if c != '\0' && !excluded.contains(c) {
                out.push(c);
            }
        }
        out
    }
}

impl Drop for Rng {
    fn drop(&mut self) {
        if let (Some(run_seed), true) = (self.run_seed, std::thread::panicking()) {
            eprintln!("{RUN_SEED_VARIABLE}={run_seed} was this failing test's run seed; set it to generate the same cases again");
        }
    }
}

/// A directory of its own under the system's temporary directory, removed with everything in it when dropped; a
/// removal that fails is ignored. Its paths come as Strings, which is what the engine's functions under test take.
pub struct TempDir(tempfile::TempDir);

impl TempDir {
    pub fn new() -> Self {
        TempDir(tempfile::Builder::new().prefix("octoflow-engine-test-").tempdir().expect("a temporary directory"))
    }

    /// The directory's path, as a str.
    pub fn path(&self) -> String {
        self.0.path().to_str().expect("a UTF-8 path").to_string()
    }

    /// The path of name in the directory, as a str.
    pub fn join(&self, name: &str) -> String {
        self.0.path().join(name).to_str().expect("a UTF-8 path").to_string()
    }
}

/// What starts a command with SIGINT at its default, so that a shell can trap it: one that the tests' process ignores,
/// as Jenkins's does, stays ignored in what it starts, as in Python, and a shell cannot trap a signal ignored on entry.
pub const HEARS_SIGINT: [&str; 2] = ["env", "--default-signal=INT"];

/// A shell script for a program the engine runs, such as a fake herdr, that runs body with SIGINT at its default,
/// whatever the tests' process does with it: it starts itself again through HEARS_SIGINT, once, with its arguments.
pub fn hearing_sigint(body: &str) -> String {
    format!(
        "#!/bin/sh\n[ -n \"$HEARS_SIGINT\" ] || {{ HEARS_SIGINT=1; export HEARS_SIGINT; exec {} \"$0\" \"$@\"; }}\n{body}",
        HEARS_SIGINT.join(" ")
    )
}

/// How long the Ctrl-C tests wait for a command to get somewhere or a process to end, before they fail.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(20);

/// Raise interrupt, from a thread of its own, once path exists: the human's Ctrl-C while a command that writes path
/// as it starts runs. The thread panics, failing the join, when path is not written within PATIENCE.
pub fn raise_once_written(path: &str, interrupt: &crate::interrupt::Interrupt) -> std::thread::JoinHandle<()> {
    let (path, interrupt) = (path.to_string(), interrupt.clone());
    std::thread::spawn(move || {
        assert!(eventually(|| std::fs::metadata(&path).is_ok_and(|m| m.len() > 0)), "{path} was not written");
        interrupt.raise();
    })
}

/// Whether the process whose pid path holds, as `echo $$` wrote it, has ended (gone, or a zombie no one reaps yet)
/// within PATIENCE: on Linux, as /proc shows it.
pub fn ended(path: &str) -> bool {
    let pid = std::fs::read_to_string(path).expect("a pid file").trim().to_string();
    eventually(|| match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // The state follows the parenthesized command name.
        Ok(stat) => stat.rsplit_once(") ").is_some_and(|(_, rest)| rest.starts_with('Z')),
        Err(_) => true,
    })
}

/// Whether done holds within PATIENCE, asked every 10 ms.
pub fn eventually(mut done: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + PATIENCE;
    while std::time::Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    done()
}

/// The repository's examples/workflows/.
pub const EXAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/workflows");

/// The bytes a string of hex digits stands for, as an oracle file writes bytes that are not UTF-8.
pub fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex digits")).collect()
}

/// An oracle file of engine/tests/oracle/, which its generator there wrote from the Python engine, parsed.
pub fn oracle(file: &str) -> serde_json::Value {
    let text = match file {
        "texts.json" => include_str!("../tests/oracle/texts.json"),
        "workflows.json" => include_str!("../tests/oracle/workflows.json"),
        "quality.json" => include_str!("../tests/oracle/quality.json"),
        "helpers.json" => include_str!("../tests/oracle/helpers.json"),
        "host.json" => include_str!("../tests/oracle/host.json"),
        "checks.json" => include_str!("../tests/oracle/checks.json"),
        "harness.json" => include_str!("../tests/oracle/harness.json"),
        "cli.json" => include_str!("../tests/oracle/cli.json"),
        _ => panic!("no oracle file {file}"),
    };
    serde_json::from_str::<serde_json::Value>(text).unwrap_or_else(|e| panic!("{file}: {e}"))
}

// -- The workflows tests/test_orchestrator.py defines, by its names -------------------------------------------------

use crate::pipeline::{Pipeline, PipelineSettings, Step};
use crate::map::Map;
use crate::json::Object;

/// A role map of (key, label) pairs, in pane order.
pub fn roles(pairs: &[(&str, &str)]) -> Map<String> {
    pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
}

/// Step::new, with again set.
pub fn again(mut st: Step, text: &str) -> Step {
    st.again = Some(text.to_string());
    st
}

/// No spec step: the task is the Builder's contract.
pub fn quick() -> Pipeline {
    let mut build = again(Step::new("build", "build", "build-{n}.md", "Build {task} in {cwd}; report to {build_path}."),
        "Fix {prev_review_path}; report to {build_path}.");
    build.edits = true;
    let mut review = again(Step::new("review", "review", "review-{n}.md", "Review {change} against {task}; write {review_path}."),
        "Recheck {build_path}; write {review_path}.");
    review.loop_to = Some("build".into());
    Pipeline::new("quick", roles(&[("build", "Builder"), ("review", "Reviewer")]), vec![build, review]).unwrap()
}

/// quick with a second verdict step after the review: the Product Manager accepts the change or sends it back to
/// the Builder, who is told the findings of whichever sent the round back.
pub fn accepting() -> Pipeline {
    let mut accept = again(Step::new("accept", "accept", "accept-{n}.md", "Accept {build_path} against {task}; write {accept_path}."),
        "Accept again; write {accept_path}.");
    accept.loop_to = Some("build".into());
    let mut steps = quick().steps().to_vec();
    steps[0].again = Some("Fix {findings}; report to {build_path}.".into());
    steps.push(accept);
    let roles = roles(&[("build", "Builder"), ("review", "Reviewer"), ("accept", "Product Manager")]);
    Pipeline::new("accepting", roles, steps).unwrap()
}

/// Four roles: the tests are written once, and the review loop goes back to the Builder only.
pub fn four_steps() -> Vec<Step> {
    let mut spec = Step::new("spec", "spec", "spec.md", "Interview about {task}; write {spec_path}.");
    spec.human_paced = true;
    let mut tests = Step::new("tests", "tests", "tests.md", "Write tests for {spec_path}; report to {tests_path}.");
    tests.edits = true;
    let mut build = again(Step::new("build", "build", "build-{n}.md", "Build {spec_path} against {tests_path}; report to {build_path}."),
        "Fix {prev_review_path}; report to {build_path}.");
    build.fresh_note = Some("Earlier reports: {earlier_build_paths}.".into());
    build.fresh_repeats_again = true;
    build.edits = true;
    let mut review = again(Step::new("review", "review", "review-{n}.md", "Review {change}; write {review_path}."),
        "Recheck; write {review_path}.");
    review.loop_to = Some("build".into());
    vec![spec, tests, build, review]
}

pub fn four_roles() -> Map<String> {
    roles(&[("spec", "Spec Collector"), ("tests", "Test Writer"), ("build", "Builder"), ("review", "Reviewer")])
}

pub fn four() -> Pipeline {
    Pipeline::new("four", four_roles(), four_steps()).unwrap()
}

/// FOUR with its Test Writer locking the tests it writes.
pub fn locking() -> Pipeline {
    let steps = four_steps().into_iter().map(|mut st| { st.locks = st.id == "tests"; st }).collect();
    Pipeline::new("locking", four_roles(), steps).unwrap()
}

/// The quality-gated step is not called build.
pub fn impl_() -> Pipeline {
    let mut imp = again(Step::new("impl", "impl", "impl-{n}.md", "Implement {task}; report to {impl_path}."),
        "Fix {prev_review_path}; report to {impl_path}.");
    imp.fresh_note = Some("Earlier reports: {earlier_impl_paths}.".into());
    imp.edits = true;
    imp.quality_gated = true;
    let mut review = again(Step::new("review", "review", "review-{n}.md", "Review {change} with {impl_path}; write {path}."),
        "Recheck {impl_path}; write {review_path}.");
    review.loop_to = Some("impl".into());
    Pipeline::new("impl", roles(&[("impl", "Implementer"), ("review", "Reviewer")]), vec![imp, review]).unwrap()
}

/// No verdict step: the run finishes when the Builder's one turn is done.
pub fn solo() -> Pipeline {
    let mut build = Step::new("build", "build", "build.md", "Build {task} in {cwd}; report to {path}.");
    build.edits = true;
    Pipeline::new("solo", roles(&[("build", "Builder")]), vec![build]).unwrap()
}

/// p with other settings, validated again, as dataclasses.replace makes one.
pub fn replaced(p: &Pipeline, change: impl FnOnce(&mut PipelineSettings)) -> Result<Pipeline, String> {
    let mut settings = p.settings().clone();
    change(&mut settings);
    Pipeline::with(p.name(), p.roles().clone(), p.steps().to_vec(), settings)
}

/// Each recording of the e2e scenarios, engine/e2e/recorded/<scenario>.json, by scenario, in name order.
pub fn recordings() -> Vec<(String, serde_json::Value)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/e2e/recorded");
    let mut paths: Vec<_> = std::fs::read_dir(dir).expect("the recordings").map(|e| e.expect("an entry").path()).collect();
    paths.sort();
    paths
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .map(|p| {
            let text = std::fs::read_to_string(&p).expect("a recording");
            let name = p.file_stem().and_then(|s| s.to_str()).expect("a name").to_string();
            let recording = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            (name, recording)
        })
        .collect()
}

/// The state a recording's saved state.json object holds, read as the engine reads one, its "<time>" and
/// "<seconds>" made values (denormalized), as a turn record's seconds must be a number. Panics: it does not read.
pub fn recorded_state(saved: &serde_json::Map<String, serde_json::Value>) -> crate::state::RunState {
    let text = denormalized(&serde_json::Value::Object(saved.clone()).to_string(), 1_791_000_000);
    crate::state::RunState::from_text(&text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// A run of helpers.json's cases with the inputs engine/tests/oracle/helpers.py gives its helpers, read from its
/// recording as that script reads them, and the outputs it recorded.
pub struct HelperCase {
    /// The case: its recording, run directory and conflicts, and the Python engine's outputs by helper.
    pub expected: serde_json::Value,
    pub state: crate::state::RunState,
    pub spec: String,
    pub report: String,
    pub review: String,
    pub evidence: crate::pull_request::GateEvidence,
    pub now: i64,
}

/// helpers.json's cases, their inputs read from the recordings.
pub fn helper_cases() -> Vec<HelperCase> {
    use serde_json::Value;
    let oracle = oracle("helpers.json");
    let epoch = oracle.get("epoch").and_then(Value::as_i64).unwrap();
    let now = oracle.get("now").and_then(Value::as_i64).unwrap();
    let recordings = recordings();
    let mut out = Vec::new();
    for case in oracle.get("cases").and_then(Value::as_array).unwrap() {
        let field = |key: &str| case.get(key).and_then(Value::as_str).unwrap().to_string();
        let (recording, run_dir) = (field("recording"), field("run_dir"));
        let name = recording.strip_suffix(".json").unwrap();
        let files = recordings.iter().find(|(n, _)| n == name).unwrap().1.get("run_files").unwrap().as_object().unwrap().clone();
        let file = |name: &str| files.get(&format!("{run_dir}{name}")).and_then(Value::as_str).map(String::from);
        let text = denormalized(&file("state.json").unwrap(), epoch);
        let Value::Object(mut saved) = serde_json::from_str::<Value>(&text).unwrap() else { panic!("a state") };
        let conflicts = case.get("conflicts").and_then(Value::as_array).unwrap();
        if !conflicts.is_empty() {
            saved.insert("conflicts".into(), Value::Array(conflicts.clone()));
        }
        let state = crate::state::RunState::from_saved(&saved).unwrap();
        // helpers.py's last: the last of the run directory's files whose name matches, by name.
        let last = |matches: &dyn Fn(&str) -> bool| {
            let mut names: Vec<&str> = files.keys().filter_map(|n| n.strip_prefix(run_dir.as_str())).filter(|n| matches(n)).collect();
            names.sort();
            names.last().map(|n| (n.to_string(), file(n).unwrap()))
        };
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let body = |n: &str, prefix: &str| n.strip_prefix(prefix).and_then(|n| n.strip_suffix(".md")).map(String::from);
        let spec = last(&|n| n == "spec.md").map(|(_, t)| t).filter(|t| !t.is_empty()).unwrap_or(state.task.clone());
        let is_report = |n: &str| body(n, "build-").is_some_and(|b| match b.split_once("-q") {
            Some((round, q)) => digits(round) && digits(q),
            None => digits(&b),
        });
        let report = last(&is_report).map_or(String::new(), |(_, t)| t);
        let review = last(&|n| body(n, "review-").is_some_and(|b| digits(&b))).map_or(String::new(), |(_, t)| t);
        let is_quality = |n: &str| body(n, "quality-").is_some_and(|b| b.split_once('-').is_some_and(|(r, q)| digits(r) && digits(q)));
        let quality = last(&is_quality).map(|(n, text)| {
            let b = body(&n, "quality-").unwrap();
            let (round, q) = b.split_once('-').unwrap();
            crate::pull_request::QualityFile { round: round.parse().unwrap(), quality_round: q.parse().unwrap(), text }
        });
        let evidence = crate::pull_request::GateEvidence { last: quality, unjudged: state.quality_gate_disabled };
        out.push(HelperCase { expected: case.clone(), state, spec, report, review, evidence, now });
    }
    out
}

/// helpers.py's denormalized: text, a recording's state.json, with each "<time>" string in it a time as
/// crate::summary::iso_utc writes it, the first at epoch (Unix seconds) and each a minute after the one before, and
/// each "<seconds>" string a count, 5 and then 7 more each time.
pub fn denormalized(text: &str, epoch: i64) -> String {
    use serde_json::Value;
    let mut out = String::new();
    let (mut times, mut counts) = (0, 5);
    let mut rest = text;
    while let Some(at) = rest.find('"') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        if let Some(after) = rest.strip_prefix("\"<time>\"") {
            out.push_str(&Value::from(crate::summary::iso_utc(epoch + 60 * times)).to_string());
            times += 1;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("\"<seconds>\"") {
            out.push_str(&counts.to_string());
            counts += 7;
            rest = after;
        } else {
            out.push('"');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The summary of the default workflow approved in round 1, as TestTurnTimings has it: crate::summary's tests
/// make it of a state, the Workflow's of a whole run.
pub const GOLDEN_SUMMARY: &str = "\
# Run 20260929-120000-a1b2c3

- **Workflow:** default
- **Outcome:** APPROVE
- **Pull request:** https://github.com/o/r/pull/7
- **Branch:** `orchestrator/add-a-token-bucket-rate-limiter-a1b2c3`
- **Wall time:** 35m 00s, from the first turn's start to 2026-10-03T04:35:00+00:00

## Task

add a rate limiter

## Roles

| role | harness | model |
| --- | --- | --- |
| Spec Collector (spec) | claude | default |
| Builder (build) | codex | default |
| Reviewer (review) | claude | opus |

## Turns

| step | role | round | harness | model | start (UTC) | duration | file |
| --- | --- | --- | --- | --- | --- | --- | --- |
| spec (human-paced) | spec | 0 | claude | default | 2026-10-03 04:00:00 | 10m 00s | spec.md |
| build | build | 1 | codex | default | 2026-10-03 04:10:00 | 20m 00s | build-1.md |
| review | review | 1 | claude | opus | 2026-10-03 04:30:00 | 5m 00s | review-1.md |

## Rounds

- Round 1: review APPROVE
";

/// The JSON object text holds, as a saved record reads; panics on text that is not one.
pub fn object(text: &str) -> Object {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(map)) => map,
        other => panic!("not a JSON object: {other:?}"),
    }
}

/// resumable_state for `resume a1b2c3` with flags over one run, saved as saved 10^4 s ago, in /proj, on host "here",
/// whose pids are all dead: Python's resumable helpers.
pub fn resumable(flags: &[&str], saved: &Object) -> Result<crate::state::RunState, crate::error::OrchestratorError> {
    let argv = ["resume", "a1b2c3"].iter().chain(flags).map(std::ffi::OsString::from);
    let Ok(crate::cli::Command::Resume(args)) = crate::cli::parse_args(argv, &crate::env::ConfigDir::at("/nowhere")) else {
        panic!("not a resume: {flags:?}")
    };
    let runs = [crate::state::SavedRun::read(10_000, serde_json::Value::Object(saved.clone())).unwrap()];
    crate::cli::resumable_state(&runs, &args, "/proj", "here", &|_| false).map(|(state, _)| state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(mut rng: Rng) -> [u64; 3] {
        [rng.next(), rng.next(), rng.next()]
    }

    /// What Rng::new(10) generated first before there was a run seed.
    const FIRST_OF_SEED_10: [u64; 3] =
        [14_025_306_511_418_466_756, 1_319_076_152_937_349_890, 7_228_085_411_122_872_806];

    #[test]
    fn a_seed_alone_generates_what_it_always_has() {
        assert_eq!(first(Rng::under(10, None)), FIRST_OF_SEED_10);
    }

    #[test]
    fn the_run_seed_is_the_variables_number_and_none_when_it_is_unset_or_empty() {
        assert_eq!(run_seed(Err(std::env::VarError::NotPresent)), None);
        assert_eq!(run_seed(Ok(String::new())), None);
        assert_eq!(run_seed(Ok("18446744073709551615".into())), Some(u64::MAX));
    }

    #[test]
    #[should_panic(expected = "OCTOFLOW_TEST_SEED=\" 5\": invalid digit")]
    fn a_run_seed_that_is_no_number_panics() {
        run_seed(Ok(" 5".into()));
    }

    /// Asserts whichever half the run it is in can show: Jenkins's runs have a seed, a plain `cargo test` none.
    #[test]
    fn new_generates_from_the_environments_run_seed_and_as_ever_without_one() {
        match std::env::var_os(RUN_SEED_VARIABLE) {
            Some(seed) if !seed.is_empty() => assert_ne!(first(Rng::new(10)), FIRST_OF_SEED_10, "under {seed:?}"),
            _ => assert_eq!(first(Rng::new(10)), FIRST_OF_SEED_10),
        }
    }

    #[test]
    fn a_run_seed_changes_what_every_seed_generates_and_keeps_the_seeds_apart() {
        let streams: Vec<[u64; 3]> = [None, Some(0), Some(1), Some(2), Some(u64::MAX)]
            .into_iter()
            .flat_map(|run_seed| [1, 2, 3, 10, 11].map(|seed| first(Rng::under(seed, run_seed))))
            .collect();
        for (k, stream) in streams.iter().enumerate() {
            assert!(!streams[..k].contains(stream), "stream {k} repeats an earlier one: {streams:?}");
        }
        assert_eq!(first(Rng::under(10, Some(7))), first(Rng::under(10, Some(7))));
    }
}
