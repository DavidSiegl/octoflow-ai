//! Crash the orchestrator right after a write of state.json, resume the run as `resume --force` does, and check that
//! it ends as an uninterrupted run does: the same files, the same state.json less its heartbeat and owner, and each
//! side effect on the world once. One crash after write k, for every k of each scenario; a run crashed several times
//! over is not tried.
//!
//! A crash is an unwind with Crashed from FakeState::after_write, right after the host wrote state.json, whichever of
//! Workflow::save and Workflow::claim wrote it. An unwind, not an error the hook returns: the Workflow handles an
//! error of a write, logging a heartbeat's and recording any other in state.json (Workflow::release), and SIGKILL
//! gives it neither chance. Nothing on the unwinding path saves or releases anything: the engine catches no unwind,
//! and neither Workflow nor anything it holds has a Drop that touches the world (interrupt::Watch's only forgets its
//! own registration). resume_unwind skips the panic hook, so a crash prints nothing.
//!
//! The resume is cli::resumed_state for `resume --force a1b2c3` and cli::run_workflow, as main runs that command,
//! on the same fakes, which play the world that outlives the orchestrator: the project (FakeHost), herdr and its
//! agents (FakeHerdr), Jenkins (FakeJenkins) and time (FakeClock). Each agent works on its prompt for TURN_SECONDS of
//! that time before its handoff file appears, so that a crash can land while it works and it finishes for the next
//! orchestrator.
//!
//! What the e2e scenarios cover instead: they kill the engine as a program, with SIGKILL at any moment, so also
//! between two writes, on real git; but only once per phase. This harness crashes after every write of state.json
//! but only there, and only on the fakes, so a bug in how Host drives git, gh or herdr is not its to find.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use super::*;
use crate::ci::fake::{jenkins_on, Outcome, JOB};
use crate::cli::{parse_args, resumed_state, run_workflow, Command, Connected, Ran};
use crate::constants::{CI_REF_PREFIX, HEARTBEAT_SECONDS};
use crate::env::ConfigDir;
use crate::host::fake::FakeClock;
use crate::pipeline::{Pipeline, DONE};
use crate::test_support::EXAMPLES;
use crate::workflow::Collaborators;
use crate::workflow_file::load_workflow_file;

/// The run's state.json, which every scenario's run writes.
const STATE_PATH: &str = "/proj/.orchestrator/runs/20260929-120000-a1b2c3/state.json";

/// Seconds each agent works on a prompt before its handoff file appears: past HEARTBEAT_SECONDS, so every turn has
/// a heartbeat save to crash after while the agent works.
const TURN_SECONDS: f64 = 90.0;

/// How many prompts each role's agent answers: more than any scenario sends it, so that an agent prompted again after
/// a crash works again, as a real one would.
const TURNS_PER_ROLE: usize = 16;

/// A run to crash: what its verdict steps answer each round, and the flags it starts with.
struct Scenario {
    /// Every verdict step's answer in round n is verdicts[n - 1]; the run ends with the last.
    verdicts: &'static [&'static str],
    /// Whether the run ends in a pull request.
    pull_request: bool,
    /// The outcome of each quality build in turn, a green one once they run out; None for a run without the quality
    /// gate.
    gate: Option<&'static [fn() -> Outcome]>,
    /// An example workflow's file name in EXAMPLES; None for the default workflow.
    workflow_file: Option<&'static str>,
}

const APPROVED: Scenario = Scenario { verdicts: &[APPROVE], pull_request: true, gate: None, workflow_file: None };

/// The orchestrator's death right after a write of state.json: the payload of the unwind that stops it.
struct Crashed;

/// One effect on the world, by kind: "prompt", "herdr", "git", "snapshot", "restore", "pull request", "jenkins" or
/// "notification", and the effect as text.
type Effects = BTreeMap<&'static str, Vec<String>>;

/// What a run leaves behind.
#[derive(Clone, Debug)]
struct Aftermath {
    /// Every file the host holds but state.json, by path.
    files: BTreeMap<String, String>,
    /// state.json, less heartbeat_at and owner, which only say which orchestrator wrote it when.
    state: Object,
    effects: Effects,
}

/// An answer an agent is still working on: when it is due on the clock, the agent's name, and the files it writes,
/// (path, text).
type Work = (f64, String, Vec<(String, String)>);

/// One scenario's run and the world it runs in, which a crash leaves as it was: the fakes, and agents that keep
/// working while no orchestrator runs.
struct World {
    scenario: &'static Scenario,
    /// The fakes every orchestrator of the run runs on, one after another; f.state is the new run's state.
    f: Fixture,
    /// The run's workflow, as the new run is started with it.
    pipeline: Pipeline,
    /// How many times every orchestrator so far wrote state.json.
    state_writes: Rc<Cell<usize>>,
    /// The state.json each crash left, in order.
    crashes: Vec<Object>,
    /// Whether an orchestrator has run it already, so that the next one resumes it.
    started: bool,
}

impl World {
    /// scenario's run, not started yet, its agents playing agent_turn.
    fn new(scenario: &'static Scenario) -> World {
        let pipeline = match scenario.workflow_file {
            Some(name) => load_workflow_file(&format!("{EXAMPLES}/{name}"), &Default::default())
                .expect("the example workflow loads"),
            None => new_state().pipeline().expect("the default workflow loads"),
        };
        let mut state = new_run(&pipeline);
        state.pull_request = scenario.pull_request;
        state.quality_job = scenario.gate.map(|_| JOB.to_string());
        let f = Fixture::on(Rc::new(FakeHost::default()), state, vec![]);
        if let Some(outcomes) = scenario.gate {
            f.ci.state().outcomes = outcomes.iter().map(|outcome| outcome()).collect();
        }
        let work: Rc<RefCell<Vec<Work>>> = Rc::default();
        for role in pipeline.roles().keys() {
            let turns = (0..TURNS_PER_ROLE).map(|_| agent_turn(scenario, &pipeline, role, &f.clock, &work)).collect();
            f.play(role, turns);
        }
        let (host, herdr) = (Rc::clone(&f.host), Rc::clone(&f.herdr));
        f.clock.hook(move |now| {
            let due: Vec<Work> = work.borrow_mut().extract_if(.., |(at, _, _)| *at <= now).collect();
            for (_, agent, files) in due {
                for (path, text) in files {
                    host.write(&path, &text, false).expect("the fake host writes");
                }
                herdr.state().statuses.insert(agent, "done".to_string());
            }
        });
        World { scenario, f, pipeline, state_writes: Rc::default(), crashes: vec![], started: false }
    }

    /// Start the run, or once started resume it as `resume --force` does, and return whether it finished; with
    /// crash_after, it crashes right after that many more writes of state.json, unless it finishes first. Panics when
    /// the run stops otherwise, resume refuses it, or it ends with another verdict than the scenario's last.
    fn run(&mut self, crash_after: Option<usize>) -> bool {
        let crash_at = crash_after.map(|k| self.state_writes.get() + k);
        // Armed anew for each orchestrator: a hook that unwinds is never put back (FakeHost::write).
        let writes = Rc::clone(&self.state_writes);
        self.f.host.state().after_write = Some(Box::new(move |path| {
            if path == STATE_PATH {
                writes.set(writes.get() + 1);
                if Some(writes.get()) == crash_at {
                    resume_unwind(Box::new(Crashed));
                }
            }
        }));
        let resume = self.started;
        self.started = true;
        let ran = catch_unwind(AssertUnwindSafe(|| self.drive(resume)));
        self.f.host.state().after_write = None;
        match ran {
            Err(payload) if payload.is::<Crashed>() => {
                self.crashes.push(self.f.saved());
                false
            }
            Err(payload) => resume_unwind(payload),
            Ok(Err(e)) => panic!("the run stopped: {e}"),
            Ok(Ok(ran)) => {
                let last = self.scenario.verdicts.last().copied();
                assert_eq!(ran.verdict.as_deref(), last, "the run ended with another verdict");
                true
            }
        }
    }

    /// One orchestrator's run on the world's fakes, as main runs `run` or, with resume, `resume --force a1b2c3`.
    fn drive(&self, resume: bool) -> Result<Ran, Stopped> {
        let jenkins = |_: &str| Ok(jenkins_on(&self.f.ci));
        let config = ConfigDir::at("/nowhere");
        let world = Connected {
            with: Collaborators { quality_gate: None, ..self.f.collaborators() },
            jenkins: &jenkins,
            config: &config,
            local_host: "here",
            stdout: &|_| {},
            stderr: &|_| {},
            workflow: &run_workflow,
        };
        if !resume {
            return run_workflow(&world, self.f.state.clone(), self.pipeline.clone(), None);
        }
        let argv = ["resume", "--force", "a1b2c3"].map(std::ffi::OsString::from);
        let Ok(Command::Resume(args)) = parse_args(argv, &config) else { panic!("resume --force does not parse") };
        let (state, pipeline) = resumed_state(&args, "/proj", &world).map_err(Stopped::Failed)?;
        run_workflow(&world, state, pipeline, None)
    }

    /// What the run has left behind so far.
    fn aftermath(&self) -> Aftermath {
        let mut files: BTreeMap<String, String> =
            self.f.host.state().files.iter().map(|(path, text)| (path.to_string(), text.clone())).collect();
        files.remove(STATE_PATH);
        let mut state = self.f.saved();
        state.remove("heartbeat_at");
        state.remove("owner");
        Aftermath { files, state, effects: self.effects() }
    }

    /// Each effect the run had on the world so far, by kind, in order. Reads and checks are not effects: what the
    /// orchestrator only looks at, it may look at again.
    fn effects(&self) -> Effects {
        let (h, herdr, ci) = (self.f.host.state(), self.f.herdr.state(), self.f.ci.state());
        let changes_git = ["commit", "push", "switch", "merge", "worktree", "branch", "update-ref"];
        let mut effects = Effects::new();
        effects.insert("prompt", herdr.prompts.iter().map(|(name, text)| format!("{name}: {text}")).collect());
        let calls = herdr.calls.iter().filter(|c| !matches!(c, Call::Prompt { .. }));
        effects.insert("herdr", calls.map(|c| format!("{c:?}")).collect());
        let git = h.git_log.iter().filter(|(_, args)| changes_git.contains(&args[0].as_str()));
        effects.insert("git", git.map(|(cwd, args)| format!("{cwd}: {}", args.join(" "))).collect());
        effects.insert("snapshot", h.snapshots.iter().map(|s| format!("{s:?}")).collect());
        effects.insert("restore", h.restores.iter().map(|r| format!("{r:?}")).collect());
        let opened = h.prs.iter().map(|pr| format!("{pr:?}"));
        effects.insert("pull request", opened.chain(h.pr_edits.iter().map(|e| format!("{e:?}"))).collect());
        let triggers = ci.requests.iter().filter(|r| r.method == "POST" && r.url.ends_with("/buildWithParameters"));
        effects.insert("jenkins", triggers.map(|r| r.form.get("GIT_REF").cloned().unwrap_or_default()).collect());
        let notes = self.f.told.notes.borrow();
        effects.insert("notification", notes.iter().map(|(title, body)| format!("{title}: {body}")).collect());
        effects
    }
}

/// role's answer to a prompt: TURN_SECONDS later on clock, its handoff file for the step and round the saved run is
/// in, and an edit of the project if the role edits it.
fn agent_turn(
    scenario: &Scenario,
    pipeline: &Pipeline,
    role: &str,
    clock: &Rc<FakeClock>,
    work: &Rc<RefCell<Vec<Work>>>,
) -> Turn {
    let (verdicts, pipeline, role) = (scenario.verdicts, pipeline.clone(), role.to_string());
    let (clock, work) = (Rc::clone(clock), Rc::clone(work));
    Box::new(move |_, state, _| {
        let step = pipeline.step(&state.phase).unwrap_or_else(|| panic!("{role} prompted in phase {}", state.phase));
        let (n, q) = (state.round, if step.quality_gated { state.quality_round } else { 0 });
        let text = if step.loop_to.is_some() {
            format!("VERDICT: {}\n1. finding", verdicts[n as usize - 1])
        } else if pipeline.contract().is_some_and(|c| c.id == step.id) {
            SPEC.to_string()
        } else {
            format!("{} {n}.{q}", step.id)
        };
        let mut files = vec![(step.path(&state.dir(), n, q), text)];
        match role.as_str() {
            "build" => files.push(("/proj/limiter.py".to_string(), format!("version {n}.{q}"))),
            "tests" => files.push(("/proj/test_limiter.py".to_string(), "assert limit() == 3".to_string())),
            _ => {}
        }
        work.borrow_mut().push((clock.now() + TURN_SECONDS, format!("{role}-{}", state.key()), files));
        "working".to_string()
    })
}

/// Whether the code documents the effect of kind as harmless to repeat, so that it may come again right after
/// itself; any other repeat is a failure, or a known gap that `expected` names.
fn idempotent(kind: &str, effect: &str) -> bool {
    match kind {
        // The snapshot's push to its quality branch: --force replaces what a failed attempt left (Host::push_ref).
        "git" if effect.contains(" push --quiet --force origin ") => {
            effect.contains(&format!(":refs/heads/{CI_REF_PREFIX}"))
        }
        // Workflow::check_repo fast-forwards the base branch (Host::fast_forward) at each start in round 0.
        "git" => effect.contains(" merge --ff-only "),
        // A snapshot moves no ref; a lock's snapshot, a resume before the run moves on takes again, harmlessly
        // (Workflow::hold_locks).
        "snapshot" => true,
        _ => false,
    }
}

/// effects without each one that may_repeat and that only repeats the one before it.
fn without_repeats(effects: &Effects, may_repeat: fn(&str, &str) -> bool) -> Effects {
    let kept = |kind: &str, es: &[String]| -> Vec<String> {
        let repeat = |i: usize| i > 0 && es[i] == es[i - 1] && may_repeat(kind, &es[i]);
        (0..es.len()).filter(|&i| !repeat(i)).map(|i| es[i].clone()).collect()
    };
    effects.iter().map(|(kind, es)| (*kind, kept(kind, es))).collect()
}

/// What a run that crashed where each of crashes, the state.json each left, says leaves behind: uninterrupted's,
/// but for the one gap the engine accepts, pinned here.
///
/// A crash after the save that released the finished run (phase done, no owner) costs the human the notice that it
/// finished, and the closing of its workspace: both come once, after that save, and a resume of a released run whose
/// files are written has nothing left to do (Workflow::end_done_run, Workflow::ending_left).
fn expected(uninterrupted: &Aftermath, crashes: &[Object]) -> Aftermath {
    let mut want = uninterrupted.clone();
    let released = |saved: &Object| saved.get("phase") == Some(&Value::from(DONE)) && saved["owner"].is_null();
    if crashes.iter().any(released) {
        let effects = &mut want.effects;
        effects.entry("notification").or_default().retain(|note| !note.starts_with("Run finished: "));
        effects.entry("herdr").or_default().retain(|call| !call.starts_with("Close "));
    }
    want
}

/// What differs between got and want, as a line each; empty when nothing does. Repeats `idempotent` allows are no
/// difference.
fn ends_as_problems(got: &Aftermath, want: &Aftermath) -> Vec<String> {
    let mut problems = vec![];
    if got.files != want.files {
        let differ = |a: &BTreeMap<String, String>, b: &BTreeMap<String, String>| -> Vec<String> {
            a.iter().filter(|(p, t)| b.get(*p) != Some(t)).map(|(p, _)| p.clone()).collect()
        };
        problems.push(format!(
            "the files differ: {:?} only or otherwise got, {:?} only or otherwise wanted",
            differ(&got.files, &want.files),
            differ(&want.files, &got.files)
        ));
    }
    if got.state != want.state {
        let keys = got.state.keys().chain(want.state.keys()).filter(|k| got.state.get(*k) != want.state.get(*k));
        let keys: std::collections::BTreeSet<&String> = keys.collect();
        let shown: Vec<String> =
            keys.iter().map(|k| format!("{k}: {:?} != {:?}", got.state.get(*k), want.state.get(*k))).collect();
        problems.push(format!("state.json differs: {}", shown.join("; ")));
    }
    let (got, want) = (without_repeats(&got.effects, idempotent), without_repeats(&want.effects, idempotent));
    for (kind, effects) in &want {
        if got.get(kind) != Some(effects) {
            problems.push(format!("the {kind} effects differ: {:#?} != {effects:#?}", got.get(kind)));
        }
    }
    problems
}

/// The scenario's aftermath when nothing crashes, and how many writes of state.json it takes.
fn uninterrupted(scenario: &'static Scenario) -> (Aftermath, usize) {
    let mut world = World::new(scenario);
    assert!(world.run(None));
    (world.aftermath(), world.state_writes.get())
}

/// For each k: crash scenario's run after write k of state.json, resume it once, and check it ends as `expected`
/// says; every crash point's problems are reported together.
fn crash_after_every_write(scenario: &'static Scenario) {
    let (reference, writes) = uninterrupted(scenario);
    let mut problems = vec![];
    for k in 1..=writes {
        let mut world = World::new(scenario);
        assert!(!world.run(Some(k)), "the run finished before write {k} of {writes}");
        assert!(world.run(None));
        let want = expected(&reference, &world.crashes);
        let found = ends_as_problems(&world.aftermath(), &want);
        problems.extend(found.into_iter().map(|p| format!("crash after write {k}: {p}")));
    }
    assert!(problems.is_empty(), "over {writes} crash points:\n{}", problems.join("\n"));
}

#[test]
fn approved_in_round_1() {
    crash_after_every_write(&APPROVED);
}

#[test]
fn changes_requested_then_approved() {
    static SCENARIO: Scenario = Scenario { verdicts: &[CHANGES_REQUESTED, APPROVE], ..APPROVED };
    crash_after_every_write(&SCENARIO);
}

#[test]
fn quality_gate_fails_then_passes() {
    static SCENARIO: Scenario = Scenario { gate: Some(&[red, green]), ..APPROVED };
    crash_after_every_write(&SCENARIO);
}

#[test]
fn product_team_with_locked_tests() {
    static SCENARIO: Scenario = Scenario { workflow_file: Some("product-team.toml"), ..APPROVED };
    crash_after_every_write(&SCENARIO);
}

#[test]
fn no_pull_request() {
    static SCENARIO: Scenario = Scenario { pull_request: false, ..APPROVED };
    crash_after_every_write(&SCENARIO);
}

#[test]
fn agents_work_past_a_heartbeat() {
    // A heartbeat save during each agent's turn is what lets a crash land while an agent works.
    assert!(TURN_SECONDS > HEARTBEAT_SECONDS as f64);
}

#[test]
fn a_repeated_prompt_is_a_difference() {
    let (reference, _) = uninterrupted(&APPROVED);
    let mut again = reference.clone();
    let prompts = again.effects.get_mut("prompt").unwrap();
    prompts.push(prompts.last().unwrap().clone());
    assert_eq!(ends_as_problems(&again, &expected(&reference, &[])).len(), 1);
}
